use crate::agent::{self, ArchiveSession};
use crate::config::Config;
use crate::db::{self, SavedDigest};
use crate::http::Client;
use crate::llm::Llm;
use crate::scrape::{self, Card, Detail};
use anyhow::{Context, Result, bail};
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Semaphore;
use uuid::Uuid;

pub struct DigestOpts {
    pub slug: Option<String>,
    pub concurrency: usize,
    pub limit: Option<usize>,
    pub force: bool,
    pub failed_only: bool,
}

pub fn opts_from(
    slug: Option<String>,
    concurrency: usize,
    limit: Option<usize>,
    force: bool,
    failed_only: bool,
) -> DigestOpts {
    DigestOpts {
        slug,
        concurrency: concurrency.max(1),
        limit,
        force,
        failed_only,
    }
}

pub async fn status(pool: &PgPool) -> Result<()> {
    let counts = db::counts(pool).await?;
    println!("{counts}");
    Ok(())
}

pub async fn catalog(_cfg: &Config, pool: &PgPool) -> Result<()> {
    let http = Client::new()?;
    let mut grouped: BTreeMap<String, (Card, Vec<(String, String)>)> = BTreeMap::new();
    for (slug, name) in scrape::CATEGORIES {
        let url = scrape::category_url(slug);
        tracing::info!(category = slug, "fetch category");
        let html = http.get_text(&url).await.with_context(|| url.clone())?;
        let cards = scrape::parse_category_page(slug, &html);
        tracing::info!(category = slug, cards = cards.len(), "parsed category");
        if cards.is_empty() {
            bail!("category {slug} returned no innovations");
        }
        for card in cards {
            grouped
                .entry(card.slug.clone())
                .and_modify(|(existing, cats)| {
                    cats.push(((*slug).to_string(), (*name).to_string()));
                    if existing.summary.len() < card.summary.len() {
                        existing.summary = card.summary.clone();
                    }
                    for link in &card.links {
                        if !existing.links.iter().any(|l| l.url == link.url) {
                            existing.links.push(link.clone());
                        }
                    }
                })
                .or_insert((card, vec![((*slug).to_string(), (*name).to_string())]));
        }
    }
    tracing::info!(innovations = grouped.len(), "catalog unique");
    for (card, categories) in grouped.values() {
        db::upsert_card(pool, card, categories).await?;
    }
    let counts = db::counts(pool).await?;
    tracing::info!(%counts, "catalog stored");
    Ok(())
}

pub async fn digest(cfg: &Config, pool: &PgPool, opts: DigestOpts) -> Result<()> {
    agent::ensure_pdftotext()?;
    let http = Arc::new(Client::new()?);
    let llm = Arc::new(Llm::new(cfg)?);
    let pool = pool.clone();
    let work = PathBuf::from(&cfg.work_dir);
    std::fs::create_dir_all(&work)?;

    let mut rows = db::list_innovations(&pool, opts.slug.as_deref(), opts.failed_only).await?;
    if rows.is_empty() {
        bail!("no innovations in the database; run `styrta ingest catalog` first");
    }
    if let Some(limit) = opts.limit {
        rows.truncate(limit);
    }
    let total = rows.len();
    tracing::info!(total, model = llm.model(), "digest start");

    let sem = Arc::new(Semaphore::new(opts.concurrency));
    let force = opts.force;
    let mut set = tokio::task::JoinSet::new();
    for row in rows {
        let permit = sem.clone();
        let http = http.clone();
        let llm = llm.clone();
        let pool = pool.clone();
        let work = work.clone();
        set.spawn(async move {
            let _permit = permit.acquire_owned().await.expect("semaphore");
            let slug = row.slug.clone();
            let id = row.id;
            let outcome = digest_one(&http, &llm, &pool, &work, row, force).await;
            (slug, id, outcome)
        });
    }

    let mut digested = 0usize;
    let mut skipped = 0usize;
    let mut failed = 0usize;
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok((slug, _id, Ok(Outcome::Digested))) => {
                digested += 1;
                tracing::info!(slug, digested, skipped, failed, total, "digested");
            }
            Ok((slug, _, Ok(Outcome::Skipped))) => {
                skipped += 1;
                tracing::info!(slug, digested, skipped, failed, total, "unchanged, skipped");
            }
            Ok((slug, id, Err(err))) => {
                failed += 1;
                tracing::error!(slug, error = %err, "digest failed");
                db::mark_failed(&pool, id, &format!("{err:#}")).await?;
            }
            Err(err) => {
                failed += 1;
                tracing::error!(error = %err, "digest task panicked");
            }
        }
    }
    tracing::info!(digested, skipped, failed, total, "digest finished");
    if digested == 0 && failed > 0 {
        bail!("{failed} innovations failed and none were digested");
    }
    Ok(())
}

enum Outcome {
    Digested,
    Skipped,
}

