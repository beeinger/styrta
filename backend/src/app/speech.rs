use async_trait::async_trait;
use bytes::Bytes;
use serde::Deserialize;

use crate::config::Config;
use crate::endpoint::{scrub_text, Endpoint};

const TRANSCRIBE_PATH: &str = "audio/transcriptions";
const SPEAK_PATH: &str = "audio/speech";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Transport(String),
    Status { code: u16, body: String },
    EmptyBody,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Transport(message) => f.write_str(message),
            Error::Status { code, body } => write!(f, "status {code}: {body}"),
            Error::EmptyBody => f.write_str("empty body"),
        }
    }
}

impl std::error::Error for Error {}

#[async_trait]
pub trait Speech: Send + Sync {
    async fn transcribe(
        &self,
        audio: Bytes,
        content_type: &str,
        language: &str,
    ) -> Result<String, Error>;
    async fn speak(&self, text: &str) -> Result<Bytes, Error>;
}

#[derive(Clone, Debug)]
pub struct Client {
    endpoint: Endpoint,
    stt_model: String,
    tts_model: String,
    tts_voice: String,
}

impl Client {
    pub fn new(cfg: &Config) -> Result<Self, Error> {
        let endpoint = Endpoint::new(
            &cfg.speech_base_url,
            &cfg.speech_api_key,
            cfg.llm_timeout,
            "",
        )
        .map_err(|err| Error::Transport(scrub_text(&format!("{err:#}"), &cfg.speech_api_key)))?;
        Ok(Self {
            endpoint,
            stt_model: cfg.stt_model.clone(),
            tts_model: cfg.tts_model.clone(),
            tts_voice: cfg.tts_voice.clone(),
        })
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

    async fn send(&self, builder: reqwest::RequestBuilder) -> Result<Bytes, Error> {
        let response = builder.send().await.map_err(|err| self.transport(err))?;
        let status = response.status();
        let bytes = response.bytes().await.map_err(|err| self.transport(err))?;
        if !status.is_success() {
            return Err(self.status(status, &bytes));
        }
        if bytes.is_empty() {
            return Err(Error::EmptyBody);
        }
        Ok(bytes)
    }
}

#[async_trait]
impl Speech for Client {
    async fn transcribe(
        &self,
        audio: Bytes,
        content_type: &str,
        language: &str,
    ) -> Result<String, Error> {
        let part = file_part(audio, content_type).map_err(|err| match err {
            Error::Transport(message) => Error::Transport(self.endpoint.scrub(&message)),
            other => other,
        })?;
        let form = reqwest::multipart::Form::new()
            .text("model", self.stt_model.clone())
            .text("language", language.to_string())
            .part("file", part);
        let builder = self
            .endpoint
            .post(TRANSCRIBE_PATH)
            .map_err(|err| self.transport(err))?
            .multipart(form);
        let bytes = self.send(builder).await?;
        let parsed: Transcription =
            serde_json::from_slice(&bytes).map_err(|err| self.transport(err))?;
        Ok(parsed.text)
    }

