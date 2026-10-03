use std::time::Duration;

use anyhow::{bail, Context, Result};
use reqwest::{Method, RequestBuilder, Url};

/// HTTP helper for one base URL and its own bearer key.
#[derive(Clone)]
pub struct Endpoint {
    http: reqwest::Client,
    base: String,
    key: String,
    user_agent: String,
}

impl std::fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Endpoint")
            .field("base", &scrub_text(&self.base, &self.key))
            .field("has_key", &!self.key.is_empty())
            .finish()
    }
}

impl Endpoint {
    pub fn new(base_url: &str, api_key: &str, timeout: Duration, user_agent: &str) -> Result<Self> {
        if timeout.is_zero() {
            bail!("timeout is zero");
        }
        let base = base_url.trim().trim_end_matches('/').to_string();
        Url::parse(&base).context("base url")?;
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .context("http client")?;
        Ok(Self {
            http,
            base,
            key: api_key.to_string(),
            user_agent: user_agent.to_string(),
        })
    }

    pub fn url(&self, path: &str) -> Result<String> {
        join(&self.base, path)
    }

    pub fn post(&self, path: &str) -> Result<RequestBuilder> {
        self.request(Method::POST, path)
    }

    pub fn request(&self, method: Method, path: &str) -> Result<RequestBuilder> {
        let url = self.url(path)?;
        let mut builder = self.http.request(method, &url);
        if !self.user_agent.is_empty() {
            builder = builder.header(reqwest::header::USER_AGENT, self.user_agent.as_str());
        }
        Ok(authorize(builder, &self.key))
    }

    pub(crate) fn scrub(&self, text: &str) -> String {
        scrub_text(text, &self.key)
    }
}

fn authorize(builder: RequestBuilder, key: &str) -> RequestBuilder {
    if key.is_empty() {
        builder
    } else {
        builder.bearer_auth(key)
    }
}

pub(crate) fn join(base: &str, path: &str) -> Result<String> {
    let base = base.trim().trim_end_matches('/');
    if base.is_empty() {
        bail!("base url is empty");
    }
    let path = path.trim().trim_start_matches('/');
    if path.contains("://") || path.starts_with("//") {
        bail!("path must be relative");
    }
    let joined = if path.is_empty() {
        base.to_string()
    } else {
        format!("{base}/{path}")
    };
    Url::parse(&joined).context("url")?;
    Ok(joined)
}

/// Drop query strings and any copy of `key` before a string is logged.
pub(crate) fn scrub_text(text: &str, key: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(idx) = rest.find('?') {
        out.push_str(&rest[..idx]);
        rest = &rest[idx + 1..];
        let skip = rest.find(query_end).unwrap_or(rest.len());
        rest = &rest[skip..];
    }
    out.push_str(rest);
    if !key.is_empty() {
        out = out.replace(key, "<redacted>");
    }
    out
}

fn query_end(c: char) -> bool {
    c.is_whitespace() || matches!(c, ')' | '"' | '<' | '>')
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{AUTHORIZATION, USER_AGENT};

    fn endpoint(key: &str) -> Endpoint {
        Endpoint::new("http://127.0.0.1:9/v1/", key, Duration::from_secs(5), "").unwrap()
    }

    #[test]
    fn joins_path_and_sets_only_this_key() {
        let chat = endpoint("chat-key-value");
        let speech = endpoint("");
        assert_eq!(
            chat.url("chat/completions").unwrap(),
            "http://127.0.0.1:9/v1/chat/completions"
        );
        assert_eq!(
            chat.url("/audio/speech").unwrap(),
            "http://127.0.0.1:9/v1/audio/speech"
        );

        let chat_req = chat.post("chat/completions").unwrap().build().unwrap();
        let speech_req = speech
            .post("audio/transcriptions")
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(
            chat_req.headers().get(AUTHORIZATION).unwrap(),
            "Bearer chat-key-value"
        );
        assert!(speech_req.headers().get(AUTHORIZATION).is_none());
        assert!(chat_req.headers().get(USER_AGENT).is_none());
        assert!(speech_req.headers().get(USER_AGENT).is_none());
        assert!(!format!("{chat:?}").contains("chat-key-value"));
        assert!(!format!("{speech:?}").contains("chat-key-value"));
    }

    #[test]
    fn user_agent_header_is_set_only_when_present() {
        let marked = Endpoint::new(
            "http://127.0.0.1:9/v1/",
            "",
            Duration::from_secs(5),
            "test-agent/1",
        )
        .unwrap();
        let req = marked.post("chat/completions").unwrap().build().unwrap();
        assert_eq!(req.headers().get(USER_AGENT).unwrap(), "test-agent/1");
        assert!(req.headers().get(AUTHORIZATION).is_none());
    }

    #[test]
    fn scrub_removes_query_and_key() {
        let key = "super-secret-key";
        let text = format!("failed for url (http://127.0.0.1/v1/chat?api_key={key}) body {key}");
        let cleaned = scrub_text(&text, key);
        assert!(!cleaned.contains(key), "{cleaned}");
        assert!(!cleaned.contains("api_key"), "{cleaned}");
        assert!(cleaned.contains("http://127.0.0.1/v1/chat"), "{cleaned}");
    }
}