async fn digest_one(
    http: &Client,
    llm: &Llm,
    pool: &PgPool,
    work: &PathBuf,
    row: db::InnovationRow,
    force: bool,
) -> Result<Outcome> {
    let html = http.get_text(&row.page_url).await?;
    let mut detail = scrape::parse_detail(&row.page_url, &html);
    if detail.title.is_empty() {
        detail.title = row.title.clone();
    }
    let page_sha = sha256_hex(page_bytes(&detail).as_bytes());
    db::save_page(
        pool,
        row.id,
        &detail.title,
        detail.licence.as_deref(),
        detail.zip_url.as_deref(),
        &detail.sections,
        &detail.intro,
        &detail.links,
        &page_sha,
    )
    .await?;

    let page_same = row.page_sha256.as_deref() == Some(page_sha.as_str());
    let version_same = row.agent_version == Some(agent::AGENT_VERSION);
    let base_fresh = !force && row.digested && page_same && version_same;
    let categories = db::categories(pool, row.id).await?;

    let Some(zip_url) = detail.zip_url.clone() else {
        if base_fresh && row.zip_sha256.is_none() {
            return Ok(Outcome::Skipped);
        }
        return finish(llm, pool, row.id, &detail, &row.page_url, &categories, None, None).await;
    };

    let dir = work.join(row.slug.replace('/', "_"));
    let _scratch = DirGuard::create(&dir)?;
    let zip_path = dir.join("archive.zip");
    let etag = if base_fresh {
        row.zip_etag.as_deref()
    } else {
        None
    };
    let fetched = http.download(&zip_url, &zip_path, etag).await?;
    if fetched.status == reqwest::StatusCode::NOT_MODIFIED {
        return Ok(Outcome::Skipped);
    }
    if fetched.too_large {
        detail.intro = format!(
            "{}\nArchiwum ma {} bajtów i zostaje na stronie ROPS. Notatka jest ze strony.",
            detail.intro, fetched.len
        );
        return finish(llm, pool, row.id, &detail, &row.page_url, &categories, None, None).await;
    }
    if !fetched.status.is_success() {
        detail.intro = format!(
            "{}\nArchiwum nie zostało pobrane (HTTP {}).",
            detail.intro,
            fetched.status
        );
        return finish(llm, pool, row.id, &detail, &row.page_url, &categories, None, None).await;
    }
    digest_zip(
        llm,
        pool,
        &row,
        &mut detail,
        &categories,
        &zip_path,
        &fetched.sha256,
        fetched.len,
        fetched.etag,
        fetched.last_modified,
        base_fresh,
    )
    .await
}

async fn digest_zip(
    llm: &Llm,
    pool: &PgPool,
    row: &db::InnovationRow,
    detail: &mut Detail,
    categories: &[String],
    zip_path: &std::path::Path,
    sha: &str,
    len: u64,
    etag: Option<String>,
    last_modified: Option<String>,
    base_fresh: bool,
) -> Result<Outcome> {
    if base_fresh && row.zip_sha256.as_deref() == Some(sha) {
        return Ok(Outcome::Skipped);
    }
    let scratch = zip_path.with_file_name("scratch");
    std::fs::create_dir_all(&scratch)?;
    let mut session = match ArchiveSession::open(zip_path, &scratch) {
        Ok(session) => session,
        Err(err) => {
            tracing::warn!(slug = row.slug, error = %err, "archive unreadable");
            detail.intro = format!(
                "{}\nArchiwum pobrano, ale nie dało się go otworzyć ({err:#}).",
                detail.intro
            );
            return finish(
                llm,
                pool,
                row.id,
                detail,
                &row.page_url,
                categories,
                Some(sha),
                Some(len as i64),
            )
            .await;
        }
    };
    let docs = session.documents();
    tracing::info!(
        slug = row.slug,
        documents = docs.len(),
        bytes = len,
        "archive indexed"
    );
    let digest = agent::run(llm, detail, &row.page_url, categories, Some(&mut session)).await?;
    tracing::info!(
        slug = row.slug,
        files = digest.files_read.len(),
        caveats = digest.caveats,
        "agent finished"
    );
    let text = agent::compose(detail, &row.page_url, categories, &digest);
    let mut raw = digest.raw.clone();
    if let Some(obj) = raw.as_object_mut() {
        obj.insert("agent_version".into(), json!(agent::AGENT_VERSION));
        obj.insert("model".into(), json!(llm.model()));
        obj.insert("zip_sha256".into(), json!(sha));
    }
    db::save_digest(
        pool,
        row.id,
        SavedDigest {
            text: &text,
            json: &raw,
            agent_version: agent::AGENT_VERSION,
            model: llm.model(),
            zip_sha256: Some(sha),
            zip_bytes: Some(len as i64),
            zip_etag: etag.as_deref(),
            zip_last_modified: last_modified.as_deref(),
        },
        &docs,
        session.cached(),
    )
    .await?;
    Ok(Outcome::Digested)
}

struct DirGuard(PathBuf);

impl DirGuard {
    fn create(path: &std::path::Path) -> Result<Self> {
        std::fs::create_dir_all(path)?;
        Ok(Self(path.to_path_buf()))
    }
}

impl Drop for DirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn finish(
    llm: &Llm,
    pool: &PgPool,
    id: Uuid,
    detail: &Detail,
    page_url: &str,
    categories: &[String],
    zip_sha256: Option<&str>,
    zip_bytes: Option<i64>,
) -> Result<Outcome> {
    let digest = agent::run(llm, detail, page_url, categories, None).await?;
    tracing::info!(files = digest.files_read.len(), caveats = digest.caveats, "agent finished");
    let text = agent::compose(detail, page_url, categories, &digest);
    let mut raw = digest.raw.clone();
    if let Some(obj) = raw.as_object_mut() {
        obj.insert("agent_version".into(), json!(agent::AGENT_VERSION));
        obj.insert("model".into(), json!(llm.model()));
    }
    db::save_digest(
        pool,
        id,
        SavedDigest {
            text: &text,
            json: &raw,
            agent_version: agent::AGENT_VERSION,
            model: llm.model(),
            zip_sha256,
            zip_bytes,
            zip_etag: None,
            zip_last_modified: None,
        },
        &[],
        &std::collections::HashMap::new(),
    )
    .await?;
    Ok(Outcome::Digested)
}

fn page_bytes(detail: &Detail) -> String {
    let mut s = String::new();
    s.push_str(&detail.title);
    s.push('\n');
    s.push_str(&detail.intro);
    for section in &detail.sections {
        s.push('\n');
        s.push_str(&section.heading);
        s.push('\n');
        s.push_str(&section.body);
    }
    s
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
