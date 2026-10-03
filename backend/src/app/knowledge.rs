use pgvector::Vector;
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::embed::{Embedder, Input};

pub const MIN_COSINE: f32 = 0.75;

/// Fallback trigram bar. Scores around this on real titles are noise; a real digest overlap sits above it.
const MIN_TRIGRAM: f64 = 0.4;

/// Title overlap that is a name match and may outrank a weaker cosine.
const NAME_TRIGRAM: f64 = 0.5;

const CARD_FIELD: &str = "card";
const SECTION_FIELD: &str = "section";
/// CPU embedding server accepts a handful of full digests per request.
const EMBED_BATCH: i64 = 4;

#[derive(Debug)]
pub enum Error {
    NotImplemented,
    Database(sqlx::Error),
    Embed(crate::embed::Error),
    EmbedCount { expected: usize, got: usize },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotImplemented => f.write_str("not implemented"),
            Error::Database(err) => write!(f, "database: {err}"),
            Error::Embed(err) => write!(f, "embed: {err}"),
            Error::EmbedCount { expected, got } => {
                write!(f, "embedder returned {got} vectors, expected {expected}")
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Database(err) => Some(err),
            Error::Embed(err) => Some(err),
            Error::NotImplemented | Error::EmbedCount { .. } => None,
        }
    }
}

impl From<sqlx::Error> for Error {
    fn from(err: sqlx::Error) -> Self {
        Error::Database(err)
    }
}

impl From<crate::embed::Error> for Error {
    fn from(err: crate::embed::Error) -> Self {
        Error::Embed(err)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Query {
    pub text: String,
    pub category: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub title: String,
    pub page_url: String,
    pub licence: String,
    pub categories: Vec<String>,
    pub snippet: String,
    pub film_url: Option<String>,
}

pub async fn search(
    pool: &PgPool,
    embedder: &dyn Embedder,
    query: &Query,
) -> Result<Vec<Hit>, Error> {
    if query.text.trim().is_empty() {
        return Ok(Vec::new());
    }
    if embeddings_present(pool, query.category.as_deref()).await? {
        vector_search(pool, embedder, query).await
    } else {
        trigram_search(pool, query).await
    }
}

pub async fn embed_missing(
    pool: &PgPool,
    embedder: &dyn Embedder,
    model: &str,
) -> Result<u64, Error> {
    let mut updated = 0_u64;
    loop {
        let wrote = embed_batch(pool, embedder, model).await?;
        if wrote == 0 {
            return Ok(updated);
        }
        updated += wrote;
    }
}

async fn embed_batch(pool: &PgPool, embedder: &dyn Embedder, model: &str) -> Result<u64, Error> {
    let mut tx = pool.begin().await?;
    let rows: Vec<Digested> = sqlx::query_as(
        r#"
        SELECT i.id, i.title, i.digest_text, i.page_sections
        FROM innovations i
        WHERE i.digest_text IS NOT NULL
          AND (
            NOT EXISTS (
                SELECT 1
                FROM innovation_chunks c
                WHERE c.innovation_id = i.id
                  AND c.field = 'card'
            )
            OR (
                CASE
                    WHEN jsonb_typeof(i.page_sections) = 'array'
                    THEN jsonb_array_length(i.page_sections)
                    ELSE 0
                END > 0
                AND NOT EXISTS (
                    SELECT 1
                    FROM innovation_chunks c
                    WHERE c.innovation_id = i.id
                      AND c.field = 'section'
                )
            )
          )
        ORDER BY i.id
        LIMIT $1
        FOR UPDATE OF i SKIP LOCKED
        "#,
    )
    .bind(EMBED_BATCH)
    .fetch_all(&mut *tx)
    .await?;

    for row in &rows {
        insert_missing_chunks(&mut tx, row).await?;
    }

    let pending: Vec<PendingChunk> = sqlx::query_as(
        r#"
        SELECT c.id, c.body
        FROM innovation_chunks c
        JOIN innovations i ON i.id = c.innovation_id
        WHERE i.digest_text IS NOT NULL
          AND c.embedding IS NULL
        ORDER BY c.id
        LIMIT $1
        FOR UPDATE OF c SKIP LOCKED
        "#,
    )
    .bind(EMBED_BATCH)
    .fetch_all(&mut *tx)
    .await?;

    if pending.is_empty() {
        tx.commit().await?;
        return Ok(0);
    }

    let texts: Vec<String> = pending.iter().map(|chunk| chunk.body.clone()).collect();
    let vectors = embedder.embed(Input::Passage, &texts).await?;
    if vectors.len() != texts.len() {
        return Err(Error::EmbedCount {
            expected: texts.len(),
            got: vectors.len(),
        });
    }

    let mut updated = 0_u64;
    for (chunk, vector) in pending.iter().zip(vectors) {
        let result = sqlx::query(
            r#"
            UPDATE innovation_chunks
            SET embedding = $1, embedding_model = $3
            WHERE id = $2
              AND embedding IS NULL
            "#,
        )
        .bind(Vector::from(vector))
        .bind(chunk.id)
        .bind(model)
        .execute(&mut *tx)
        .await?;
        updated += result.rows_affected();
    }
    tx.commit().await?;
    Ok(updated)
}

#[derive(sqlx::FromRow)]
struct Digested {
    id: Uuid,
    title: String,
    digest_text: String,
    page_sections: Value,
}

#[derive(sqlx::FromRow)]
struct PendingChunk {
    id: i64,
    body: String,
}

#[derive(sqlx::FromRow)]
struct VectorRow {
    title: String,
    page_url: String,
    licence: String,
    categories: Vec<String>,
    digest_text: String,
    chunk_body: Option<String>,
    film_url: Option<String>,
    title_sim: f64,
    cosine: Option<f64>,
}

#[derive(sqlx::FromRow)]
struct TrigramRow {
    title: String,
    page_url: String,
    licence: String,
    categories: Vec<String>,
    snippet: String,
    film_url: Option<String>,
    score: f64,
}

async fn embeddings_present(pool: &PgPool, category: Option<&str>) -> Result<bool, Error> {
    let present: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM innovation_chunks c
            JOIN innovations i ON i.id = c.innovation_id
            WHERE c.embedding IS NOT NULL
              AND i.digest_text IS NOT NULL
              AND (
                $1::text IS NULL
                OR EXISTS (
                    SELECT 1
                    FROM innovation_categories cat
                    WHERE cat.innovation_id = i.id
                      AND cat.slug = $1
                )
              )
        )
        "#,
    )
    .bind(category)
    .fetch_one(pool)
    .await?;
    Ok(present)
}

