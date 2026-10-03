use anyhow::{bail, Context, Result};
use encoding_rs::{Encoding, UTF_8};
use reqwest::{header, StatusCode};
use std::time::Duration;

pub struct Client {
    inner: reqwest::Client,
}

pub struct Fetched {
    pub status: StatusCode,
    pub bytes: Vec<u8>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub content_type: Option<String>,
}

impl Client {
    pub fn new() -> Result<Self> {
        let inner = reqwest::Client::builder()
            .user_agent("styrta-ingest/0.1")
            .timeout(Duration::from_secs(300))
            .redirect(reqwest::redirect::Policy::limited(8))
            .gzip(true)
            .build()?;
        Ok(Self { inner })
    }

    pub async fn get_text(&self, url: &str) -> Result<String> {
        let fetched = self.get(url, None, None).await?;
        if !fetched.status.is_success() {
            bail!("{url} returned {}", fetched.status);
        }
        Ok(decode_body(&fetched.bytes, fetched.content_type.as_deref()))
    }

    pub async fn get(
        &self,
        url: &str,
        etag: Option<&str>,
        last_modified: Option<&str>,
    ) -> Result<Fetched> {
        let mut last_err = None;
        for attempt in 1..=3 {
            let mut req = self.inner.get(url);
            if let Some(etag) = etag {
                req = req.header(header::IF_NONE_MATCH, etag);
            }
            if let Some(lm) = last_modified {
                req = req.header(header::IF_MODIFIED_SINCE, lm);
            }
            match req.send().await {
                Ok(resp) => {
                    let status = resp.status();
                    if status == StatusCode::NOT_MODIFIED {
                        return Ok(Fetched {
                            status,
                            bytes: Vec::new(),
                            etag: etag.map(str::to_string),
                            last_modified: last_modified.map(str::to_string),
                            content_type: None,
                        });
                    }
                    if status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS {
                        last_err = Some(anyhow::anyhow!("{url} returned {status}"));
                        tokio::time::sleep(Duration::from_millis(400 * attempt)).await;
                        continue;
                    }
                    let etag = header_string(&resp, header::ETAG);
                    let last_modified = header_string(&resp, header::LAST_MODIFIED);
                    let content_type = header_string(&resp, header::CONTENT_TYPE);
                    let bytes = resp.bytes().await.context("read body")?.to_vec();
                    return Ok(Fetched {
                        status,
                        bytes,
                        etag,
                        last_modified,
                        content_type,
                    });
                }
                Err(err) => {
                    last_err = Some(err.into());
                    tokio::time::sleep(Duration::from_millis(400 * attempt)).await;
                }
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("request failed: {url}")))
    }
}

fn header_string(resp: &reqwest::Response, name: header::HeaderName) -> Option<String> {
    resp.headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

pub fn decode_body(bytes: &[u8], content_type: Option<&str>) -> String {
    let label = content_type
        .and_then(charset_label)
        .or_else(|| meta_charset(bytes));
    let encoding = label
        .and_then(|l| Encoding::for_label(l.as_bytes()))
        .unwrap_or(UTF_8);
    let (text, _, _) = encoding.decode(bytes);
    text.into_owned()
}

fn charset_label(content_type: &str) -> Option<String> {
    for part in content_type.split(';').skip(1) {
        let part = part.trim();
        let Some((key, value)) = part.split_once('=') else {
            continue;
        };
        if key.trim().eq_ignore_ascii_case("charset") {
            return Some(value.trim().trim_matches('"').to_string());
        }
    }
    None
}

fn meta_charset(bytes: &[u8]) -> Option<String> {
    let head = &bytes[..bytes.len().min(2048)];
    let lower = String::from_utf8_lossy(head).to_ascii_lowercase();
    let marker = "charset=";
    let idx = lower.find(marker)?;
    let rest = &lower[idx + marker.len()..];
    let end = rest
        .find(|c: char| c == '"' || c == '\'' || c == ';' || c == ' ' || c == '>')
        .unwrap_or(rest.len());
    let label = rest[..end].trim();
    if label.is_empty() {
        None
    } else {
        Some(label.to_string())
    }
}
