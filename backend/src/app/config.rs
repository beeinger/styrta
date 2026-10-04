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
    pub work_dir: PathBuf,
    pub geocoder_base_url: String,
    pub geocoder_user_agent: String,
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
            .field("work_dir", &self.work_dir)
            .field("geocoder_base_url", &self.geocoder_base_url)
            .field("geocoder_user_agent", &self.geocoder_user_agent)
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

    pub fn load_ingest() -> Result<Self> {
        Self::ingest_from(|key| std::env::var(key).ok())
    }

    /// Database and embed settings only. Chat, speech, and the JWT secret are unused.
    pub fn load_embed_job() -> Result<Self> {
        Self::embed_job_from(|key| std::env::var(key).ok())
    }

    pub(crate) fn from_lookup(mut lookup: impl FnMut(&str) -> Option<String>) -> Result<Self> {
        let chat = chat_settings(&mut lookup)?;
        let speech = speech_settings(&mut lookup)?;
        let embed = embed_settings(&mut lookup)?;
        let jwt_secret = require(&mut lookup, "STYRTA_JWT_SECRET")?;
        let database_url = require(&mut lookup, "DATABASE_URL")?;
        let audio_dir = audio_dir(&mut lookup)?;
        let (geocoder_base_url, geocoder_user_agent) = geocoder_settings(&mut lookup)?;
        Ok(Self {
            llm_base_url: chat.base_url,
            llm_api_key: chat.api_key,
            llm_model: chat.model,
            llm_timeout: chat.timeout,
            llm_user_agent: chat.user_agent,
            speech_base_url: speech.base_url,
            speech_api_key: speech.api_key,
            stt_model: speech.stt_model,
            tts_model: speech.tts_model,
            tts_voice: speech.tts_voice,
            embed_base_url: embed.base_url,
            embed_api_key: embed.api_key,
            embed_model: embed.model,
            embed_query_prefix: embed.query_prefix,
            embed_passage_prefix: embed.passage_prefix,
            jwt_secret,
            database_url,
            audio_dir,
            work_dir: PathBuf::from("/tmp/styrta-ingest"),
            geocoder_base_url,
            geocoder_user_agent,
        })
    }

    fn ingest_from(mut lookup: impl FnMut(&str) -> Option<String>) -> Result<Self> {
        let chat = chat_settings(&mut lookup)?;
        let database_url = require(&mut lookup, "DATABASE_URL")?;
        let work_dir = work_dir(&mut lookup)?;
        Ok(Self {
            llm_base_url: chat.base_url,
            llm_api_key: chat.api_key,
            llm_model: chat.model,
            llm_timeout: chat.timeout,
            llm_user_agent: chat.user_agent,
            speech_base_url: String::new(),
            speech_api_key: String::new(),
            stt_model: String::new(),
            tts_model: String::new(),
            tts_voice: String::new(),
            embed_base_url: String::new(),
            embed_api_key: String::new(),
            embed_model: String::new(),
            embed_query_prefix: String::new(),
            embed_passage_prefix: String::new(),
            jwt_secret: String::new(),
            database_url,
            audio_dir: PathBuf::from("/var/lib/styrta/audio"),
            work_dir,
            geocoder_base_url: String::new(),
            geocoder_user_agent: String::new(),
        })
    }

    fn embed_job_from(mut lookup: impl FnMut(&str) -> Option<String>) -> Result<Self> {
        let embed = embed_settings(&mut lookup)?;
        let database_url = require(&mut lookup, "DATABASE_URL")?;
        Ok(Self {
            llm_base_url: String::new(),
            llm_api_key: String::new(),
            llm_model: String::new(),
            llm_timeout: Duration::from_secs(120),
            llm_user_agent: String::new(),
            speech_base_url: String::new(),
            speech_api_key: String::new(),
            stt_model: String::new(),
            tts_model: String::new(),
            tts_voice: String::new(),
            embed_base_url: embed.base_url,
            embed_api_key: embed.api_key,
            embed_model: embed.model,
            embed_query_prefix: embed.query_prefix,
            embed_passage_prefix: embed.passage_prefix,
            jwt_secret: String::new(),
            database_url,
            audio_dir: PathBuf::from("/var/lib/styrta/audio"),
            work_dir: PathBuf::from("/tmp/styrta-ingest"),
            geocoder_base_url: String::new(),
            geocoder_user_agent: String::new(),
        })
    }
}

const NOMINATIM: &str = "https://nominatim.openstreetmap.org";

fn geocoder_settings(lookup: &mut impl FnMut(&str) -> Option<String>) -> Result<(String, String)> {
    let configured = optional(lookup, "GEOCODER_BASE_URL");
    let base = if configured.is_empty() {
        NOMINATIM.to_string()
    } else {
        base_url("GEOCODER_BASE_URL", &configured)?
    };
    let configured_agent = optional(lookup, "GEOCODER_USER_AGENT");
    let agent = if configured_agent.is_empty() {
        let chat = optional(lookup, "LLM_USER_AGENT");
        if chat.is_empty() {
            "styrta/1.0 (public place search)".to_string()
        } else {
            chat
        }
    } else {
        configured_agent
    };
    Ok((base, agent))
}

