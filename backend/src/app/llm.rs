use std::collections::BTreeMap;
use std::pin::Pin;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use futures::Stream;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use crate::config::Config;
use crate::endpoint::Endpoint;

const ATTEMPTS: u32 = 3;
const CHAT_PATH: &str = "chat/completions";

pub type ModelStream = Pin<Box<dyn Stream<Item = Result<StreamItem>> + Send>>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum Message {
    System {
        content: String,
    },
    User {
        content: String,
    },
    Assistant {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tool_calls: Vec<ToolCall>,
    },
    Tool {
        tool_call_id: String,
        name: String,
        content: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// Raw JSON arguments, as returned by the model.
    pub arguments: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssistantMessage {
    pub content: Option<String>,
    pub tool_calls: Vec<ToolCall>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StreamItem {
    TextDelta(String),
    Done(AssistantMessage),
}

#[derive(Clone, Debug)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[derive(Clone, Debug, Default)]
pub enum ToolChoice {
    #[default]
    Auto,
    None,
    Required,
    Named(String),
}

#[derive(Clone, Debug)]
pub struct CompletionRequest {
    pub messages: Vec<Message>,
    pub tools: Vec<ToolDefinition>,
    pub tool_choice: ToolChoice,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
}

impl CompletionRequest {
    pub fn new(messages: Vec<Message>) -> Self {
        Self {
            messages,
            tools: Vec::new(),
            tool_choice: ToolChoice::Auto,
            temperature: None,
            max_tokens: None,
        }
    }
}

#[async_trait]
pub trait Model: Send + Sync {
    async fn complete(&self, request: &CompletionRequest) -> Result<AssistantMessage>;
    async fn stream(&self, request: &CompletionRequest) -> Result<ModelStream>;
}

#[derive(Clone, Debug)]
pub struct Client {
    endpoint: Endpoint,
    model: String,
}

impl Client {
    pub fn new(cfg: &Config) -> Result<Self> {
        Self::from_chat(
            &cfg.llm_base_url,
            &cfg.llm_api_key,
            &cfg.llm_model,
            cfg.llm_timeout,
            &cfg.llm_user_agent,
        )
    }

    pub fn from_chat(
        base_url: &str,
        api_key: &str,
        model: &str,
        timeout: Duration,
        user_agent: &str,
    ) -> Result<Self> {
        let endpoint = Endpoint::new(base_url, api_key, timeout, user_agent)?;
        Ok(Self {
            endpoint,
            model: model.to_string(),
        })
    }

    pub fn model(&self) -> &str {
        &self.model
    }
}

#[async_trait]
impl Model for Client {
    async fn complete(&self, request: &CompletionRequest) -> Result<AssistantMessage> {
        let response = self.send(request, false).await?;
        let text = response.text().await.context("chat completion")?;
        parse_completion(&text).map_err(|err| scrub_error(&self.endpoint, err))
    }

    async fn stream(&self, request: &CompletionRequest) -> Result<ModelStream> {
        let response = self.send(request, true).await?;
        Ok(spawn_stream(response, self.endpoint.clone()))
    }
}

impl Client {
    async fn send(&self, request: &CompletionRequest, stream: bool) -> Result<reqwest::Response> {
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            match self.send_once(request, stream).await {
                Ok(response) => return Ok(response),
                Err(failed) if failed.retryable && attempt < ATTEMPTS => {
                    tracing::warn!(attempt, error = %failed.error, "chat completion failed");
                    tokio::time::sleep(Duration::from_millis(200 * u64::from(attempt))).await;
                }
                Err(failed) => return Err(failed.error.context("chat completion")),
            }
        }
    }

    async fn send_once(
        &self,
        request: &CompletionRequest,
        stream: bool,
    ) -> std::result::Result<reqwest::Response, Attempt> {
        let builder = self
            .endpoint
            .post(CHAT_PATH)
            .map_err(|err| fatal(self.endpoint.scrub(&err.to_string())))?;
        let body = wire_body(&self.model, request, stream);
        let response = match builder.json(&body).send().await {
            Ok(response) => response,
            Err(err) => {
                return Err(Attempt {
                    retryable: retryable_transport(&err),
                    error: anyhow!(self.endpoint.scrub(&err.to_string())),
                });
            }
        };
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let bytes = response.bytes().await.unwrap_or_default();
        let text = String::from_utf8_lossy(&bytes);
        Err(Attempt {
            retryable: retryable_status(status),
            error: anyhow!(http_error(status, &text, &self.endpoint)),
        })
    }
}

struct Attempt {
    retryable: bool,
    error: anyhow::Error,
}

fn fatal(message: String) -> Attempt {
    Attempt {
        retryable: false,
        error: anyhow!(message),
    }
}

fn scrub_error(endpoint: &Endpoint, err: anyhow::Error) -> anyhow::Error {
    anyhow!(endpoint.scrub(&format!("{err:#}"))).context("chat completion")
}

pub(crate) fn retryable_status(status: reqwest::StatusCode) -> bool {
    status.as_u16() == 429 || status.is_server_error()
}

pub(crate) fn retryable_transport(err: &reqwest::Error) -> bool {
    err.is_timeout() || err.is_connect()
}

fn http_error(status: reqwest::StatusCode, body: &str, endpoint: &Endpoint) -> String {
    let body = endpoint.scrub(&truncate(body, 400));
    format!("chat completion failed: {status}: {body}")
}

fn truncate(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

fn wire_body(model: &str, request: &CompletionRequest, stream: bool) -> Value {
    let mut body = json!({
        "model": model,
        "messages": request.messages.iter().map(message_to_wire).collect::<Vec<_>>(),
        "stream": stream,
    });
    if let Some(temperature) = request.temperature {
        body["temperature"] = json!(temperature);
    }
    if let Some(max_tokens) = request.max_tokens {
        body["max_tokens"] = json!(max_tokens);
    }
    if !request.tools.is_empty() {
        body["tools"] = json!(request.tools.iter().map(tool_to_wire).collect::<Vec<_>>());
        body["tool_choice"] = tool_choice_wire(&request.tool_choice);
    }
    body
}

fn message_to_wire(message: &Message) -> Value {
    match message {
        Message::System { content } => json!({"role": "system", "content": content}),
        Message::User { content } => json!({"role": "user", "content": content}),
        Message::Assistant {
            content,
            tool_calls,
        } => {
            let mut value = json!({
                "role": "assistant",
                "content": content,
            });
            if !tool_calls.is_empty() {
                value["tool_calls"] =
                    json!(tool_calls.iter().map(tool_call_wire).collect::<Vec<_>>());
            }
            value
        }
        Message::Tool {
            tool_call_id,
            name,
            content,
        } => json!({
            "role": "tool",
            "tool_call_id": tool_call_id,
            "name": name,
            "content": content,
        }),
    }
}

fn tool_call_wire(call: &ToolCall) -> Value {
    json!({
        "id": call.id,
        "type": "function",
        "function": {
            "name": call.name,
            "arguments": call.arguments,
        }
    })
}

fn tool_to_wire(tool: &ToolDefinition) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": tool.name,
            "description": tool.description,
            "parameters": tool.parameters,
        }
    })
}