async fn vector_search(
    pool: &PgPool,
    embedder: &dyn Embedder,
    query: &Query,
) -> Result<Vec<Hit>, Error> {
    let embedded = embedder
        .embed(Input::Query, std::slice::from_ref(&query.text))
        .await?;
    if embedded.len() != 1 {
        return Err(Error::EmbedCount {
            expected: 1,
            got: embedded.len(),
        });
    }
    let vector = Vector::from(embedded.into_iter().next().expect("checked length"));
    let rows: Vec<VectorRow> = sqlx::query_as(
        r#"
        SELECT
            i.title,
            i.page_url,
            COALESCE(i.licence, '') AS licence,
            COALESCE(
                (
                    SELECT array_agg(cat.name ORDER BY cat.name)
                    FROM innovation_categories cat
                    WHERE cat.innovation_id = i.id
                ),
                '{}'::text[]
            ) AS categories,
            i.digest_text,
            best.body AS chunk_body,
            (
                SELECT l.url
                FROM innovation_links l
                WHERE l.innovation_id = i.id
                  AND lower(l.kind) IN ('video', 'film')
                ORDER BY l.id
                LIMIT 1
            ) AS film_url,
            similarity(lower(i.title), lower($2))::float8 AS title_sim,
            best.cosine
        FROM innovations i
        LEFT JOIN LATERAL (
            SELECT
                c.body,
                (1 - (c.embedding <=> $1::vector))::float8 AS cosine
            FROM innovation_chunks c
            WHERE c.innovation_id = i.id
              AND c.embedding IS NOT NULL
            ORDER BY c.embedding <=> $1::vector
            LIMIT 1
        ) best ON true
        WHERE i.digest_text IS NOT NULL
          AND (
            $3::text IS NULL
            OR EXISTS (
                SELECT 1
                FROM innovation_categories cat
                WHERE cat.innovation_id = i.id
                  AND cat.slug = $3
            )
          )
        "#,
    )
    .bind(vector)
    .bind(&query.text)
    .bind(query.category.as_deref())
    .fetch_all(pool)
    .await?;
    Ok(rank_vector_rows(rows))
}

