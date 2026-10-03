use crate::config::Config;
use crate::extract::Extracted;
use crate::scrape::{self, Card, Link, Section};
use anyhow::{Context, Result};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, sqlx::FromRow)]
pub struct InnovationRow {
    pub id: Uuid,
    pub slug: String,
    pub title: String,
    pub page_url: String,
    pub zip_sha256: Option<String>,
    pub zip_etag: Option<String>,
    pub page_sha256: Option<String>,
    pub agent_version: Option<i32>,
    pub digested: bool,
}

pub async fn pool(cfg: &Config) -> Result<PgPool> {
    let pool = PgPool::connect(&cfg.database_url).await?;
    Ok(pool)
}

pub async fn migrate(pool: &PgPool) -> Result<()> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version text PRIMARY KEY,
            applied_at timestamptz NOT NULL DEFAULT now()
        )",
    )
    .execute(pool)
    .await?;
    let version = "001_init";
    let applied: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM schema_migrations WHERE version = $1)")
            .bind(version)
            .fetch_one(pool)
            .await?;
    if applied {
        return Ok(());
    }
    let mut tx = pool.begin().await?;
    for statement in include_str!("../migrations/001_init.sql").split(';') {
        let statement = statement.trim();
        if statement.is_empty() {
            continue;
        }
        sqlx::query(statement)
            .execute(&mut *tx)
            .await
            .with_context(|| {
                format!(
                    "migration statement: {}",
                    statement.chars().take(120).collect::<String>()
                )
            })?;
    }
    sqlx::query("INSERT INTO schema_migrations (version) VALUES ($1)")
        .bind(version)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn upsert_card(
    pool: &PgPool,
    card: &Card,
    categories: &[(String, String)],
) -> Result<Uuid> {
    let zip_url = card
        .links
        .iter()
        .find(|l| l.kind == "zip")
        .map(|l| l.url.clone());
    let id: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO innovations (source_key, slug, title, title_key, page_url, card_summary, zip_url, last_seen_at)
        VALUES ('rops', $1, $2, $3, $4, $5, $6, now())
        ON CONFLICT (source_key, slug) DO UPDATE SET
            title = EXCLUDED.title,
            title_key = EXCLUDED.title_key,
            page_url = EXCLUDED.page_url,
            card_summary = EXCLUDED.card_summary,
            zip_url = COALESCE(EXCLUDED.zip_url, innovations.zip_url),
            last_seen_at = now()
        RETURNING id
        "#,
    )
    .bind(&card.slug)
    .bind(&card.title)
    .bind(scrape::title_key(&card.title))
    .bind(&card.page_url)
    .bind(&card.summary)
    .bind(&zip_url)
    .fetch_one(pool)
    .await?;

    for (slug, name) in categories {
        sqlx::query(
            r#"
            INSERT INTO innovation_categories (innovation_id, slug, name)
            VALUES ($1, $2, $3)
            ON CONFLICT DO NOTHING
            "#,
        )
        .bind(id)
        .bind(slug)
        .bind(name)
        .execute(pool)
        .await?;
    }
    replace_links(pool, id, &card.links).await?;
    Ok(id)
}

pub async fn save_page(
    pool: &PgPool,
    id: Uuid,
    title: &str,
    licence: Option<&str>,
    zip_url: Option<&str>,
    sections: &[Section],
    intro: &str,
    links: &[Link],
    page_sha256: &str,
) -> Result<()> {
    let body = sections_json(intro, sections);
    sqlx::query(
        r#"
        UPDATE innovations SET
            title = $2,
            title_key = $3,
            licence = $4,
            zip_url = COALESCE($5, zip_url),
            page_sections = $6,
            page_sha256 = $7,
            last_seen_at = now()
        WHERE id = $1
        "#,
    )
    .bind(id)
    .bind(title)
    .bind(scrape::title_key(title))
    .bind(licence)
    .bind(zip_url)
    .bind(body)
    .bind(page_sha256)
    .execute(pool)
    .await?;
    replace_links(pool, id, links).await?;
    Ok(())
}