    async fn speak(&self, text: &str) -> Result<Bytes, Error> {
        let body = serde_json::json!({
            "model": self.tts_model,
            "input": text,
            "voice": self.tts_voice,
        });
        let builder = self
            .endpoint
            .post(SPEAK_PATH)
            .map_err(|err| self.transport(err))?
            .json(&body);
        self.send(builder).await
    }
}

#[derive(Deserialize)]
struct Transcription {
    text: String,
}

fn file_part(audio: Bytes, content_type: &str) -> Result<reqwest::multipart::Part, Error> {
    let mime = content_type.split(';').next().unwrap_or_default().trim();
    let mime = if mime.is_empty() {
        "application/octet-stream"
    } else {
        mime
    };
    reqwest::multipart::Part::bytes(audio.to_vec())
        .file_name(audio_filename(mime))
        .mime_str(mime)
        .map_err(|err| Error::Transport(err.to_string()))
}

fn audio_filename(mime: &str) -> &'static str {
    match mime {
        "audio/wav" | "audio/x-wav" | "audio/wave" => "audio.wav",
        "audio/mpeg" | "audio/mp3" => "audio.mp3",
        "audio/webm" => "audio.webm",
        "audio/ogg" => "audio.ogg",
        "audio/mp4" | "audio/m4a" => "audio.m4a",
        "audio/flac" => "audio.flac",
        _ => "audio.bin",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::extract::State;
    use axum::http::header::{AUTHORIZATION, USER_AGENT};
    use axum::http::{Request, StatusCode};
    use axum::routing::post;
    use axum::Router;
    use std::sync::{Arc, Mutex};

    const CHAT_KEY: &str = "chat-key-not-for-speech";
    const SPEECH_KEY: &str = "speech-key-value";

    #[derive(Clone)]
    struct Capture {
        hits: Arc<Mutex<Vec<Hit>>>,
        mode: Mode,
        key: String,
    }

    #[derive(Clone, Copy)]
    enum Mode {
        Ok,
        EchoKey,
        Empty,
        Redirect,
    }

    struct Hit {
        path: String,
        auth: Option<String>,
        body: Vec<u8>,
    }

    fn config(base: &str, speech_key: &str) -> Config {
        let map: std::collections::HashMap<String, String> = [
            ("LLM_BASE_URL", "http://127.0.0.1:9/v1"),
            ("LLM_API_KEY", CHAT_KEY),
            ("LLM_MODEL", "chat-model"),
            ("LLM_TIMEOUT_SECS", "5"),
            ("SPEECH_BASE_URL", base),
            ("SPEECH_API_KEY", speech_key),
            ("STT_MODEL", "stt-model"),
            ("TTS_MODEL", "tts-model"),
            ("TTS_VOICE", "tts-voice"),
            ("EMBED_BASE_URL", "http://127.0.0.1:9/v1"),
            ("EMBED_MODEL", "embed-model"),
            ("EMBED_QUERY_PREFIX", "query: "),
            ("EMBED_PASSAGE_PREFIX", "passage: "),
            ("STYRTA_JWT_SECRET", "jwt-secret-value"),
            ("DATABASE_URL", "postgres://127.0.0.1/styrta"),
        ]
        .into_iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect();
        Config::from_lookup(|name| map.get(name).cloned()).unwrap()
    }

    async fn spawn(mode: Mode, key: &str) -> (String, Arc<Mutex<Vec<Hit>>>) {
        let hits = Arc::new(Mutex::new(Vec::new()));
        let state = Capture {
            hits: hits.clone(),
            mode,
            key: key.to_string(),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = Router::new()
            .route("/v1/audio/transcriptions", post(record))
            .route("/v1/audio/speech", post(record))
            .with_state(state);
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{addr}/v1"), hits)
    }

    async fn record(
        State(state): State<Capture>,
        request: Request<Body>,
    ) -> axum::response::Response {
        assert!(request.headers().get(USER_AGENT).is_none());
        let path = request.uri().path().to_string();
        let auth = request
            .headers()
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let (parts, body) = request.into_parts();
        let bytes = axum::body::to_bytes(body, 1024 * 1024).await.unwrap();
        let leaked = format!("{auth:?} {path} {}", String::from_utf8_lossy(&bytes));
        assert!(!leaked.contains(CHAT_KEY), "chat key was sent");
        state.hits.lock().unwrap().push(Hit {
            path,
            auth,
            body: bytes.to_vec(),
        });
        match state.mode {
            Mode::EchoKey => (StatusCode::INTERNAL_SERVER_ERROR, state.key).into_response(),
            Mode::Empty => StatusCode::OK.into_response(),
            Mode::Redirect => {
                let mut response = StatusCode::FOUND.into_response();
                response.headers_mut().insert(
                    axum::http::header::LOCATION,
                    "http://127.0.0.1:9/v1/audio/speech".parse().unwrap(),
                );
                response
            }
            Mode::Ok if parts.uri.path().ends_with("audio/speech") => {
                (StatusCode::OK, Bytes::from_static(b"audio-bytes")).into_response()
            }
            Mode::Ok => (
                StatusCode::OK,
                axum::Json(serde_json::json!({"text": "cześć"})),
            )
                .into_response(),
        }
    }

    use axum::response::IntoResponse;

    #[tokio::test]
    async fn empty_key_sends_no_authorization_and_not_the_chat_key() {
        let (base, hits) = spawn(Mode::Ok, "").await;
        let client = Client::new(&config(&base, "")).unwrap();
        let text = client
            .transcribe(Bytes::from_static(b"RIFF-audio"), "audio/wav", "pl")
            .await
            .unwrap();
        assert_eq!(text, "cześć");
        let audio = client.speak("dzień dobry").await.unwrap();
        assert_eq!(audio.as_ref(), b"audio-bytes");

        let hits = hits.lock().unwrap();
        assert_eq!(hits.len(), 2);
        assert!(hits.iter().all(|hit| hit.auth.is_none()));
        assert_eq!(hits[0].path, "/v1/audio/transcriptions");
        assert_eq!(hits[1].path, "/v1/audio/speech");
        let transcribe = String::from_utf8_lossy(&hits[0].body);
        assert!(transcribe.contains("stt-model"));
        assert!(transcribe.contains("RIFF-audio"));
        assert_eq!(form_field(&transcribe, "language").as_deref(), Some("pl"));
        assert!(!transcribe.contains(CHAT_KEY));
        let speak: serde_json::Value = serde_json::from_slice(&hits[1].body).unwrap();
        assert_eq!(speak["model"], "tts-model");
        assert_eq!(speak["input"], "dzień dobry");
        assert_eq!(speak["voice"], "tts-voice");
        assert!(!hits[1]
            .body
            .windows(CHAT_KEY.len())
            .any(|w| w == CHAT_KEY.as_bytes()));
        let rendered = format!("{client:?}");
        assert!(!rendered.contains(CHAT_KEY));
    }

    #[tokio::test]
    async fn transcribe_sends_the_callers_language() {
        let (base, hits) = spawn(Mode::Ok, "").await;
        let client = Client::new(&config(&base, "")).unwrap();
        client
            .transcribe(Bytes::from_static(b"wav"), "audio/wav", "en")
            .await
            .unwrap();
        let body = String::from_utf8(hits.lock().unwrap()[0].body.clone()).unwrap();
        assert_eq!(form_field(&body, "model").as_deref(), Some("stt-model"));
        assert_eq!(form_field(&body, "language").as_deref(), Some("en"));
    }

    fn form_field(body: &str, name: &str) -> Option<String> {
        let marker = format!("name=\"{name}\"");
        let start = body.find(&marker)? + marker.len();
        let value = body[start..].split("\r\n\r\n").nth(1)?;
        Some(value.split("\r\n").next()?.to_string())
    }

    #[tokio::test]
    async fn key_is_absent_from_display_and_debug() {
        let (base, _) = spawn(Mode::EchoKey, SPEECH_KEY).await;
        let client = Client::new(&config(&base, SPEECH_KEY)).unwrap();
        let err = client.speak("hello").await.unwrap_err();
        let rendered = format!("{err} {err:?} {client:?}");
        assert!(!rendered.contains(SPEECH_KEY), "key leaked");
        assert!(rendered.contains("500"), "{err}");
        assert!(matches!(err, Error::Status { code: 500, .. }));
    }

    #[tokio::test]
    async fn empty_success_body_is_an_error() {
        let (base, _) = spawn(Mode::Empty, "").await;
        let client = Client::new(&config(&base, "")).unwrap();
        let err = client.speak("hello").await.unwrap_err();
        assert_eq!(err, Error::EmptyBody);
        assert_eq!(err.to_string(), "empty body");
    }

    #[tokio::test]
    async fn redirect_is_not_followed() {
        let (base, hits) = spawn(Mode::Redirect, "").await;
        let client = Client::new(&config(&base, "")).unwrap();
        let err = client.speak("hello").await.unwrap_err();
        assert!(matches!(err, Error::Status { code: 302, .. }), "{err}");
        assert_eq!(hits.lock().unwrap().len(), 1);
    }
}