fn rank_vector_rows(rows: Vec<VectorRow>) -> Vec<Hit> {
    let mut ranked = Vec::new();
    for row in rows {
        let cosine = row.cosine.unwrap_or(f64::NEG_INFINITY);
        let vector_hit = cosine >= f64::from(MIN_COSINE);
        let name_hit = row.title_sim >= NAME_TRIGRAM;
        if !vector_hit && !name_hit {
            continue;
        }
        let score = if name_hit && vector_hit {
            row.title_sim.max(cosine)
        } else if name_hit {
            row.title_sim
        } else {
            cosine
        };
        let snippet = if vector_hit {
            row.chunk_body
                .clone()
                .unwrap_or_else(|| row.digest_text.clone())
        } else {
            row.digest_text.clone()
        };
        ranked.push((
            score,
            Hit {
                title: row.title,
                page_url: row.page_url,
                licence: row.licence,
                categories: row.categories,
                snippet,
                film_url: row.film_url,
            },
        ));
    }
    ranked.sort_by(|left, right| {
        right
            .0
            .total_cmp(&left.0)
            .then_with(|| left.1.title.cmp(&right.1.title))
            .then_with(|| left.1.page_url.cmp(&right.1.page_url))
    });
    ranked.into_iter().map(|(_, hit)| hit).collect()
}

async fn trigram_search(pool: &PgPool, query: &Query) -> Result<Vec<Hit>, Error> {
    let rows: Vec<TrigramRow> = sqlx::query_as(
        r#"
        SELECT
            i.title,
            i.page_url,
            COALESCE(i.licence, '') AS licence,
            COALESCE(
                (
                    SELECT array_agg(cat.name ORDER BY cat.name)
                    FROM innovation_categories cat
                    WHERE cat.innovation_id = i.id
                ),
                '{}'::text[]
            ) AS categories,
            i.digest_text AS snippet,
            (
                SELECT l.url
                FROM innovation_links l
                WHERE l.innovation_id = i.id
                  AND lower(l.kind) IN ('video', 'film')
                ORDER BY l.id
                LIMIT 1
            ) AS film_url,
            GREATEST(
                similarity(lower(i.title), lower($1)),
                similarity(lower(i.digest_text), lower($1))
            )::float8 AS score
        FROM innovations i
        WHERE i.digest_text IS NOT NULL
          AND (
            $2::text IS NULL
            OR EXISTS (
                SELECT 1
                FROM innovation_categories cat
                WHERE cat.innovation_id = i.id
                  AND cat.slug = $2
            )
          )
          AND GREATEST(
                similarity(lower(i.title), lower($1)),
                similarity(lower(i.digest_text), lower($1))
          ) >= $3
        ORDER BY score DESC, i.title, i.page_url
        "#,
    )
    .bind(&query.text)
    .bind(query.category.as_deref())
    .bind(MIN_TRIGRAM)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter(|row| row.score >= MIN_TRIGRAM)
        .map(|row| Hit {
            title: row.title,
            page_url: row.page_url,
            licence: row.licence,
            categories: row.categories,
            snippet: row.snippet,
            film_url: row.film_url,
        })
        .collect())
}

