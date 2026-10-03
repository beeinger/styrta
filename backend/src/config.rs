use anyhow::{bail, Context, Result};

#[derive(Clone, Debug)]
pub struct Config {
    pub database_url: String,
    pub openai_base_url: String,
    pub openai_api_key: String,
    pub openai_model: String,
    pub work_dir: String,
}

impl Config {
    pub fn load() -> Result<Self> {
        let database_url =
            std::env::var("DATABASE_URL").context("DATABASE_URL is not set (see backend/.env)")?;
        let openai_base_url = std::env::var("OPENAI_BASE_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:4000/v1".to_string());
        let openai_api_key = std::env::var("OPENAI_API_KEY")
            .or_else(|_| std::env::var("LITELLM_MASTER_KEY"))
            .context("set OPENAI_API_KEY or LITELLM_MASTER_KEY")?;
        if openai_api_key.is_empty() {
            bail!("OpenAI-compatible API key is empty");
        }
        let openai_model =
            std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "glm-5.3-flash".to_string());
        let work_dir =
            std::env::var("STYRTA_WORK_DIR").unwrap_or_else(|_| "/tmp/styrta-ingest".to_string());
        Ok(Self {
            database_url,
            openai_base_url: openai_base_url.trim_end_matches('/').to_string(),
            openai_api_key,
            openai_model,
            work_dir,
        })
    }
}