pub async fn list_innovations(
    pool: &PgPool,
    slug: Option<&str>,
    failed_only: bool,
) -> Result<Vec<InnovationRow>> {
    let rows = sqlx::query_as::<_, InnovationRow>(
        r#"
        SELECT id, slug, title, page_url,
               zip_sha256, zip_etag, page_sha256, agent_version,
               digest_text IS NOT NULL AS digested
        FROM innovations
        WHERE source_key = 'rops'
          AND ($1::text IS NULL OR slug = $1 OR slug LIKE '%,' || $1)
          AND ($2::bool = false OR (status = 'failed' AND digest_text IS NULL))
        ORDER BY slug
        "#,
    )
    .bind(slug)
    .bind(failed_only)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn categories(pool: &PgPool, id: Uuid) -> Result<Vec<String>> {
    let names: Vec<(String,)> = sqlx::query_as(
        "SELECT name FROM innovation_categories WHERE innovation_id = $1 ORDER BY name",
    )
    .bind(id)
    .fetch_all(pool)
    .await?;
    Ok(names.into_iter().map(|row| row.0).collect())
}

pub struct SavedDigest<'a> {
    pub text: &'a str,
    pub json: &'a Value,
    pub agent_version: i32,
    pub model: &'a str,
    pub zip_sha256: Option<&'a str>,
    pub zip_bytes: Option<i64>,
    pub zip_etag: Option<&'a str>,
    pub zip_last_modified: Option<&'a str>,
}

pub async fn save_digest(
    pool: &PgPool,
    id: Uuid,
    saved: SavedDigest<'_>,
    files: &[crate::extract::Entry],
    extracted: &std::collections::HashMap<String, Extracted>,
) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        r#"
        UPDATE innovations SET
            digest_text = $2,
            digest_json = $3,
            agent_version = $4,
            status = 'digested',
            error = NULL,
            zip_sha256 = COALESCE($5, zip_sha256),
            zip_bytes = COALESCE($6, zip_bytes),
            zip_etag = COALESCE($7, zip_etag),
            zip_last_modified = COALESCE($8, zip_last_modified),
            digested_at = now()
        WHERE id = $1
        "#,
    )
    .bind(id)
    .bind(saved.text)
    .bind(saved.json)
    .bind(saved.agent_version)
    .bind(saved.zip_sha256)
    .bind(saved.zip_bytes)
    .bind(saved.zip_etag)
    .bind(saved.zip_last_modified)
    .execute(&mut *tx)
    .await?;

    sqlx::query("DELETE FROM innovation_files WHERE innovation_id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    for entry in files.iter().filter(|e| e.kind.readable()) {
        let got = extracted.get(&entry.path);
        sqlx::query(
            r#"
            INSERT INTO innovation_files (innovation_id, path, byte_len, sha256, media_kind, extracted_text, extract_note)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            "#,
        )
        .bind(id)
        .bind(&entry.path)
        .bind(entry.byte_len as i64)
        .bind(got.map(|g| g.sha256.as_str()))
        .bind(entry.kind.as_str())
        .bind(got.map(|g| g.text.as_str()))
        .bind(got.map(|g| g.note.as_str()))
        .execute(&mut *tx)
        .await?;
    }

    sqlx::query("DELETE FROM innovation_chunks WHERE innovation_id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        r#"
        INSERT INTO innovation_chunks (innovation_id, ordinal, field, body, embedding_model)
        VALUES ($1, 0, 'digest', $2, $3)
        "#,
    )
    .bind(id)
    .bind(saved.text)
    .bind(saved.model)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(())
}

pub async fn mark_failed(pool: &PgPool, id: Uuid, error: &str) -> Result<()> {
    let error: String = error.chars().take(2000).collect();
    sqlx::query(
        r#"
        UPDATE innovations SET
            error = $2,
            status = CASE WHEN digest_text IS NULL THEN 'failed' ELSE status END
        WHERE id = $1
        "#,
    )
    .bind(id)
    .bind(error)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn counts(pool: &PgPool) -> Result<Value> {
    let row: (i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
            count(*)::bigint,
            count(*) FILTER (WHERE digest_text IS NOT NULL)::bigint,
            count(*) FILTER (WHERE status = 'failed' AND digest_text IS NULL)::bigint,
            count(*) FILTER (WHERE zip_url IS NOT NULL)::bigint
        FROM innovations
        WHERE source_key = 'rops'
        "#,
    )
    .fetch_one(pool)
    .await?;
    Ok(json!({
        "innovations": row.0,
        "digested": row.1,
        "failed": row.2,
        "with_zip": row.3,
    }))
}

async fn replace_links(pool: &PgPool, id: Uuid, links: &[Link]) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM innovation_links WHERE innovation_id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    for link in links {
        sqlx::query(
            "INSERT INTO innovation_links (innovation_id, kind, url, label) VALUES ($1, $2, $3, $4)",
        )
        .bind(id)
        .bind(link.kind)
        .bind(&link.url)
        .bind(&link.label)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

fn sections_json(intro: &str, sections: &[Section]) -> Value {
    let mut items = Vec::new();
    if !intro.trim().is_empty() {
        items.push(json!({"heading": "Wstęp", "body": intro.trim()}));
    }
    for section in sections {
        items.push(json!({"heading": section.heading, "body": section.body}));
    }
    Value::Array(items)
}