struct ChatSettings {
    base_url: String,
    api_key: String,
    model: String,
    timeout: Duration,
    user_agent: String,
}

struct SpeechSettings {
    base_url: String,
    api_key: String,
    stt_model: String,
    tts_model: String,
    tts_voice: String,
}

struct EmbedSettings {
    base_url: String,
    api_key: String,
    model: String,
    query_prefix: String,
    passage_prefix: String,
}

fn chat_settings(lookup: &mut impl FnMut(&str) -> Option<String>) -> Result<ChatSettings> {
    Ok(ChatSettings {
        base_url: base_url("LLM_BASE_URL", &require(lookup, "LLM_BASE_URL")?)?,
        api_key: require(lookup, "LLM_API_KEY")?,
        model: require(lookup, "LLM_MODEL")?,
        timeout: timeout(lookup)?,
        user_agent: optional(lookup, "LLM_USER_AGENT"),
    })
}

fn speech_settings(lookup: &mut impl FnMut(&str) -> Option<String>) -> Result<SpeechSettings> {
    Ok(SpeechSettings {
        base_url: base_url("SPEECH_BASE_URL", &require(lookup, "SPEECH_BASE_URL")?)?,
        api_key: optional(lookup, "SPEECH_API_KEY"),
        stt_model: require(lookup, "STT_MODEL")?,
        tts_model: require(lookup, "TTS_MODEL")?,
        tts_voice: require(lookup, "TTS_VOICE")?,
    })
}

