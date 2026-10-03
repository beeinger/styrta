use std::path::PathBuf;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use reqwest::Url;

#[derive(Clone)]
pub struct Config {
    pub llm_base_url: String,
    pub llm_api_key: String,
    pub llm_model: String,
    pub llm_timeout: Duration,
    pub llm_user_agent: String,
    pub speech_base_url: String,
    pub speech_api_key: String,
    pub stt_model: String,
    pub tts_model: String,
    pub tts_voice: String,
    pub embed_base_url: String,
    pub embed_api_key: String,
    pub embed_model: String,
    pub embed_query_prefix: String,
    pub embed_passage_prefix: String,
    pub jwt_secret: String,
    pub database_url: String,
    pub audio_dir: PathBuf,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("llm_base_url", &self.llm_base_url)
            .field("llm_api_key", &redacted(&self.llm_api_key))
            .field("llm_model", &self.llm_model)
            .field("llm_timeout", &self.llm_timeout)
            .field("llm_user_agent", &self.llm_user_agent)
            .field("speech_base_url", &self.speech_base_url)
            .field("speech_api_key", &redacted(&self.speech_api_key))
            .field("stt_model", &self.stt_model)
            .field("tts_model", &self.tts_model)
            .field("tts_voice", &self.tts_voice)
            .field("embed_base_url", &self.embed_base_url)
            .field("embed_api_key", &redacted(&self.embed_api_key))
            .field("embed_model", &self.embed_model)
            .field("embed_query_prefix", &self.embed_query_prefix)
            .field("embed_passage_prefix", &self.embed_passage_prefix)
            .field("jwt_secret", &"<redacted>")
            .field("database_url", &"<redacted>")
            .field("audio_dir", &self.audio_dir)
            .finish()
    }
}

fn redacted(secret: &str) -> &'static str {
    if secret.is_empty() {
        ""
    } else {
        "<redacted>"
    }
}

