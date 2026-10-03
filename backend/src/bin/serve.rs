use std::sync::Arc;

use anyhow::{anyhow, Context};
use styrta::api::{self, AppState};
use styrta::appdb::Store;
use styrta::config::Config;
use styrta::embed;
use styrta::llm;
use styrta::speech;

const LISTEN: &str = "0.0.0.0:8088";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    match std::env::args().nth(1).as_deref() {
        None => serve(Config::load()?).await,
        Some("embed") => embed_missing().await,
        Some(command) => Err(anyhow!("unknown command {command}")),
    }
}

async fn serve(config: Config) -> anyhow::Result<()> {
    let store = connect_store(&config).await?;
    let model = llm::Client::new(&config).context("chat client")?;
    let speech = speech::Client::new(&config).context("speech client")?;
    let embedder = embed::Client::new(&config).context("embed client")?;

    let state = AppState {
        config: Arc::new(config),
        store,
        model: Arc::new(model),
        speech: Arc::new(speech),
        embedder: Arc::new(embedder),
    };
    styrta::harness::resume_running(api::harness_services(&state))
        .await
        .context("resume running turns")?;

    let app = api::router(state);
    let listener = tokio::net::TcpListener::bind(LISTEN)
        .await
        .context("bind")?;
    tracing::info!(addr = LISTEN, "listening");
    axum::serve(listener, app).await.context("serve")?;
    Ok(())
}

async fn embed_missing() -> anyhow::Result<()> {
    let config = Config::load_embed_job()?;
    let store = connect_store(&config).await?;
    let embedder = embed::Client::new(&config).context("embed client")?;
    let wrote = styrta::knowledge::embed_missing(store.pool(), &embedder, &config.embed_model)
        .await
        .map_err(|err| anyhow!("{err}"))
        .context("embed missing")?;
    tracing::info!(chunks = wrote, "embedded");
    Ok(())
}

async fn connect_store(config: &Config) -> anyhow::Result<Store> {
    match Store::connect(&config.database_url).await {
        Ok(store) => Ok(store),
        Err(err) => {
            let text = err
                .to_string()
                .replace(&config.database_url, "DATABASE_URL");
            Err(anyhow!(text).context("connect store"))
        }
    }
}