fn embed_settings(lookup: &mut impl FnMut(&str) -> Option<String>) -> Result<EmbedSettings> {
    Ok(EmbedSettings {
        base_url: base_url("EMBED_BASE_URL", &require(lookup, "EMBED_BASE_URL")?)?,
        api_key: optional(lookup, "EMBED_API_KEY"),
        model: require(lookup, "EMBED_MODEL")?,
        query_prefix: require_set(lookup, "EMBED_QUERY_PREFIX")?,
        passage_prefix: require_set(lookup, "EMBED_PASSAGE_PREFIX")?,
    })
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
    match lookup("LLM_TIMEOUT_SECS") {
        None => Ok(Duration::from_secs(120)),
        Some(raw) => {
            let raw = raw.trim();
            if raw.is_empty() {
                bail!("LLM_TIMEOUT_SECS is empty");
            }
            let secs: u64 = raw
                .parse()
                .with_context(|| "LLM_TIMEOUT_SECS is not an integer")?;
            if secs == 0 {
                bail!("LLM_TIMEOUT_SECS must be greater than zero");
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

fn work_dir(lookup: &mut impl FnMut(&str) -> Option<String>) -> Result<PathBuf> {
    match lookup("STYRTA_WORK_DIR") {
        None => Ok(PathBuf::from("/tmp/styrta-ingest")),
        Some(value) => {
            let value = value.trim();
            if value.is_empty() {
                bail!("STYRTA_WORK_DIR is empty");
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
            ("LLM_BASE_URL".into(), "http://127.0.0.1:9/v1/".into()),
            ("LLM_API_KEY".into(), "llm-secret-value".into()),
            ("LLM_MODEL".into(), "chat-model".into()),
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
            "LLM_BASE_URL",
            "LLM_API_KEY",
            "LLM_MODEL",
            "LLM_TIMEOUT_SECS",
            "LLM_USER_AGENT",
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
            "GEOCODER_BASE_URL",
            "GEOCODER_USER_AGENT",
        ];
        Config::from_lookup(|key| {
            assert!(ALLOWED.contains(&key), "config read {key}");
            map.get(key).cloned()
        })
    }

    #[test]
    fn missing_required_names_the_variable() {
        for name in [
            "LLM_BASE_URL",
            "LLM_API_KEY",
            "LLM_MODEL",
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
        map.insert("LLM_API_KEY".into(), "   ".into());
        let msg = load(&map).unwrap_err().to_string();
        assert!(msg.contains("LLM_API_KEY"), "{msg}");
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
    fn geocoder_defaults_to_nominatim_and_can_be_overridden() {
        let cfg = load(&complete()).unwrap();
        assert_eq!(
            cfg.geocoder_base_url,
            "https://nominatim.openstreetmap.org"
        );
        assert_eq!(cfg.geocoder_user_agent, "styrta/1.0 (public place search)");

        let mut map = complete();
        map.insert(
            "GEOCODER_BASE_URL".into(),
            " http://127.0.0.1:9/geo/ ".into(),
        );
        map.insert("GEOCODER_USER_AGENT".into(), " styrta-test/2 ".into());
        let cfg = load(&map).unwrap();
        assert_eq!(cfg.geocoder_base_url, "http://127.0.0.1:9/geo");
        assert_eq!(cfg.geocoder_user_agent, "styrta-test/2");

        map.insert("GEOCODER_USER_AGENT".into(), "".into());
        map.insert("LLM_USER_AGENT".into(), "chat-agent/1".into());
        let cfg = load(&map).unwrap();
        assert_eq!(cfg.geocoder_user_agent, "chat-agent/1");
    }

    #[test]
    fn user_agent_missing_empty_or_set() {
        let missing = load(&complete()).unwrap();
        assert_eq!(missing.llm_user_agent, "");

        let mut map = complete();
        map.insert("LLM_USER_AGENT".into(), "   ".into());
        let empty = load(&map).unwrap();
        assert_eq!(empty.llm_user_agent, "");

        map.insert("LLM_USER_AGENT".into(), " test-agent/1 ".into());
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
        map.insert("LLM_TIMEOUT_SECS".into(), "15".into());
        map.insert("STYRTA_AUDIO_DIR".into(), "/tmp/styrta-audio".into());
        let cfg = load(&map).unwrap();
        assert_eq!(cfg.llm_timeout, Duration::from_secs(15));
        assert_eq!(cfg.audio_dir, PathBuf::from("/tmp/styrta-audio"));

        map.insert("LLM_TIMEOUT_SECS".into(), "nope".into());
        let msg = load(&map).unwrap_err().to_string();
        assert!(msg.contains("LLM_TIMEOUT_SECS"), "{msg}");
    }

    #[test]
    fn base_url_rejects_secrets_without_echoing_them() {
        let mut map = complete();
        let secret = "query-secret-value";
        map.insert(
            "LLM_BASE_URL".into(),
            format!("http://user:{secret}@127.0.0.1:9/v1"),
        );
        let msg = load(&map).unwrap_err().to_string();
        assert!(msg.contains("LLM_BASE_URL"), "{msg}");
        assert!(!msg.contains(secret), "{msg}");

        map.insert(
            "LLM_BASE_URL".into(),
            format!("http://127.0.0.1:9/v1?api_key={secret}"),
        );
        let msg = load(&map).unwrap_err().to_string();
        assert!(msg.contains("LLM_BASE_URL"), "{msg}");
        assert!(!msg.contains(secret), "{msg}");
    }

    #[test]
    fn ingest_reads_chat_database_and_work_dir_only() {
        let mut seen = Vec::new();
        let map: HashMap<String, String> = HashMap::from([
            ("DATABASE_URL".into(), "postgres://127.0.0.1/styrta".into()),
            ("LLM_BASE_URL".into(), "http://127.0.0.1:9/v1".into()),
            ("LLM_API_KEY".into(), "llm-secret-value".into()),
            ("LLM_MODEL".into(), "chat".into()),
            ("STYRTA_WORK_DIR".into(), "/tmp/work".into()),
        ]);
        let cfg = Config::ingest_from(|key| {
            seen.push(key.to_string());
            map.get(key).cloned()
        })
        .unwrap();
        assert!(
            seen.iter().all(|key| key == "DATABASE_URL"
                || key.starts_with("LLM_")
                || key == "STYRTA_WORK_DIR"),
            "{seen:?}"
        );
        assert_eq!(cfg.work_dir, PathBuf::from("/tmp/work"));
        assert_eq!(cfg.embed_model, "");
        assert_eq!(cfg.jwt_secret, "");
        assert!(!format!("{cfg:?}").contains("llm-secret-value"));
    }

    #[test]
    fn embed_job_reads_database_and_embed_only() {
        let mut seen = Vec::new();
        let map: HashMap<String, String> = HashMap::from([
            ("DATABASE_URL".into(), "postgres://127.0.0.1/styrta".into()),
            ("EMBED_BASE_URL".into(), "http://127.0.0.1:9/embed".into()),
            ("EMBED_MODEL".into(), "embed".into()),
            ("EMBED_QUERY_PREFIX".into(), "query: ".into()),
            ("EMBED_PASSAGE_PREFIX".into(), "passage: ".into()),
        ]);
        let cfg = Config::embed_job_from(|key| {
            seen.push(key.to_string());
            map.get(key).cloned()
        })
        .unwrap();
        assert!(
            seen.iter()
                .all(|key| key == "DATABASE_URL" || key.starts_with("EMBED_")),
            "{seen:?}"
        );
        assert_eq!(cfg.embed_model, "embed");
        assert_eq!(cfg.llm_api_key, "");
        assert_eq!(cfg.llm_base_url, "");
        assert_eq!(cfg.jwt_secret, "");
        assert_eq!(cfg.speech_base_url, "");
    }
}