impl Config {
    pub fn load() -> Result<Self> {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    /// Database and embed settings are required. Chat, speech, and the JWT secret
    /// may be absent: this command does not sign tokens or call those endpoints.
    pub fn load_embed_job() -> Result<Self> {
        Self::from_lookup(|key| {
            if let Ok(value) = std::env::var(key) {
                return Some(value);
            }
            match key {
                "OPENAI_BASE_URL" | "SPEECH_BASE_URL" => Some("http://127.0.0.1:9/v1".to_string()),
                "OPENAI_API_KEY" | "OPENAI_MODEL" | "STT_MODEL" | "TTS_MODEL" | "TTS_VOICE"
                | "STYRTA_JWT_SECRET" => Some("unused".to_string()),
                _ => None,
            }
        })
    }

    pub(crate) fn from_lookup(mut lookup: impl FnMut(&str) -> Option<String>) -> Result<Self> {
        let llm_base_url = base_url("OPENAI_BASE_URL", &require(&mut lookup, "OPENAI_BASE_URL")?)?;
        let llm_api_key = require(&mut lookup, "OPENAI_API_KEY")?;
        let llm_model = require(&mut lookup, "OPENAI_MODEL")?;
        let llm_timeout = timeout(&mut lookup)?;
        let llm_user_agent = optional(&mut lookup, "OPENAI_USER_AGENT");

        let speech_base_url =
            base_url("SPEECH_BASE_URL", &require(&mut lookup, "SPEECH_BASE_URL")?)?;
        let speech_api_key = optional(&mut lookup, "SPEECH_API_KEY");
        let stt_model = require(&mut lookup, "STT_MODEL")?;
        let tts_model = require(&mut lookup, "TTS_MODEL")?;
        let tts_voice = require(&mut lookup, "TTS_VOICE")?;

        let embed_base_url = base_url("EMBED_BASE_URL", &require(&mut lookup, "EMBED_BASE_URL")?)?;
        let embed_api_key = optional(&mut lookup, "EMBED_API_KEY");
        let embed_model = require(&mut lookup, "EMBED_MODEL")?;
        let embed_query_prefix = require_set(&mut lookup, "EMBED_QUERY_PREFIX")?;
        let embed_passage_prefix = require_set(&mut lookup, "EMBED_PASSAGE_PREFIX")?;

        let jwt_secret = require(&mut lookup, "STYRTA_JWT_SECRET")?;
        let database_url = require(&mut lookup, "DATABASE_URL")?;
        let audio_dir = audio_dir(&mut lookup)?;

        Ok(Self {
            llm_base_url,
            llm_api_key,
            llm_model,
            llm_timeout,
            llm_user_agent,
            speech_base_url,
            speech_api_key,
            stt_model,
            tts_model,
            tts_voice,
            embed_base_url,
            embed_api_key,
            embed_model,
            embed_query_prefix,
            embed_passage_prefix,
            jwt_secret,
            database_url,
            audio_dir,
        })
    }
}

fn require(lookup: &mut impl FnMut(&str) -> Option<String>, name: &str) -> Result<String> {
    match lookup(name) {
        None => bail!("{name} is not set"),
        Some(value) => {
            let value = value.trim().to_string();
            if value.is_empty() {
                bail!("{name} is empty");
            }
            Ok(value)
        }
    }
}

fn require_set(lookup: &mut impl FnMut(&str) -> Option<String>, name: &str) -> Result<String> {
    match lookup(name) {
        None => bail!("{name} is not set"),
        Some(value) => Ok(value.trim().to_string()),
    }
}

fn optional(lookup: &mut impl FnMut(&str) -> Option<String>, name: &str) -> String {
    lookup(name).unwrap_or_default().trim().to_string()
}

fn timeout(lookup: &mut impl FnMut(&str) -> Option<String>) -> Result<Duration> {
    match lookup("OPENAI_TIMEOUT_SECS") {
        None => Ok(Duration::from_secs(120)),
        Some(raw) => {
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

fn audio_dir(lookup: &mut impl FnMut(&str) -> Option<String>) -> Result<PathBuf> {
    match lookup("STYRTA_AUDIO_DIR") {
        None => Ok(PathBuf::from("/var/lib/styrta/audio")),
        Some(value) => {
            let value = value.trim();
            if value.is_empty() {
                bail!("STYRTA_AUDIO_DIR is empty");
            }
            Ok(PathBuf::from(value))
        }
    }
}

fn base_url(name: &str, value: &str) -> Result<String> {
    let trimmed = value.trim().trim_end_matches('/').to_string();
    let url = Url::parse(&trimmed).with_context(|| format!("{name} is not a url"))?;
    if url.scheme() != "http" && url.scheme() != "https" {
        bail!("{name} is not a url");
    }
    if url.host_str().is_none() {
        bail!("{name} is not a url");
    }
    if !url.username().is_empty() || url.password().is_some() {
        bail!("{name} must not include credentials");
    }
    if url.query().is_some() || url.fragment().is_some() {
        bail!("{name} must not include a query or fragment");
    }
    Ok(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn complete() -> HashMap<String, String> {
        HashMap::from([
            ("OPENAI_BASE_URL".into(), "http://127.0.0.1:9/v1/".into()),
            ("OPENAI_API_KEY".into(), "llm-secret-value".into()),
            ("OPENAI_MODEL".into(), "chat-model".into()),
            ("SPEECH_BASE_URL".into(), "http://127.0.0.1:9/speech".into()),
            ("STT_MODEL".into(), "stt".into()),
            ("TTS_MODEL".into(), "tts".into()),
            ("TTS_VOICE".into(), "voice".into()),
            ("EMBED_BASE_URL".into(), "http://127.0.0.1:9/embed".into()),
            ("EMBED_MODEL".into(), "embed".into()),
            ("EMBED_QUERY_PREFIX".into(), "query: ".into()),
            ("EMBED_PASSAGE_PREFIX".into(), "passage: ".into()),
            ("STYRTA_JWT_SECRET".into(), "jwt-secret-value".into()),
            (
                "DATABASE_URL".into(),
                "postgres://styrta:password@127.0.0.1:5432/styrta".into(),
            ),
        ])
    }

    fn load(map: &HashMap<String, String>) -> Result<Config> {
        const ALLOWED: &[&str] = &[
            "OPENAI_BASE_URL",
            "OPENAI_API_KEY",
            "OPENAI_MODEL",
            "OPENAI_TIMEOUT_SECS",
            "OPENAI_USER_AGENT",
            "SPEECH_BASE_URL",
            "SPEECH_API_KEY",
            "STT_MODEL",
            "TTS_MODEL",
            "TTS_VOICE",
            "EMBED_BASE_URL",
            "EMBED_API_KEY",
            "EMBED_MODEL",
            "EMBED_QUERY_PREFIX",
            "EMBED_PASSAGE_PREFIX",
            "STYRTA_JWT_SECRET",
            "DATABASE_URL",
            "STYRTA_AUDIO_DIR",
        ];
        Config::from_lookup(|key| {
            assert!(ALLOWED.contains(&key), "config read {key}");
            map.get(key).cloned()
        })
    }

    #[test]
    fn missing_required_names_the_variable() {
        for name in [
            "OPENAI_BASE_URL",
            "OPENAI_API_KEY",
            "OPENAI_MODEL",
            "SPEECH_BASE_URL",
            "STT_MODEL",
            "TTS_MODEL",
            "TTS_VOICE",
            "EMBED_BASE_URL",
            "EMBED_MODEL",
            "EMBED_QUERY_PREFIX",
            "EMBED_PASSAGE_PREFIX",
            "STYRTA_JWT_SECRET",
            "DATABASE_URL",
        ] {
            let mut map = complete();
            map.remove(name);
            let err = load(&map).unwrap_err();
            let msg = err.to_string();
            assert!(msg.contains(name), "{name}: {msg}");
            assert!(msg.contains("is not set"), "{name}: {msg}");
        }
    }

    #[test]
    fn empty_required_names_the_variable() {
        let mut map = complete();
        map.insert("OPENAI_API_KEY".into(), "   ".into());
        let msg = load(&map).unwrap_err().to_string();
        assert!(msg.contains("OPENAI_API_KEY"), "{msg}");
        assert!(msg.contains("is empty"), "{msg}");
    }

    #[test]
    fn trims_base_urls_and_applies_defaults() {
        let cfg = load(&complete()).unwrap();
        assert_eq!(cfg.llm_base_url, "http://127.0.0.1:9/v1");
        assert_eq!(cfg.llm_timeout, Duration::from_secs(120));
        assert_eq!(cfg.audio_dir, PathBuf::from("/var/lib/styrta/audio"));
        assert_eq!(cfg.speech_api_key, "");
        assert_eq!(cfg.embed_api_key, "");
        let debug = format!("{cfg:?}");
        assert!(!debug.contains("llm-secret-value"));
        assert!(!debug.contains("jwt-secret-value"));
        assert!(!debug.contains("password"));
    }

    #[test]
    fn user_agent_missing_empty_or_set() {
        let missing = load(&complete()).unwrap();
        assert_eq!(missing.llm_user_agent, "");

        let mut map = complete();
        map.insert("OPENAI_USER_AGENT".into(), "   ".into());
        let empty = load(&map).unwrap();
        assert_eq!(empty.llm_user_agent, "");

        map.insert("OPENAI_USER_AGENT".into(), " test-agent/1 ".into());
        let set = load(&map).unwrap();
        assert_eq!(set.llm_user_agent, "test-agent/1");
        let debug = format!("{set:?}");
        assert!(debug.contains("test-agent/1"), "{debug}");
        assert!(!debug.contains("llm-secret-value"), "{debug}");
    }

    #[test]
    fn empty_speech_and_embed_keys_and_prefixes_are_kept() {
        let mut map = complete();
        map.insert("SPEECH_API_KEY".into(), "".into());
        map.insert("EMBED_API_KEY".into(), "  ".into());
        map.insert("EMBED_QUERY_PREFIX".into(), "".into());
        map.insert("EMBED_PASSAGE_PREFIX".into(), "".into());
        let cfg = load(&map).unwrap();
        assert_eq!(cfg.speech_api_key, "");
        assert_eq!(cfg.embed_api_key, "");
        assert_eq!(cfg.embed_query_prefix, "");
        assert_eq!(cfg.embed_passage_prefix, "");
    }

    #[test]
    fn timeout_override_and_rejects_garbage() {
        let mut map = complete();
        map.insert("OPENAI_TIMEOUT_SECS".into(), "15".into());
        map.insert("STYRTA_AUDIO_DIR".into(), "/tmp/styrta-audio".into());
        let cfg = load(&map).unwrap();
        assert_eq!(cfg.llm_timeout, Duration::from_secs(15));
        assert_eq!(cfg.audio_dir, PathBuf::from("/tmp/styrta-audio"));

        map.insert("OPENAI_TIMEOUT_SECS".into(), "nope".into());
        let msg = load(&map).unwrap_err().to_string();
        assert!(msg.contains("OPENAI_TIMEOUT_SECS"), "{msg}");
    }

    #[test]
    fn base_url_rejects_secrets_without_echoing_them() {
        let mut map = complete();
        let secret = "query-secret-value";
        map.insert(
            "OPENAI_BASE_URL".into(),
            format!("http://user:{secret}@127.0.0.1:9/v1"),
        );
        let msg = load(&map).unwrap_err().to_string();
        assert!(msg.contains("OPENAI_BASE_URL"), "{msg}");
        assert!(!msg.contains(secret), "{msg}");

        map.insert(
            "OPENAI_BASE_URL".into(),
            format!("http://127.0.0.1:9/v1?api_key={secret}"),
        );
        let msg = load(&map).unwrap_err().to_string();
        assert!(msg.contains("OPENAI_BASE_URL"), "{msg}");
        assert!(!msg.contains(secret), "{msg}");
    }
}
