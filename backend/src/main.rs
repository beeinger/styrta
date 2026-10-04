mod agent;
mod db;
mod extract;
mod http;
mod ingest;
mod scrape;

use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use styrta::config::Config;

#[derive(Parser)]
#[command(name = "styrta", version)]
struct Cli {
    #[command(subcommand)]
    cmd: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Scrape the ROPS innovation library and digest new or changed archives.
    Ingest(IngestArgs),
}

#[derive(Args)]
struct IngestArgs {
    #[command(subcommand)]
    cmd: Option<IngestCmd>,
}

#[derive(Subcommand)]
enum IngestCmd {
    /// Category pages, titles, and links. Does not download archives.
    Catalog,
    /// Digest innovations whose archive hash or page text changed.
    Digest(DigestArgs),
    /// Counts of catalogued and digested rows.
    Status,
}

#[derive(Args)]
struct DigestArgs {
    /// Innovation slug, for example `bawita`.
    #[arg(long)]
    slug: Option<String>,
    #[arg(long, default_value_t = 2)]
    concurrency: usize,
    #[arg(long)]
    limit: Option<usize>,
    /// Run the agent again even when the archive hash matches.
    #[arg(long)]
    force: bool,
    /// Only rows whose last digest failed and produced no text.
    #[arg(long)]
    failed: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    let cfg = Config::load_ingest()?;
    let pool = db::pool(&cfg).await?;
    db::migrate(&pool).await?;

    match cli.cmd {
        Command::Ingest(args) => match args.cmd {
            Some(IngestCmd::Catalog) => ingest::catalog(&cfg, &pool).await?,
            Some(IngestCmd::Status) => ingest::status(&pool).await?,
            Some(IngestCmd::Digest(d)) => {
                ingest::digest(
                    &cfg,
                    &pool,
                    ingest::opts_from(d.slug, d.concurrency, d.limit, d.force, d.failed),
                )
                .await?;
            }
            None => {
                ingest::catalog(&cfg, &pool).await?;
                ingest::digest(&cfg, &pool, ingest::opts_from(None, 2, None, false, false)).await?;
            }
        },
    }
    Ok(())
}
