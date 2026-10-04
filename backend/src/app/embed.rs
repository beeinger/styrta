use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;

use crate::config::Config;
use crate::endpoint::{scrub_text, Endpoint};

const EMBED_PATH: &str = "embeddings";
const TIMEOUT: Duration = Duration::from_secs(120);

pub const DIMENSION: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    Query,
    Passage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Transport(String),
    Status { code: u16, body: String },
    Dimension(usize),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Transport(message) => f.write_str(message),
            Error::Status { code, body } => write!(f, "status {code}: {body}"),
            Error::Dimension(got) => write!(f, "embedding width {got}, expected {DIMENSION}"),
        }
    }
}

impl std::error::Error for Error {}

#[async_trait]
pub trait Embedder: Send + Sync {
    async fn embed(&self, input: Input, texts: &[String]) -> Result<Vec<Vec<f32>>, Error>;
}

#[derive(Clone, Debug)]
pub struct Client {
    endpoint: Endpoint,
    model: String,
    query_prefix: String,
    passage_prefix: String,
}

impl Client {
    pub fn new(cfg: &Config) -> Result<Self, Error> {
        let endpoint = Endpoint::new(&cfg.embed_base_url, &cfg.embed_api_key, TIMEOUT, "")
            .map_err(|err| Error::Transport(scrub_text(&format!("{err:#}"), &cfg.embed_api_key)))?;
        Ok(Self {
            endpoint,
            model: cfg.embed_model.clone(),
            query_prefix: cfg.embed_query_prefix.clone(),
            passage_prefix: cfg.embed_passage_prefix.clone(),
        })
    }

    /// Embeds a one-token probe and rejects any width other than [`DIMENSION`].
    pub async fn check_dimension(&self) -> Result<(), Error> {
        let texts = ["a".to_string()];
        let vectors = self.embed(Input::Query, &texts).await?;
        match vectors.as_slice() {
            [vector] if vector.len() == DIMENSION => Ok(()),
            [vector] => Err(Error::Dimension(vector.len())),
            _ => Err(Error::Dimension(0)),
        }
    }

    fn prefix(&self, input: Input, text: &str) -> String {
        let prefix = match input {
            Input::Query => self.query_prefix.as_str(),
            Input::Passage => self.passage_prefix.as_str(),
        };
        let mut out = String::with_capacity(prefix.len() + text.len());
        out.push_str(prefix);
        out.push_str(text);
        out
    }

    fn transport(&self, err: impl ToString) -> Error {
        Error::Transport(self.endpoint.scrub(&err.to_string()))
    }

    fn status(&self, status: reqwest::StatusCode, bytes: &[u8]) -> Error {
        let text = String::from_utf8_lossy(bytes);
        Error::Status {
            code: status.as_u16(),
            body: crate::endpoint::truncate(&self.endpoint.scrub(&text), 400),
        }
    }
}

#[async_trait]
impl Embedder for Client {
    async fn embed(&self, input: Input, texts: &[String]) -> Result<Vec<Vec<f32>>, Error> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let input_texts: Vec<String> = texts.iter().map(|text| self.prefix(input, text)).collect();
        let body = serde_json::json!({
            "model": self.model,
            "input": input_texts,
        });
        let builder = self
            .endpoint
            .post(EMBED_PATH)
            .map_err(|err| self.transport(err))?
            .json(&body);
        let response = builder.send().await.map_err(|err| self.transport(err))?;
        let status = response.status();
        let bytes = response.bytes().await.map_err(|err| self.transport(err))?;
        if !status.is_success() {
            return Err(self.status(status, &bytes));
        }
        if bytes.is_empty() {
            return Err(Error::Transport("empty body".into()));
        }
        let parsed: EmbedResponse =
            serde_json::from_slice(&bytes).map_err(|err| self.transport(err))?;
        take_vectors(parsed.data, texts.len())
    }
}

#[derive(Deserialize)]
struct EmbedResponse {
    data: Vec<EmbedItem>,
}

#[derive(Deserialize)]
struct EmbedItem {
    index: usize,
    embedding: Vec<f32>,
}