fn tool_choice_wire(choice: &ToolChoice) -> Value {
    match choice {
        ToolChoice::Auto => json!("auto"),
        ToolChoice::None => json!("none"),
        ToolChoice::Required => json!("required"),
        ToolChoice::Named(name) => json!({"type": "function", "function": {"name": name}}),
    }
}

fn parse_completion(text: &str) -> Result<AssistantMessage> {
    let body: CompletionBody = serde_json::from_str(text).context("chat completion")?;
    let choice = body
        .choices
        .into_iter()
        .next()
        .context("chat completion returned no choices")?;
    let message = choice
        .message
        .context("chat completion returned no message")?;
    let mut assembler = Assembler::default();
    if let Some(content) = message.content {
        if let Some(piece) = content_text(&content)? {
            assembler.content.push_str(&piece);
        }
    }
    if let Some(calls) = message.tool_calls {
        for (pos, call) in calls.into_iter().enumerate() {
            assembler.push_tool(pos as u32, call);
        }
    }
    assembler.finish()
}

#[derive(Deserialize)]
struct CompletionBody {
    choices: Vec<CompletionChoice>,
}

#[derive(Deserialize)]
struct CompletionChoice {
    message: Option<CompletionMessage>,
}

#[derive(Deserialize)]
struct CompletionMessage {
    content: Option<Value>,
    tool_calls: Option<Vec<RawTool>>,
}

#[derive(Deserialize)]
struct ChunkBody {
    choices: Vec<ChunkChoice>,
}

#[derive(Deserialize)]
struct ChunkChoice {
    delta: Option<Delta>,
}

#[derive(Deserialize)]
struct Delta {
    content: Option<Value>,
    tool_calls: Option<Vec<RawTool>>,
}

