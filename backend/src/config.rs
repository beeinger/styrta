use std::time::Duration;

use anyhow::{bail, Context, Result};

#[derive(Clone)]
pub struct Config {
    pub database_url: String,
    pub llm_base_url: String,
    pub llm_api_key: String,
    pub llm_model: String,
    pub llm_timeout: Duration,
    pub llm_user_agent: String,
    pub work_dir: String,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("database_url", &"<redacted>")
            .field("llm_base_url", &self.llm_base_url)
            .field("llm_api_key", &"<redacted>")
            .field("llm_model", &self.llm_model)
            .field("llm_timeout", &self.llm_timeout)
            .field("llm_user_agent", &self.llm_user_agent)
            .field("work_dir", &self.work_dir)
            .finish()
    }
}

impl Config {
    pub fn load() -> Result<Self> {
        let database_url = std::env::var("DATABASE_URL").context("DATABASE_URL is not set")?;
        let llm_base_url = required("OPENAI_BASE_URL")?;
        let llm_base_url = llm_base_url.trim_end_matches('/').to_string();
        if llm_base_url.is_empty() {
            bail!("OPENAI_BASE_URL is empty");
        }
        let llm_api_key = required("OPENAI_API_KEY")?;
        let llm_model = required("OPENAI_MODEL")?;
        let llm_timeout = timeout()?;
        let llm_user_agent = std::env::var("OPENAI_USER_AGENT")
            .unwrap_or_default()
            .trim()
            .to_string();
        let work_dir =
            std::env::var("STYRTA_WORK_DIR").unwrap_or_else(|_| "/tmp/styrta-ingest".to_string());
        Ok(Self {
            database_url,
            llm_base_url,
            llm_api_key,
            llm_model,
            llm_timeout,
            llm_user_agent,
            work_dir,
        })
    }
}

fn required(name: &str) -> Result<String> {
    let value = std::env::var(name).with_context(|| format!("{name} is not set"))?;
    let value = value.trim().to_string();
    if value.is_empty() {
        bail!("{name} is empty");
    }
    Ok(value)
}

fn timeout() -> Result<Duration> {
    match std::env::var("OPENAI_TIMEOUT_SECS") {
        Err(_) => Ok(Duration::from_secs(120)),
        Ok(raw) => {
            let raw = raw.trim();
            if raw.is_empty() {
                bail!("OPENAI_TIMEOUT_SECS is empty");
            }
            let secs: u64 = raw
                .parse()
                .with_context(|| "OPENAI_TIMEOUT_SECS is not an integer")?;
            if secs == 0 {
                bail!("OPENAI_TIMEOUT_SECS must be greater than zero");
            }
            Ok(Duration::from_secs(secs))
        }
    }
}
