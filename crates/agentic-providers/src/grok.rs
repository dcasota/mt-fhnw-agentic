//! xAI Grok chat-completions client.
//!
//! Endpoint: `POST https://api.x.ai/v1/chat/completions` (OpenAI-compatible).
//! Header: `Authorization: Bearer <key>`.
//!
//! xAI does not currently expose an embeddings endpoint; [`Provider::embed`]
//! returns [`ProviderError::Unimplemented`].
//!
//! Compatible with both the public xAI API and the model exposed by the
//! Grok Build CLI (<https://x.ai/news/grok-build-cli>).

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::traits::{
    ChatRequest, ChatResponse, EmbeddingRequest, EmbeddingResponse, Provider, ProviderError, Role,
};

#[cfg(test)]
use crate::traits::ChatMessage;

const API_BASE: &str = "https://api.x.ai/v1";

pub struct Grok {
    api_key: String,
    base: String,
    client: Client,
}

impl Grok {
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
struct ChatReq<'a> {
    model: &'a str,
    messages: Vec<ChatReqMsg<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    seed: Option<u64>,
}

#[derive(Debug, Serialize)]
struct ChatReqMsg<'a> {
    role: &'static str,
    content: &'a str,
}

#[derive(Debug, Deserialize)]
struct ChatResp {
    choices: Vec<ChatChoice>,
    model: String,
    usage: Usage,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatRespMsg,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatRespMsg {
    content: String,
}

#[derive(Debug, Deserialize)]
struct Usage {
    prompt_tokens: u32,
    completion_tokens: u32,
}

fn build_chat_body<'a>(req: &'a ChatRequest) -> ChatReq<'a> {
    let mut messages = Vec::with_capacity(req.messages.len() + 1);
    if let Some(s) = req.system.as_deref() {
        messages.push(ChatReqMsg {
            role: "system",
            content: s,
        });
    }
    for m in &req.messages {
        messages.push(ChatReqMsg {
            role: match m.role {
                Role::System => "system",
                Role::User => "user",
                Role::Assistant => "assistant",
            },
            content: &m.content,
        });
    }
    ChatReq {
        model: &req.model,
        messages,
        temperature: req.temperature,
        max_tokens: req.max_tokens,
        seed: req.seed,
    }
}

#[async_trait]
impl Provider for Grok {
    fn name(&self) -> &'static str {
        "grok"
    }

    async fn chat(&self, req: &ChatRequest) -> Result<ChatResponse, ProviderError> {
        let url = format!("{}/chat/completions", self.base);
        let body = build_chat_body(req);
        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
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
        let parsed: ChatResp = serde_json::from_str(&text)?;
        let first = parsed
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| ProviderError::Rejected("no choices in response".into()))?;
        Ok(ChatResponse {
            content: first.message.content,
            model: parsed.model,
            tokens_in: parsed.usage.prompt_tokens,
            tokens_out: parsed.usage.completion_tokens,
            finish_reason: first.finish_reason.unwrap_or_else(|| "unknown".into()),
        })
    }

    async fn embed(&self, _req: &EmbeddingRequest) -> Result<EmbeddingResponse, ProviderError> {
        Err(ProviderError::Unimplemented("grok (no embeddings API)"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_chat_body_prepends_system() {
        let req = ChatRequest {
            model: "grok-4".into(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: "hi".into(),
            }],
            temperature: Some(0.0),
            max_tokens: Some(32),
            seed: Some(7),
            system: Some("be terse".into()),
        };
        let body = build_chat_body(&req);
        assert_eq!(body.messages.len(), 2);
        assert_eq!(body.messages[0].role, "system");
        assert_eq!(body.messages[0].content, "be terse");
        assert_eq!(body.seed, Some(7));
    }

    #[test]
    fn parses_minimal_chat_response() {
        let raw = r#"{
            "model":"grok-4",
            "choices":[{"message":{"role":"assistant","content":"hi"},"finish_reason":"stop"}],
            "usage":{"prompt_tokens":5,"completion_tokens":1,"total_tokens":6}
        }"#;
        let parsed: ChatResp = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.usage.completion_tokens, 1);
        assert_eq!(parsed.choices[0].message.content, "hi");
    }
}