async fn insert_missing_chunks(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    row: &Digested,
) -> Result<(), Error> {
    let fields: Vec<String> =
        sqlx::query_scalar("SELECT field FROM innovation_chunks WHERE innovation_id = $1")
            .bind(row.id)
            .fetch_all(&mut **tx)
            .await?;
    let has_card = fields.iter().any(|field| field == CARD_FIELD);
    let has_section = fields.iter().any(|field| field == SECTION_FIELD);

    let mut chunks = Vec::new();
    if !has_card {
        chunks.push((CARD_FIELD, card_body(&row.title, &row.digest_text)));
    }
    if !has_section {
        for body in section_bodies(&row.page_sections) {
            chunks.push((SECTION_FIELD, body));
        }
    }
    if chunks.is_empty() {
        return Ok(());
    }

    let mut ordinal: i32 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(ordinal), -1) FROM innovation_chunks WHERE innovation_id = $1",
    )
    .bind(row.id)
    .fetch_one(&mut **tx)
    .await?;
    for (field, body) in chunks {
        ordinal = ordinal
            .checked_add(1)
            .ok_or_else(|| sqlx::Error::Protocol("too many page sections".into()))?;
        sqlx::query(
            r#"
            INSERT INTO innovation_chunks (innovation_id, ordinal, field, body)
            VALUES ($1, $2, $3, $4)
            "#,
        )
        .bind(row.id)
        .bind(ordinal)
        .bind(field)
        .bind(body)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

fn card_body(title: &str, digest: &str) -> String {
    format!("{title}\n{digest}")
}

fn section_bodies(sections: &Value) -> Vec<String> {
    let Some(items) = sections.as_array() else {
        return Vec::new();
    };
    items.iter().filter_map(section_body).collect()
}

fn section_body(item: &Value) -> Option<String> {
    let heading = json_text(item, "heading");
    let body = json_text(item, "body");
    match (heading, body) {
        (None, None) => None,
        (Some(heading), None) => Some(heading),
        (None, Some(body)) => Some(body),
        (Some(heading), Some(body)) => Some(format!("{heading}\n{body}")),
    }
}

fn json_text(item: &Value, key: &str) -> Option<String> {
    let text = item.get(key)?.as_str()?.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use serde_json::json;
    use sqlx::postgres::PgPoolOptions;
    use tokio::sync::MutexGuard;
    use uuid::Uuid;

    use super::*;
    use crate::embed::Input;

    const TEST_DB: &str = "styrta_knowledge_test";

    static DB_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    static SCHEMA_READY: AtomicBool = AtomicBool::new(false);

    type EmbedCalls = Arc<Mutex<Vec<(Input, Vec<String>)>>>;

    struct FakeEmbedder {
        query: Vec<f32>,
        passage: Vec<f32>,
        calls: EmbedCalls,
    }

    #[async_trait]
    impl Embedder for FakeEmbedder {
        async fn embed(
            &self,
            input: Input,
            texts: &[String],
        ) -> Result<Vec<Vec<f32>>, crate::embed::Error> {
            self.calls
                .lock()
                .expect("call log")
                .push((input, texts.to_vec()));
            let template = match input {
                Input::Query => &self.query,
                Input::Passage => &self.passage,
            };
            Ok(vec![template.clone(); texts.len()])
        }
    }

    fn axis(index: usize) -> Vec<f32> {
        let mut values = vec![0.0; crate::embed::DIMENSION];
        values[index] = 1.0;
        values
    }

    fn angled(cosine: f32) -> Vec<f32> {
        let mut values = vec![0.0; crate::embed::DIMENSION];
        values[0] = cosine;
        values[1] = (1.0 - cosine * cosine).sqrt();
        values
    }

    fn fake(query: Vec<f32>, passage: Vec<f32>) -> (FakeEmbedder, EmbedCalls) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        (
            FakeEmbedder {
                query,
                passage,
                calls: Arc::clone(&calls),
            },
            calls,
        )
    }

    fn db_name(url: &str) -> &str {
        url.split('?')
            .next()
            .unwrap_or(url)
            .rsplit('/')
            .next()
            .unwrap_or("")
    }

    fn url_with_db(url: &str, db: &str) -> String {
        let (base, query) = match url.split_once('?') {
            Some((base, query)) => (base, Some(query)),
            None => (url, None),
        };
        let Some((prefix, _)) = base.rsplit_once('/') else {
            panic!("DATABASE_URL has no database name");
        };
        let mut out = format!("{prefix}/{db}");
        if let Some(query) = query {
            out.push('?');
            out.push_str(query);
        }
        out
    }

    fn database_url() -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(".env");
        let _ = dotenvy::from_path(path);
        std::env::var("DATABASE_URL").expect("DATABASE_URL")
    }

    fn test_database_url() -> String {
        let url = url_with_db(&database_url(), TEST_DB);
        assert_eq!(db_name(&url), TEST_DB);
        url
    }

    async fn connect_test_pool() -> PgPool {
        PgPoolOptions::new()
            .max_connections(2)
            .acquire_timeout(std::time::Duration::from_secs(5))
            .connect(&test_database_url())
            .await
            .expect("connect styrta_knowledge_test")
    }

    async fn prepare_database() {
        let admin_url = url_with_db(&database_url(), "postgres");
        assert_ne!(db_name(&admin_url), TEST_DB);
        let admin = PgPoolOptions::new()
            .max_connections(1)
            .connect(&admin_url)
            .await
            .expect("connect postgres");
        for attempt in 0..5 {
            let _ = sqlx::query(
                r#"
                SELECT pg_terminate_backend(pid)
                FROM pg_stat_activity
                WHERE datname = 'styrta_knowledge_test'
                  AND pid <> pg_backend_pid()
                "#,
            )
            .execute(&admin)
            .await;
            match sqlx::query("DROP DATABASE IF EXISTS styrta_knowledge_test")
                .execute(&admin)
                .await
            {
                Ok(_) => break,
                Err(err) if attempt == 4 => panic!("drop styrta_knowledge_test: {err}"),
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(50)).await,
            }
        }
        sqlx::query("CREATE DATABASE styrta_knowledge_test")
            .execute(&admin)
            .await
            .expect("create styrta_knowledge_test");
        admin.close().await;

        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(&test_database_url())
            .await
            .expect("connect empty styrta_knowledge_test");
        sqlx::raw_sql(include_str!("../../migrations/001_init.sql"))
            .execute(&pool)
            .await
            .expect("apply 001_init.sql");
        pool.close().await;
    }

    struct Fixture {
        pool: PgPool,
        _guard: MutexGuard<'static, ()>,
    }

    async fn fixture() -> Fixture {
        let _guard = DB_LOCK.lock().await;
        if !SCHEMA_READY.load(Ordering::Acquire) {
            prepare_database().await;
            SCHEMA_READY.store(true, Ordering::Release);
        }
        // A pool is bound to the runtime that opened it. Each tokio test has its own runtime.
        let pool = connect_test_pool().await;
        sqlx::query("TRUNCATE innovations CASCADE")
            .execute(&pool)
            .await
            .expect("truncate");
        Fixture { _guard, pool }
    }

    async fn insert_innovation(
        pool: &PgPool,
        title: &str,
        page_url: &str,
        licence: Option<&str>,
        digest: Option<&str>,
        sections: Value,
    ) -> Uuid {
        let status = if digest.is_some() {
            "digested"
        } else {
            "catalogued"
        };
        sqlx::query_scalar(
            r#"
            INSERT INTO innovations (
                slug, title, title_key, page_url, licence, digest_text, page_sections, status
            )
            VALUES (gen_random_uuid()::text, $1, lower($1), $2, $3, $4, $5, $6)
            RETURNING id
            "#,
        )
        .bind(title)
        .bind(page_url)
        .bind(licence)
        .bind(digest)
        .bind(sections)
        .bind(status)
        .fetch_one(pool)
        .await
        .expect("insert innovation")
    }

    async fn insert_category(pool: &PgPool, id: Uuid, slug: &str, name: &str) {
        sqlx::query(
            "INSERT INTO innovation_categories (innovation_id, slug, name) VALUES ($1, $2, $3)",
        )
        .bind(id)
        .bind(slug)
        .bind(name)
        .execute(pool)
        .await
        .expect("insert category");
    }

    async fn insert_link(pool: &PgPool, id: Uuid, kind: &str, url: &str) {
        sqlx::query("INSERT INTO innovation_links (innovation_id, kind, url) VALUES ($1, $2, $3)")
            .bind(id)
            .bind(kind)
            .bind(url)
            .execute(pool)
            .await
            .expect("insert link");
    }

    async fn insert_chunk(
        pool: &PgPool,
        id: Uuid,
        ordinal: i32,
        field: &str,
        body: &str,
        embedding: Option<Vec<f32>>,
    ) {
        sqlx::query(
            r#"
            INSERT INTO innovation_chunks (innovation_id, ordinal, field, body, embedding)
            VALUES ($1, $2, $3, $4, $5)
            "#,
        )
        .bind(id)
        .bind(ordinal)
        .bind(field)
        .bind(body)
        .bind(embedding.map(Vector::from))
        .execute(pool)
        .await
        .expect("insert chunk");
    }

    #[tokio::test]
    async fn fixture_hit_uses_only_stored_urls() {
        let fix = fixture().await;
        let pool = &fix.pool;
        let page_url = "https://fixture.example/innovation";
        let film_url = "https://fixture.example/film";
        let tempting = "https://not-stored.example/want";
        let decoy_url = "https://decoy.example/page";
        let id = insert_innovation(
            pool,
            "Fixture Innovation",
            page_url,
            Some("CC-BY-SA"),
            Some(&format!("Opis zawiera {tempting}")),
            json!([]),
        )
        .await;
        insert_category(pool, id, "seniorzy", "Seniorzy").await;
        insert_category(pool, id, "sport", "Sport").await;
        insert_link(pool, id, "page", page_url).await;
        insert_link(pool, id, "pdf", "https://fixture.example/card.pdf").await;
        insert_link(pool, id, "video", film_url).await;
        insert_chunk(
            pool,
            id,
            0,
            "card",
            "Hala sportowa przy szkole",
            Some(axis(0)),
        )
        .await;

        let decoy = insert_innovation(
            pool,
            "Decoy Innovation",
            decoy_url,
            None,
            Some("Inny opis bez trafienia"),
            json!([]),
        )
        .await;
        insert_chunk(pool, decoy, 0, "card", "Nie ten wiersz", Some(axis(1))).await;

        let (embedder, calls) = fake(axis(0), axis(0));
        let query = Query {
            text: "qxv semantic passage".into(),
            category: None,
        };
        let hits = search(pool, &embedder, &query).await.expect("search");
        assert_eq!(hits.len(), 1, "{hits:?}");
        let hit = &hits[0];
        assert_eq!(hit.title, "Fixture Innovation");
        assert_eq!(hit.page_url, page_url);
        assert_eq!(hit.film_url.as_deref(), Some(film_url));
        assert_eq!(hit.licence, "CC-BY-SA");
        assert_eq!(
            hit.categories,
            vec!["Seniorzy".to_string(), "Sport".to_string()]
        );
        assert_eq!(hit.snippet, "Hala sportowa przy szkole");
        let rendered = format!("{hit:?}");
        assert!(!rendered.contains(tempting), "{rendered}");
        assert!(!rendered.contains(decoy_url), "{rendered}");
        let seen = calls.lock().expect("calls").clone();
        assert_eq!(seen, vec![(Input::Query, vec![query.text])]);
    }

    #[tokio::test]
    async fn cosine_below_minimum_returns_empty() {
        let fix = fixture().await;
        let pool = &fix.pool;
        let id = insert_innovation(
            pool,
            "Warsztat ceramiczny",
            "https://fixture.example/ceramic",
            None,
            Some("Lepienie gliny w pracowni"),
            json!([]),
        )
        .await;
        insert_chunk(pool, id, 0, "card", "Lepienie gliny", Some(angled(0.5))).await;
        let (embedder, _) = fake(axis(0), axis(0));
        let hits = search(
            pool,
            &embedder,
            &Query {
                text: "qxv platypus kiln".into(),
                category: None,
            },
        )
        .await
        .expect("search");
        assert!(hits.is_empty(), "{hits:?}");
    }

    #[tokio::test]
    async fn clear_title_match_returns_despite_weak_cosine() {
        let fix = fixture().await;
        let pool = &fix.pool;
        let id = insert_innovation(
            pool,
            "Boccia Parkowa",
            "https://fixture.example/boccia",
            Some("CC0"),
            Some("Skrót bocci w parku"),
            json!([]),
        )
        .await;
        insert_chunk(
            pool,
            id,
            0,
            "card",
            "chunk should not win",
            Some(angled(0.2)),
        )
        .await;
        let (embedder, _) = fake(axis(0), axis(0));
        let hits = search(
            pool,
            &embedder,
            &Query {
                text: "Boccia Parkowa".into(),
                category: None,
            },
        )
        .await
        .expect("search");
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(hits[0].title, "Boccia Parkowa");
        assert_eq!(hits[0].page_url, "https://fixture.example/boccia");
        assert_eq!(hits[0].snippet, "Skrót bocci w parku");
        assert_eq!(hits[0].licence, "CC0");
        assert_eq!(hits[0].film_url, None);
    }

    #[tokio::test]
    async fn title_match_ranks_above_weaker_vector() {
        let fix = fixture().await;
        let pool = &fix.pool;
        let name_id = insert_innovation(
            pool,
            "Boccia Parkowa",
            "https://fixture.example/boccia",
            None,
            Some("Skrót bocci w parku"),
            json!([]),
        )
        .await;
        insert_chunk(
            pool,
            name_id,
            0,
            "card",
            "chunk should not win",
            Some(angled(0.2)),
        )
        .await;
        let other_id = insert_innovation(
            pool,
            "Inne zajęcia ruchowe",
            "https://fixture.example/inne",
            None,
            Some("Opis innych zajęć"),
            json!([]),
        )
        .await;
        insert_chunk(
            pool,
            other_id,
            0,
            "card",
            "Trafienie wektorowe",
            Some(angled(0.8)),
        )
        .await;
        let (embedder, _) = fake(axis(0), axis(0));
        let hits = search(
            pool,
            &embedder,
            &Query {
                text: "Boccia Parkowa".into(),
                category: None,
            },
        )
        .await
        .expect("search");
        assert_eq!(
            hits.iter()
                .map(|hit| hit.title.as_str())
                .collect::<Vec<_>>(),
            vec!["Boccia Parkowa", "Inne zajęcia ruchowe"]
        );
        assert_eq!(hits[0].snippet, "Skrót bocci w parku");
        assert_eq!(hits[1].snippet, "Trafienie wektorowe");
    }

    #[tokio::test]
    async fn category_filter_limits_hits() {
        let fix = fixture().await;
        let pool = &fix.pool;
        let sport = insert_innovation(
            pool,
            "Aaaaa sportowe",
            "https://fixture.example/sport",
            None,
            Some("Zajęcia sportowe"),
            json!([]),
        )
        .await;
        insert_category(pool, sport, "sport", "Sport").await;
        insert_chunk(pool, sport, 0, "card", "Boisko", Some(axis(0))).await;
        let culture = insert_innovation(
            pool,
            "Bbbbb kulturowe",
            "https://fixture.example/kultura",
            Some("PD"),
            Some("Zajęcia kulturalne"),
            json!([]),
        )
        .await;
        insert_category(pool, culture, "kultura", "Kultura").await;
        insert_chunk(pool, culture, 0, "card", "Scena", Some(axis(0))).await;

        let (embedder, _) = fake(axis(0), axis(0));
        let hits = search(
            pool,
            &embedder,
            &Query {
                text: "qxv motion query".into(),
                category: Some("sport".into()),
            },
        )
        .await
        .expect("search");
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(hits[0].page_url, "https://fixture.example/sport");
        assert_eq!(hits[0].categories, vec!["Sport".to_string()]);
        assert_eq!(hits[0].licence, "");

        let missing = search(
            pool,
            &embedder,
            &Query {
                text: "qxv motion query".into(),
                category: Some("brak".into()),
            },
        )
        .await
        .expect("search");
        assert!(missing.is_empty(), "{missing:?}");
    }

    #[tokio::test]
    async fn trigram_fallback_returns_digest_or_empty() {
        let fix = fixture().await;
        let pool = &fix.pool;
        let digest = "Gra boccia dla seniorów w krakowskim parku miejskim";
        let title = "Innowacja senioralna";
        let id = insert_innovation(
            pool,
            title,
            "https://fixture.example/digest",
            None,
            Some(digest),
            json!([]),
        )
        .await;
        insert_link(pool, id, "video", "https://fixture.example/digest.mp4").await;
        insert_category(pool, id, "sport", "Sport").await;

        let (embedder, calls) = fake(axis(0), axis(0));
        let hits = search(
            pool,
            &embedder,
            &Query {
                text: "Gra boccia dla seniorów w krakowskim parku".into(),
                category: None,
            },
        )
        .await
        .expect("search");
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(hits[0].snippet, digest);
        assert_eq!(hits[0].page_url, "https://fixture.example/digest");
        assert_eq!(
            hits[0].film_url.as_deref(),
            Some("https://fixture.example/digest.mp4")
        );
        assert_eq!(hits[0].categories, vec!["Sport".to_string()]);
        assert_eq!(hits[0].licence, "");
        assert!(calls.lock().expect("calls").is_empty());

        for text in ["parku", "qxv platypus kiln"] {
            let score = trigram_score(pool, text, title, digest).await;
            assert!(
                score < MIN_TRIGRAM,
                "{text} scored {score}, threshold {MIN_TRIGRAM}"
            );
            let hits = search(
                pool,
                &embedder,
                &Query {
                    text: text.into(),
                    category: None,
                },
            )
            .await
            .expect("search");
            assert!(hits.is_empty(), "{text}: {hits:?}");
        }
        assert!(calls.lock().expect("calls").is_empty());
    }

    async fn trigram_score(pool: &PgPool, query: &str, title: &str, digest: &str) -> f64 {
        sqlx::query_scalar(
            r#"
            SELECT GREATEST(
                similarity(lower($1), lower($2)),
                similarity(lower($3), lower($2))
            )::float8
            "#,
        )
        .bind(title)
        .bind(query)
        .bind(digest)
        .fetch_one(pool)
        .await
        .expect("similarity")
    }

    #[tokio::test]
    async fn undigested_row_is_not_embedded() {
        let fix = fixture().await;
        let pool = &fix.pool;
        let undigested = insert_innovation(
            pool,
            "Nieprzetrawione",
            "https://undigested.example/row",
            None,
            None,
            json!([{"heading": "Wstęp", "body": "nie wolno tego embedować"}]),
        )
        .await;
        let (embedder, calls) = fake(axis(0), axis(0));
        let wrote = embed_missing(pool, &embedder, "test").await.expect("embed");
        assert_eq!(wrote, 0);
        assert!(calls.lock().expect("calls").is_empty());

        let digested = insert_innovation(
            pool,
            "Kartka",
            "https://fixture.example/kartka",
            None,
            Some("Treść kartki"),
            json!([]),
        )
        .await;
        insert_chunk(pool, digested, 0, "digest", "Treść kartki", None).await;
        let wrote = embed_missing(pool, &embedder, "test").await.expect("embed");
        assert_eq!(wrote, 2);
        let seen = calls.lock().expect("calls").clone();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].0, Input::Passage);
        assert_eq!(
            seen[0].1,
            vec![
                "Treść kartki".to_string(),
                "Kartka\nTreść kartki".to_string(),
            ]
        );
        assert!(seen
            .iter()
            .all(|(_, texts)| { texts.iter().all(|text| !text.contains("Nieprzetrawione")) }));

        let undigested_chunks: i64 =
            sqlx::query_scalar("SELECT count(*) FROM innovation_chunks WHERE innovation_id = $1")
                .bind(undigested)
                .fetch_one(pool)
                .await
                .expect("count");
        assert_eq!(undigested_chunks, 0);

        let digested_chunks: Vec<(i32, String, String, bool)> = sqlx::query_as(
            r#"
            SELECT ordinal, field, body, embedding IS NOT NULL
            FROM innovation_chunks
            WHERE innovation_id = $1
            ORDER BY ordinal
            "#,
        )
        .bind(digested)
        .fetch_all(pool)
        .await
        .expect("digested chunks");
        assert_eq!(
            digested_chunks,
            vec![
                (0, "digest".into(), "Treść kartki".into(), true),
                (1, "card".into(), "Kartka\nTreść kartki".into(), true),
            ]
        );

        let again = embed_missing(pool, &embedder, "test").await.expect("embed");
        assert_eq!(again, 0);
        assert_eq!(calls.lock().expect("calls").len(), 1);

        let hits = search(
            pool,
            &embedder,
            &Query {
                text: "Nieprzetrawione".into(),
                category: None,
            },
        )
        .await
        .expect("search");
        assert!(hits.iter().all(|hit| hit.title != "Nieprzetrawione"));
        assert!(hits
            .iter()
            .all(|hit| hit.page_url != "https://undigested.example/row"));
    }

    #[tokio::test]
    async fn embed_missing_creates_card_and_section_chunks() {
        let fix = fixture().await;
        let pool = &fix.pool;
        let sections = json!([
            {
                "heading": "1. Na czym polega rozwiązanie?",
                "body": "Opis rozwiązania"
            }
        ]);
        let fresh = insert_innovation(
            pool,
            "Tytuł",
            "https://fixture.example/nowa",
            None,
            Some("Skrót innowacji"),
            sections,
        )
        .await;
        let existing = insert_innovation(
            pool,
            "Istniejąca",
            "https://fixture.example/istniejaca",
            None,
            Some("Ma już chunk"),
            json!([{"heading": "Wstęp", "body": "nie dopisywać"}]),
        )
        .await;
        insert_chunk(pool, existing, 0, "digest", "Zostaw tę treść", None).await;

        let (embedder, calls) = fake(axis(0), axis(1));
        let wrote = embed_missing(pool, &embedder, "test").await.expect("embed");
        assert_eq!(wrote, 5);
        let seen = calls.lock().expect("calls").clone();
        assert_eq!(seen.len(), 2);
        assert!(seen.iter().all(|(input, _)| *input == Input::Passage));
        let mut texts: Vec<String> = seen.into_iter().flat_map(|(_, batch)| batch).collect();
        texts.sort();
        let mut expected = vec![
            "1. Na czym polega rozwiązanie?\nOpis rozwiązania".to_string(),
            "Istniejąca\nMa już chunk".to_string(),
            "Tytuł\nSkrót innowacji".to_string(),
            "Wstęp\nnie dopisywać".to_string(),
            "Zostaw tę treść".to_string(),
        ];
        expected.sort();
        assert_eq!(texts, expected);

        let fresh_rows: Vec<(i32, String, String, bool, Option<String>)> = sqlx::query_as(
            r#"
            SELECT ordinal, field, body, embedding IS NOT NULL, embedding_model
            FROM innovation_chunks
            WHERE innovation_id = $1
            ORDER BY ordinal
            "#,
        )
        .bind(fresh)
        .fetch_all(pool)
        .await
        .expect("fresh chunks");
        assert_eq!(
            fresh_rows,
            vec![
                (
                    0,
                    "card".into(),
                    "Tytuł\nSkrót innowacji".into(),
                    true,
                    Some("test".to_string()),
                ),
                (
                    1,
                    "section".into(),
                    "1. Na czym polega rozwiązanie?\nOpis rozwiązania".into(),
                    true,
                    Some("test".to_string()),
                ),
            ]
        );

        let kept: Vec<(i32, String, String, bool, Option<String>)> = sqlx::query_as(
            r#"
            SELECT ordinal, field, body, embedding IS NOT NULL, embedding_model
            FROM innovation_chunks
            WHERE innovation_id = $1
            ORDER BY ordinal
            "#,
        )
        .bind(existing)
        .fetch_all(pool)
        .await
        .expect("existing chunks");
        assert_eq!(
            kept,
            vec![
                (
                    0,
                    "digest".into(),
                    "Zostaw tę treść".into(),
                    true,
                    Some("test".to_string()),
                ),
                (
                    1,
                    "card".into(),
                    "Istniejąca\nMa już chunk".into(),
                    true,
                    Some("test".to_string()),
                ),
                (
                    2,
                    "section".into(),
                    "Wstęp\nnie dopisywać".into(),
                    true,
                    Some("test".to_string()),
                ),
            ]
        );

        let again = embed_missing(pool, &embedder, "test").await.expect("embed");
        assert_eq!(again, 0);
        assert_eq!(calls.lock().expect("calls").len(), 2);
        let fresh_count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM innovation_chunks WHERE innovation_id = $1")
                .bind(fresh)
                .fetch_one(pool)
                .await
                .expect("fresh count");
        let existing_count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM innovation_chunks WHERE innovation_id = $1")
                .bind(existing)
                .fetch_one(pool)
                .await
                .expect("existing count");
        assert_eq!(fresh_count, 2);
        assert_eq!(existing_count, 3);
    }
}