#[derive(Deserialize)]
struct RawTool {
    index: Option<u32>,
    id: Option<String>,
    function: Option<RawFunction>,
}

#[derive(Deserialize)]
struct RawFunction {
    name: Option<String>,
    arguments: Option<Value>,
}

#[derive(Default)]
struct Assembler {
    content: String,
    saw: bool,
    tools: BTreeMap<u32, PartialTool>,
}

#[derive(Default)]
struct PartialTool {
    id: String,
    name: String,
    arguments: String,
}

impl Assembler {
    fn push_tool(&mut self, fallback: u32, call: RawTool) {
        let idx = call.index.unwrap_or(fallback);
        let slot = self.tools.entry(idx).or_default();
        if let Some(id) = call.id {
            if !id.is_empty() {
                slot.id = id;
            }
        }
        let Some(function) = call.function else {
            return;
        };
        if let Some(name) = function.name {
            if !name.is_empty() {
                slot.name = name;
            }
        }
        if let Some(arguments) = function.arguments {
            slot.arguments.push_str(&args_fragment(&arguments));
        }
    }

    fn finish(&self) -> Result<AssistantMessage> {
        let mut tool_calls = Vec::with_capacity(self.tools.len());
        for call in self.tools.values() {
            if call.id.is_empty() || call.name.is_empty() {
                bail!("chat completion tool call");
            }
            tool_calls.push(ToolCall {
                id: call.id.clone(),
                name: call.name.clone(),
                arguments: call.arguments.clone(),
            });
        }
        Ok(AssistantMessage {
            content: nonempty(&self.content),
            tool_calls,
        })
    }
}