fn take_vectors(mut items: Vec<EmbedItem>, expected: usize) -> Result<Vec<Vec<f32>>, Error> {
    if items.len() != expected {
        return Err(Error::Transport(format!(
            "embedding count {} for {expected} inputs",
            items.len()
        )));
    }
    items.sort_by_key(|item| item.index);
    let mut out = Vec::with_capacity(items.len());
    for (i, item) in items.into_iter().enumerate() {
        if item.index != i {
            return Err(Error::Transport(format!("embedding index {}", item.index)));
        }
        if item.embedding.len() != DIMENSION {
            return Err(Error::Dimension(item.embedding.len()));
        }
        out.push(item.embedding);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use axum::extract::State;
    use axum::http::header::USER_AGENT;
    use axum::http::{HeaderMap, StatusCode};
    use axum::routing::post;
    use axum::Json;
    use axum::Router;
    use serde_json::{json, Value};
    use std::sync::{Arc, Mutex};

    const CHAT_KEY: &str = "chat-key-not-for-embed";
    const EMBED_KEY: &str = "embed-key-value";

    #[derive(Clone)]
    struct Capture {
        hits: Arc<Mutex<Vec<Hit>>>,
        width: usize,
        echo_key: bool,
        key: String,
    }

    struct Hit {
        auth: Option<String>,
        input: Vec<String>,
        model: String,
    }

    fn config(base: &str, embed_key: &str, query: &str, passage: &str) -> Config {
        // Built directly so a trailing space in the prefix is not trimmed.
        Config {
            llm_base_url: "http://127.0.0.1:9/v1".into(),
            llm_api_key: CHAT_KEY.into(),
            llm_model: "chat-model".into(),
            llm_timeout: Duration::from_secs(5),
            llm_user_agent: String::new(),
            speech_base_url: "http://127.0.0.1:9/v1".into(),
            speech_api_key: String::new(),
            stt_model: "stt-model".into(),
            tts_model: "tts-model".into(),
            tts_voice: "tts-voice".into(),
            embed_base_url: base.into(),
            embed_api_key: embed_key.into(),
            embed_model: "embed-model".into(),
            embed_query_prefix: query.into(),
            embed_passage_prefix: passage.into(),
            jwt_secret: "jwt-secret-value".into(),
            database_url: "postgres://127.0.0.1/styrta".into(),
            audio_dir: std::path::PathBuf::from("/var/lib/styrta/audio"),
            work_dir: std::path::PathBuf::from("/tmp/styrta-ingest"),
        }
    }

    async fn spawn(width: usize, echo_key: bool, key: &str) -> (String, Arc<Mutex<Vec<Hit>>>) {
        let hits = Arc::new(Mutex::new(Vec::new()));
        let state = Capture {
            hits: hits.clone(),
            width,
            echo_key,
            key: key.to_string(),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = Router::new()
            .route("/v1/embeddings", post(record))
            .with_state(state);
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{addr}/v1"), hits)
    }

    async fn record(
        State(state): State<Capture>,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> axum::response::Response {
        use axum::response::IntoResponse;
        assert!(headers.get(USER_AGENT).is_none());
        let auth = headers
            .get(reqwest::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let input = body["input"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap().to_string())
            .collect();
        let model = body["model"].as_str().unwrap().to_string();
        let leaked = format!("{auth:?} {model} {}", body["input"]);
        assert!(!leaked.contains(CHAT_KEY), "chat key was sent");
        state.hits.lock().unwrap().push(Hit { auth, input, model });
        if state.echo_key {
            return (StatusCode::BAD_REQUEST, state.key).into_response();
        }
        let embedding = vec![0.0_f32; state.width];
        Json(json!({
            "data": [{
                "object": "embedding",
                "index": 0,
                "embedding": embedding,
            }]
        }))
        .into_response()
    }

    #[tokio::test]
    async fn prefixes_query_and_passage_before_the_request() {
        let (base, hits) = spawn(DIMENSION, false, "").await;
        let client = Client::new(&config(&base, "", "query: ", "passage: ")).unwrap();
        let query = client
            .embed(Input::Query, &["hello".to_string()])
            .await
            .unwrap();
        let passage = client
            .embed(Input::Passage, &["hello".to_string()])
            .await
            .unwrap();
        client.check_dimension().await.unwrap();
        assert_eq!(query.len(), 1);
        assert_eq!(query[0].len(), DIMENSION);
        assert_eq!(passage[0].len(), DIMENSION);

        let hits = hits.lock().unwrap();
        assert!(hits.iter().all(|hit| hit.auth.is_none()));
        assert!(hits.iter().all(|hit| hit.model == "embed-model"));
        assert_eq!(hits[0].input, vec!["query: hello".to_string()]);
        assert_eq!(hits[1].input, vec!["passage: hello".to_string()]);
        assert_eq!(hits[2].input, vec!["query: a".to_string()]);
    }

    #[tokio::test]
    async fn rejects_768_dimension() {
        let (base, hits) = spawn(768, false, "").await;
        let client = Client::new(&config(&base, "", "query: ", "passage: ")).unwrap();
        let err = client
            .embed(Input::Passage, &["hello".to_string()])
            .await
            .unwrap_err();
        assert_eq!(err, Error::Dimension(768));
        assert!(err.to_string().contains("768"));
        let probe = client.check_dimension().await.unwrap_err();
        assert_eq!(probe, Error::Dimension(768));
        let hits = hits.lock().unwrap();
        assert_eq!(hits[0].input, vec!["passage: hello".to_string()]);
        assert_eq!(hits[1].input, vec!["query: a".to_string()]);
    }

    #[tokio::test]
    async fn key_is_absent_from_display_and_debug() {
        let (base, hits) = spawn(DIMENSION, true, EMBED_KEY).await;
        let client = Client::new(&config(&base, EMBED_KEY, "", "")).unwrap();
        let err = client
            .embed(Input::Query, &["hello".to_string()])
            .await
            .unwrap_err();
        let rendered = format!("{err} {err:?} {client:?}");
        assert!(!rendered.contains(EMBED_KEY), "key leaked");
        assert!(!rendered.contains(CHAT_KEY), "chat key leaked");
        assert!(rendered.contains("400"), "{err}");
        let hits = hits.lock().unwrap();
        assert_eq!(hits[0].auth.as_deref(), Some("Bearer embed-key-value"));
        assert_ne!(hits[0].auth.as_deref(), Some(CHAT_KEY));
    }
}
