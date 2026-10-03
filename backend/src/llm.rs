use crate::config::Config;
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Clone, Debug)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Clone, Debug)]
pub struct ModelMessage {
    pub content: Option<String>,
    pub tool_calls: Vec<ToolCall>,
}

pub struct Llm {
    http: reqwest::Client,
    base: String,
    key: String,
    model: String,
}

impl Llm {
    pub fn new(cfg: &Config) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(180))
            .build()?;
        Ok(Self {
            http,
            base: cfg.openai_base_url.clone(),
            key: cfg.openai_api_key.clone(),
            model: cfg.openai_model.clone(),
        })
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub async fn chat(&self, messages: &[Value], tools: &Value) -> Result<ModelMessage> {
        let body = json!({
            "model": self.model,
            "temperature": 0.1,
            "max_tokens": 8000,
            "messages": messages,
            "tools": tools,
            "tool_choice": "auto",
        });
        let url = format!("{}/chat/completions", self.base);
        let resp = self
            .http
            .post(url)
            .bearer_auth(&self.key)
            .json(&body)
            .send()
            .await
            .context("llm request")?;
        let status = resp.status();
        let text = resp.text().await.context("llm body")?;
        if !status.is_success() {
            bail!("llm {status}: {}", truncate(&text, 500));
        }
        let parsed: ChatResponse = serde_json::from_str(&text)
            .with_context(|| format!("llm json: {}", truncate(&text, 400)))?;
        let choice = parsed
            .choices
            .into_iter()
            .next()
            .context("llm returned no choices")?;
        let tool_calls = choice
            .message
            .tool_calls
            .unwrap_or_default()
            .into_iter()
            .map(|call| ToolCall {
                id: call.id,
                name: call.function.name,
                arguments: call.function.arguments,
            })
            .collect();
        Ok(ModelMessage {
            content: choice.message.content,
            tool_calls,
        })
    }
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: Assistant,
}

#[derive(Deserialize)]
struct Assistant {
    content: Option<String>,
    tool_calls: Option<Vec<RawToolCall>>,
}

#[derive(Deserialize)]
struct RawToolCall {
    id: String,
    function: RawFunction,
}

#[derive(Deserialize)]
struct RawFunction {
    name: String,
    arguments: String,
}

fn truncate(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}
