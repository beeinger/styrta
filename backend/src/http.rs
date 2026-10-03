use anyhow::{bail, Context, Result};
use encoding_rs::{Encoding, UTF_8};
use reqwest::{header, StatusCode};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

/// Packs above this stay on the ROPS site. The brief then uses the page.
/// Measured packs go up to about 5.5 GB; three are 11–28 GB.
pub const ARCHIVE_CAP: u64 = 6 * 1024 * 1024 * 1024;

pub struct Client {
    inner: reqwest::Client,
}

pub struct Fetched {
    pub status: StatusCode,
    pub bytes: Vec<u8>,
    pub content_type: Option<String>,
}

pub struct Download {
    pub status: StatusCode,
    pub sha256: String,
    pub len: u64,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub too_large: bool,
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
            match self.get_once(url, etag, last_modified).await {
                Ok(fetched) => {
                    if fetched.status.is_server_error()
                        || fetched.status == StatusCode::TOO_MANY_REQUESTS
                    {
                        last_err = Some(anyhow::anyhow!("{url} returned {}", fetched.status));
                        tokio::time::sleep(Duration::from_millis(400 * attempt)).await;
                        continue;
                    }
                    return Ok(fetched);
                }
                Err(err) => {
                    last_err = Some(err);
                    tokio::time::sleep(Duration::from_millis(400 * attempt)).await;
                }
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("request failed: {url}")))
    }

    /// Stream an archive to `dest`. A 304 leaves `dest` untouched.
    pub async fn download(&self, url: &str, dest: &Path, etag: Option<&str>) -> Result<Download> {
        let mut last_err = None;
        for attempt in 1..=3 {
            match self.download_once(url, dest, etag).await {
                Ok(done) => return Ok(done),
                Err(err) => {
                    let _ = std::fs::remove_file(dest);
                    tracing::warn!(url, attempt, error = %err, "download failed");
                    last_err = Some(err);
                    tokio::time::sleep(Duration::from_secs(2 * attempt)).await;
                }
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("download failed: {url}")))
    }

    async fn get_once(
        &self,
        url: &str,
        etag: Option<&str>,
        last_modified: Option<&str>,
    ) -> Result<Fetched> {
        let mut req = self.inner.get(url);
        if let Some(etag) = etag {
            req = req.header(header::IF_NONE_MATCH, etag);
        }
        if let Some(lm) = last_modified {
            req = req.header(header::IF_MODIFIED_SINCE, lm);
        }
        let resp = req.send().await.with_context(|| format!("request {url}"))?;
        let status = resp.status();
        if status == StatusCode::NOT_MODIFIED {
            return Ok(Fetched {
                status,
                bytes: Vec::new(),
                content_type: None,
            });
        }
        let content_type = header_string(&resp, header::CONTENT_TYPE);
        let bytes = resp
            .bytes()
            .await
            .with_context(|| format!("read body {url}"))?
            .to_vec();
        Ok(Fetched {
            status,
            bytes,
            content_type,
        })
    }

    async fn download_once(&self, url: &str, dest: &Path, etag: Option<&str>) -> Result<Download> {
        let mut req = self.inner.get(url).timeout(Duration::from_secs(20 * 60));
        if let Some(etag) = etag {
            req = req.header(header::IF_NONE_MATCH, etag);
        }
        let resp = req.send().await.with_context(|| format!("request {url}"))?;
        let status = resp.status();
        if status == StatusCode::NOT_MODIFIED {
            return Ok(Download {
                status,
                sha256: String::new(),
                len: 0,
                etag: etag.map(str::to_string),
                last_modified: None,
                too_large: false,
            });
        }
        if status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS {
            bail!("{url} returned {status}");
        }
        if !status.is_success() {
            return Ok(Download {
                status,
                sha256: String::new(),
                len: 0,
                etag: header_string(&resp, header::ETAG),
                last_modified: header_string(&resp, header::LAST_MODIFIED),
                too_large: false,
            });
        }
        if let Some(claimed) = resp.content_length() {
            if claimed > ARCHIVE_CAP {
                return Ok(Download {
                    status,
                    sha256: String::new(),
                    len: claimed,
                    etag: None,
                    last_modified: None,
                    too_large: true,
                });
            }
        }
        let etag_out = header_string(&resp, header::ETAG);
        let last_modified = header_string(&resp, header::LAST_MODIFIED);
        let mut file = tokio::fs::File::create(dest)
            .await
            .with_context(|| format!("create {}", dest.display()))?;
        let mut hasher = Sha256::new();
        let mut len = 0u64;
        let mut resp = resp;
        while let Some(chunk) = resp
            .chunk()
            .await
            .with_context(|| format!("read body {url}"))?
        {
            len += chunk.len() as u64;
            if len > ARCHIVE_CAP {
                return Ok(Download {
                    status,
                    sha256: String::new(),
                    len,
                    etag: etag_out,
                    last_modified,
                    too_large: true,
                });
            }
            hasher.update(&chunk);
            file.write_all(&chunk).await.context("write archive")?;
        }
        file.flush().await.context("flush archive")?;
        Ok(Download {
            status,
            sha256: hex::encode(hasher.finalize()),
            len,
            etag: etag_out,
            last_modified,
            too_large: false,
        })
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
