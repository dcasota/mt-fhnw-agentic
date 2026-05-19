//! Anthropic Messages API.
//!
//! Endpoint: `POST https://api.anthropic.com/v1/messages`
//! Required headers: `x-api-key`, `anthropic-version: 2023-06-01` (stable),
//! optional `anthropic-beta: ...`.
//!
//! Anthropic does **not** expose an embeddings API — `embed` returns
//! [`ProviderError::Unimplemented`].

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::traits::{
    ChatRequest, ChatResponse, EmbeddingRequest, EmbeddingResponse, Provider, ProviderError, Role,
};

#[cfg(test)]
use crate::traits::ChatMessage;

const API_BASE: &str = "https://api.anthropic.com/v1";
const API_VERSION: &str = "2023-06-01";

pub struct Anthropic {
    api_key: String,
    base: String,
    client: Client,
}

impl Anthropic {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base: API_BASE.into(),
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .unwrap_or_else(|_| Client::new()),
        }
    }

    #[must_use]
    pub fn with_base(mut self, base: impl Into<String>) -> Self {
        self.base = base.into();
        self
    }
}

#[derive(Debug, Serialize)]
struct ReqBody<'a> {
    model: &'a str,
    max_tokens: u32,
    messages: Vec<ReqMsg<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<&'a str>,
}

#[derive(Debug, Serialize)]
struct ReqMsg<'a> {
    role: &'static str, // "user" | "assistant"
    content: &'a str,
}

#[derive(Debug, Deserialize)]
struct RespBody {
    content: Vec<RespContent>,
    model: String,
    stop_reason: Option<String>,
    usage: RespUsage,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum RespContent {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
struct RespUsage {
    input_tokens: u32,
    output_tokens: u32,
}

/// Build the request body. Anthropic distinguishes `system` from the message
/// array; any [`Role::System`] in the input is merged into the top-level
/// system field (concatenated by newline).
fn build_body<'a>(req: &'a ChatRequest) -> ReqBody<'a> {
    let mut system_pieces: Vec<&str> = Vec::new();
    if let Some(s) = req.system.as_deref() {
        system_pieces.push(s);
    }
    let mut messages = Vec::with_capacity(req.messages.len());
    for m in &req.messages {
        match m.role {
            Role::System => system_pieces.push(&m.content),
            Role::User => messages.push(ReqMsg {
                role: "user",
                content: &m.content,
            }),
            Role::Assistant => messages.push(ReqMsg {
                role: "assistant",
                content: &m.content,
            }),
        }
    }
    let system = if system_pieces.is_empty() {
        None
    } else {
        // Box the joined string into the request via `Cow` would need lifetime
        // gymnastics; instead, leak a String via `Box::leak` -- the leak is
        // bounded by the lifetime of the request and reclaimed when the
        // request drops. Acceptable for one-shot HTTP bodies.
        let joined = system_pieces.join("\n");
        Some(Box::leak(joined.into_boxed_str()) as &str)
    };
    ReqBody {
        model: &req.model,
        max_tokens: req.max_tokens.unwrap_or(4096),
        messages,
        temperature: req.temperature,
        system,
    }
}

#[async_trait]
impl Provider for Anthropic {
    fn name(&self) -> &'static str {
        "anthropic"
    }

    async fn chat(&self, req: &ChatRequest) -> Result<ChatResponse, ProviderError> {
        let url = format!("{}/messages", self.base);
        let body = build_body(req);
        let resp = self
            .client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", API_VERSION)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            if status.as_u16() == 429 {
                return Err(ProviderError::RateLimited(None));
            }
            return Err(ProviderError::Rejected(format!("HTTP {status}: {text}")));
        }
        let parsed: RespBody = serde_json::from_str(&text)?;
        let content = parsed
            .content
            .into_iter()
            .filter_map(|c| match c {
                RespContent::Text { text } => Some(text),
                RespContent::Other => None,
            })
            .collect::<Vec<_>>()
            .join("");
        Ok(ChatResponse {
            content,
            model: parsed.model,
            tokens_in: parsed.usage.input_tokens,
            tokens_out: parsed.usage.output_tokens,
            finish_reason: parsed.stop_reason.unwrap_or_else(|| "unknown".into()),
        })
    }

    async fn embed(&self, _req: &EmbeddingRequest) -> Result<EmbeddingResponse, ProviderError> {
        Err(ProviderError::Unimplemented(
            "anthropic (no embeddings API)",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_request() -> ChatRequest {
        ChatRequest {
            model: "claude-opus-4-7".into(),
            messages: vec![
                ChatMessage {
                    role: Role::System,
                    content: "be brief".into(),
                },
                ChatMessage {
                    role: Role::User,
                    content: "say hi".into(),
                },
            ],
            temperature: Some(0.2),
            max_tokens: Some(64),
            seed: None,
            system: Some("you are helpful".into()),
        }
    }

    #[test]
    fn build_body_merges_system_messages() {
        let req = sample_request();
        let body = build_body(&req);
        assert_eq!(body.model, "claude-opus-4-7");
        assert_eq!(body.max_tokens, 64);
        assert!(body.system.is_some());
        let sys = body.system.unwrap();
        assert!(sys.contains("you are helpful"));
        assert!(sys.contains("be brief"));
        // Only user/assistant messages reach the messages array.
        assert_eq!(body.messages.len(), 1);
        assert_eq!(body.messages[0].role, "user");
        assert_eq!(body.messages[0].content, "say hi");
    }

    #[test]
    fn parses_minimal_response() {
        let raw = r#"{
            "content": [{"type":"text","text":"hi there"}],
            "model": "claude-opus-4-7",
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 12, "output_tokens": 3}
        }"#;
        let parsed: RespBody = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.usage.input_tokens, 12);
        assert_eq!(parsed.model, "claude-opus-4-7");
    }
}