fn nonempty(text: &str) -> Option<String> {
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

fn args_fragment(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

fn content_text(value: &Value) -> Result<Option<String>> {
    match value {
        Value::Null => Ok(None),
        Value::String(text) => Ok(Some(text.clone())),
        Value::Array(parts) => {
            let mut out = String::new();
            for part in parts {
                if let Some(text) = part.get("text").and_then(Value::as_str) {
                    out.push_str(text);
                } else if let Some(text) = part.as_str() {
                    out.push_str(text);
                } else {
                    bail!("chat completion content");
                }
            }
            Ok(Some(out))
        }
        _ => bail!("chat completion content"),
    }
}

enum Apply {
    Ignore,
    Delta(String),
    Finished(AssistantMessage),
}

fn apply_event(assembler: &mut Assembler, event: &str) -> Result<Apply> {
    let Some(data) = event_data(event) else {
        return Ok(Apply::Ignore);
    };
    if data == "[DONE]" {
        return Ok(Apply::Finished(assembler.finish()?));
    }
    if data.is_empty() {
        return Ok(Apply::Ignore);
    }
    let chunk: ChunkBody = serde_json::from_str(&data).context("chat completion")?;
    assembler.saw = true;
    let Some(choice) = chunk.choices.into_iter().next() else {
        return Ok(Apply::Ignore);
    };
    let Some(delta) = choice.delta else {
        return Ok(Apply::Ignore);
    };
    let mut text = None;
    if let Some(content) = delta.content {
        if let Some(piece) = content_text(&content)? {
            if !piece.is_empty() {
                assembler.content.push_str(&piece);
                text = Some(piece);
            }
        }
    }
    if let Some(calls) = delta.tool_calls {
        for (pos, call) in calls.into_iter().enumerate() {
            assembler.push_tool(pos as u32, call);
        }
    }
    Ok(match text {
        Some(piece) => Apply::Delta(piece),
        None => Apply::Ignore,
    })
}

fn event_data(event: &str) -> Option<String> {
    let mut data = String::new();
    let mut any = false;
    for line in event.split('\n') {
        let line = line.trim_end_matches('\r');
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        if let Some(rest) = line.strip_prefix("data:") {
            if any {
                data.push('\n');
            }
            any = true;
            data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
        }
    }
    any.then_some(data)
}

fn pop_event(buf: &mut Vec<u8>) -> Result<Option<String>> {
    let Some((at, len)) = find_sep(buf) else {
        return Ok(None);
    };
    let raw: Vec<u8> = buf.drain(..at + len).collect();
    let text = String::from_utf8(raw[..at].to_vec()).context("chat completion")?;
    Ok(Some(text))
}

fn find_sep(buf: &[u8]) -> Option<(usize, usize)> {
    let nl = find_sub(buf, b"\n\n").map(|at| (at, 2));
    let cr = find_sub(buf, b"\r\n\r\n").map(|at| (at, 4));
    match (nl, cr) {
        (Some(a), Some(b)) if b.0 < a.0 => Some(b),
        (Some(a), _) => Some(a),
        (None, other) => other,
    }
}

fn find_sub(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len())
        .position(|window| window == needle)
}

fn spawn_stream(response: reqwest::Response, endpoint: Endpoint) -> ModelStream {
    let (tx, rx) = mpsc::channel(32);
    tokio::spawn(async move {
        if let Err(err) = read_stream(response, &endpoint, &tx).await {
            let _ = tx.send(Err(scrub_error(&endpoint, err))).await;
        }
    });
    Box::pin(ReceiverStream::new(rx))
}

async fn read_stream(
    response: reqwest::Response,
    endpoint: &Endpoint,
    tx: &mpsc::Sender<Result<StreamItem>>,
) -> Result<()> {
    use futures::StreamExt;
    let mut incoming = response.bytes_stream();
    let mut buf = Vec::new();
    let mut assembler = Assembler::default();
    loop {
        if dispatch(&mut buf, &mut assembler, tx).await? {
            return Ok(());
        }
        match incoming.next().await {
            Some(Ok(chunk)) => buf.extend_from_slice(&chunk),
            Some(Err(err)) => {
                return Err(anyhow!(endpoint.scrub(&err.to_string())).context("chat completion"));
            }
            None => break,
        }
    }
    if !buf.is_empty() {
        let tail = String::from_utf8(buf).context("chat completion")?;
        match apply_event(&mut assembler, &tail)? {
            Apply::Ignore => {}
            Apply::Delta(text) => {
                if tx.send(Ok(StreamItem::TextDelta(text))).await.is_err() {
                    return Ok(());
                }
            }
            Apply::Finished(message) => {
                let _ = tx.send(Ok(StreamItem::Done(message))).await;
                return Ok(());
            }
        }
    }
    if !assembler.saw && assembler.content.is_empty() && assembler.tools.is_empty() {
        bail!("chat completion returned no choices");
    }
    let message = assembler.finish()?;
    let _ = tx.send(Ok(StreamItem::Done(message))).await;
    Ok(())
}

/// Returns true when the stream has emitted its final message.
async fn dispatch(
    buf: &mut Vec<u8>,
    assembler: &mut Assembler,
    tx: &mpsc::Sender<Result<StreamItem>>,
) -> Result<bool> {
    while let Some(event) = pop_event(buf)? {
        match apply_event(assembler, &event)? {
            Apply::Ignore => {}
            Apply::Delta(text) => {
                if tx.send(Ok(StreamItem::TextDelta(text))).await.is_err() {
                    return Ok(true);
                }
            }
            Apply::Finished(message) => {
                let _ = tx.send(Ok(StreamItem::Done(message))).await;
                return Ok(true);
            }
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Bytes;
    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode};
    use axum::routing::post;
    use axum::Router;
    use futures::StreamExt;
    use reqwest::header::{AUTHORIZATION, USER_AGENT};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn sample_completion() -> &'static str {
        r#"{"id":"c1","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"search_events","arguments":{"lat":1}}}]},"finish_reason":"tool_calls"}]}"#
    }

    #[test]
    fn parses_tool_call_and_text_parts() {
        let message = parse_completion(sample_completion()).unwrap();
        assert!(message.content.is_none());
        assert_eq!(message.tool_calls.len(), 1);
        assert_eq!(message.tool_calls[0].name, "search_events");
        assert_eq!(message.tool_calls[0].arguments, r#"{"lat":1}"#);
        assert_eq!(message.tool_calls[0].id, "call_1");

        let text = r#"{"choices":[{"message":{"content":[{"type":"text","text":"Hel"},{"type":"text","text":"lo"}]}}]}"#;
        let message = parse_completion(text).unwrap();
        assert_eq!(message.content.as_deref(), Some("Hello"));
        assert!(message.tool_calls.is_empty());
    }

    #[test]
    fn parses_stream_deltas_then_done() {
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"search_events\",\"arguments\":\"\"}}]}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"q\\\":\"}}]}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"1}\"}}]}}]}\n\n",
            "data: [DONE]\n\n",
        );
        let items = collect_sse(body).unwrap();
        assert_eq!(
            items,
            vec![
                StreamItem::TextDelta("Hel".into()),
                StreamItem::TextDelta("lo".into()),
                StreamItem::Done(AssistantMessage {
                    content: Some("Hello".into()),
                    tool_calls: vec![ToolCall {
                        id: "call_1".into(),
                        name: "search_events".into(),
                        arguments: r#"{"q":1}"#.into(),
                    }],
                }),
            ]
        );
    }

    fn collect_sse(body: &str) -> Result<Vec<StreamItem>> {
        let mut buf = body.as_bytes().to_vec();
        let mut assembler = Assembler::default();
        let mut items = Vec::new();
        while let Some(event) = pop_event(&mut buf)? {
            match apply_event(&mut assembler, &event)? {
                Apply::Ignore => {}
                Apply::Delta(text) => items.push(StreamItem::TextDelta(text)),
                Apply::Finished(message) => {
                    items.push(StreamItem::Done(message));
                    return Ok(items);
                }
            }
        }
        Ok(items)
    }

    #[tokio::test]
    async fn connect_failure_is_retryable() {
        let err = reqwest::Client::builder()
            .connect_timeout(Duration::from_millis(200))
            .timeout(Duration::from_millis(200))
            .build()
            .unwrap()
            .get("http://127.0.0.1:1/")
            .send()
            .await
            .unwrap_err();
        assert!(retryable_transport(&err), "{err}");
    }

    #[test]
    fn classifies_retryable_statuses() {
        for code in [429, 500, 502, 503, 504] {
            let status = StatusCode::from_u16(code).unwrap();
            assert!(retryable_status(status), "{code}");
        }
        for code in [400, 401, 403, 404, 422] {
            let status = StatusCode::from_u16(code).unwrap();
            assert!(!retryable_status(status), "{code}");
        }
    }

    #[derive(Clone)]
    struct Script {
        hits: Arc<AtomicUsize>,
        key: String,
        fail_first: bool,
        stream: bool,
        user_agent: Option<String>,
    }

    async fn spawn_script(script: Script) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = Router::new()
            .route("/v1/chat/completions", post(scripted))
            .with_state(script);
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{addr}/v1")
    }

    async fn scripted(
        State(script): State<Script>,
        headers: HeaderMap,
        body: Bytes,
    ) -> (StatusCode, String) {
        let n = script.hits.fetch_add(1, Ordering::SeqCst);
        let auth = headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");
        assert_eq!(auth, format!("Bearer {}", script.key));
        let agent = headers
            .get(USER_AGENT)
            .and_then(|value| value.to_str().ok());
        assert_eq!(agent, script.user_agent.as_deref());
        let parsed: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["model"], "chat-model");
        assert!(parsed.get("tool_choice").is_none());
        if script.stream {
            assert_eq!(parsed["stream"], true);
            let sse = concat!(
                "data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n\n",
                "data: [DONE]\n\n",
            );
            return (StatusCode::OK, sse.to_string());
        }
        if script.fail_first && n == 0 {
            return (StatusCode::INTERNAL_SERVER_ERROR, script.key);
        }
        (StatusCode::OK, sample_completion().to_string())
    }

    fn client(base: &str, key: &str) -> Client {
        let map: std::collections::HashMap<String, String> = [
            ("OPENAI_BASE_URL", base),
            ("OPENAI_API_KEY", key),
            ("OPENAI_MODEL", "chat-model"),
            ("OPENAI_TIMEOUT_SECS", "5"),
            ("SPEECH_BASE_URL", "http://127.0.0.1:9/speech"),
            ("STT_MODEL", "stt"),
            ("TTS_MODEL", "tts"),
            ("TTS_VOICE", "voice"),
            ("EMBED_BASE_URL", "http://127.0.0.1:9/embed"),
            ("EMBED_MODEL", "embed"),
            ("EMBED_QUERY_PREFIX", ""),
            ("EMBED_PASSAGE_PREFIX", ""),
            ("STYRTA_JWT_SECRET", "jwt-secret-value"),
            ("DATABASE_URL", "postgres://127.0.0.1/styrta"),
        ]
        .into_iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect();
        let cfg = Config::from_lookup(|name| map.get(name).cloned()).unwrap();
        Client::new(&cfg).unwrap()
    }

    #[tokio::test]
    async fn retries_server_error_then_parses_tool_call() {
        let key = "llm-test-key-value".to_string();
        let hits = Arc::new(AtomicUsize::new(0));
        let base = spawn_script(Script {
            hits: hits.clone(),
            key: key.clone(),
            fail_first: true,
            stream: false,
            user_agent: None,
        })
        .await;
        let client = client(&base, &key);
        let request = CompletionRequest::new(vec![Message::User {
            content: "nearby".into(),
        }]);
        let message = client.complete(&request).await.unwrap();
        assert_eq!(hits.load(Ordering::SeqCst), 2);
        assert_eq!(message.tool_calls[0].name, "search_events");
    }

    #[tokio::test]
    async fn fatal_status_is_not_retried_and_key_is_scrubbed() {
        let key = "llm-test-key-value".to_string();
        let hits = Arc::new(AtomicUsize::new(0));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let state = Script {
            hits: hits.clone(),
            key: key.clone(),
            fail_first: false,
            stream: false,
            user_agent: None,
        };
        let app = Router::new()
            .route(
                "/v1/chat/completions",
                post(
                    |State(script): State<Script>, headers: HeaderMap| async move {
                        script.hits.fetch_add(1, Ordering::SeqCst);
                        let auth = headers
                            .get(AUTHORIZATION)
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or("");
                        assert_eq!(auth, format!("Bearer {}", script.key));
                        (StatusCode::BAD_REQUEST, script.key)
                    },
                ),
            )
            .with_state(state);
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client = client(&format!("http://{addr}/v1"), &key);
        let err = client
            .complete(&CompletionRequest::new(vec![Message::User {
                content: "hi".into(),
            }]))
            .await
            .unwrap_err();
        let msg = format!("{err:#}");
        assert!(!msg.contains(&key), "{msg}");
        assert!(msg.contains("400") || msg.contains("Bad Request"), "{msg}");
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn stream_emits_delta_then_done() {
        let key = "llm-test-key-value".to_string();
        let base = spawn_script(Script {
            hits: Arc::new(AtomicUsize::new(0)),
            key: key.clone(),
            fail_first: false,
            stream: true,
            user_agent: None,
        })
        .await;
        let client = client(&base, &key);
        let mut stream = client
            .stream(&CompletionRequest::new(vec![Message::User {
                content: "hi".into(),
            }]))
            .await
            .unwrap();
        let mut items = Vec::new();
        while let Some(item) = stream.next().await {
            items.push(item.unwrap());
        }
        assert_eq!(
            items,
            vec![
                StreamItem::TextDelta("Hi".into()),
                StreamItem::Done(AssistantMessage {
                    content: Some("Hi".into()),
                    tool_calls: Vec::new(),
                }),
            ]
        );
    }

    #[tokio::test]
    async fn sends_configured_user_agent() {
        let key = "llm-test-key-value".to_string();
        let agent = "test-agent/1".to_string();
        let base = spawn_script(Script {
            hits: Arc::new(AtomicUsize::new(0)),
            key: key.clone(),
            fail_first: false,
            stream: false,
            user_agent: Some(agent.clone()),
        })
        .await;
        let mut map: std::collections::HashMap<String, String> = [
            ("OPENAI_BASE_URL", base.as_str()),
            ("OPENAI_API_KEY", key.as_str()),
            ("OPENAI_MODEL", "chat-model"),
            ("OPENAI_TIMEOUT_SECS", "5"),
            ("OPENAI_USER_AGENT", agent.as_str()),
            ("SPEECH_BASE_URL", "http://127.0.0.1:9/speech"),
            ("STT_MODEL", "stt"),
            ("TTS_MODEL", "tts"),
            ("TTS_VOICE", "voice"),
            ("EMBED_BASE_URL", "http://127.0.0.1:9/embed"),
            ("EMBED_MODEL", "embed"),
            ("EMBED_QUERY_PREFIX", ""),
            ("EMBED_PASSAGE_PREFIX", ""),
            ("STYRTA_JWT_SECRET", "jwt-secret-value"),
            ("DATABASE_URL", "postgres://127.0.0.1/styrta"),
        ]
        .into_iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect();
        let cfg = Config::from_lookup(|name| map.remove(name)).unwrap();
        let client = Client::new(&cfg).unwrap();
        let message = client
            .complete(&CompletionRequest::new(vec![Message::User {
                content: "nearby".into(),
            }]))
            .await
            .unwrap();
        assert_eq!(message.tool_calls[0].name, "search_events");
    }

    #[test]
    fn message_checkpoint_round_trip() {
        let message = Message::Assistant {
            content: None,
            tool_calls: vec![ToolCall {
                id: "call_1".into(),
                name: "remember".into(),
                arguments: "{\"key\":\"a\"}".into(),
            }],
        };
        let json = serde_json::to_string(&message).unwrap();
        let back: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(back, message);
    }
}
