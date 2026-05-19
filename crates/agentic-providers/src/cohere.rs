//! Cohere v2 API.
//!
//! Endpoints:
//!   * `POST https://api.cohere.com/v2/chat`
//!   * `POST https://api.cohere.com/v2/embed`
//! Header: `Authorization: Bearer <key>`.

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::traits::{
    ChatRequest, ChatResponse, EmbeddingRequest, EmbeddingResponse, Provider, ProviderError, Role,
};

const API_BASE: &str = "https://api.cohere.com/v2";

pub struct Cohere {
    api_key: String,
    base: String,
    client: Client,
}

impl Cohere {
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
}

#[derive(Debug, Serialize)]
struct ChatReqMsg<'a> {
    role: &'static str,
    content: &'a str,
}

#[derive(Debug, Deserialize)]
struct ChatResp {
    message: ChatRespMsg,
    finish_reason: Option<String>,
    usage: Usage,
}

#[derive(Debug, Deserialize)]
struct ChatRespMsg {
    content: Vec<ChatRespContent>,
}

#[derive(Debug, Deserialize)]
struct ChatRespContent {
    #[serde(rename = "type")]
    kind: String,
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Usage {
    tokens: UsageTokens,
}

#[derive(Debug, Deserialize)]
struct UsageTokens {
    input_tokens: u32,
    output_tokens: u32,
}

#[derive(Debug, Serialize)]
struct EmbReq<'a> {
    model: &'a str,
    texts: &'a [String],
    input_type: &'a str,
    embedding_types: &'a [&'a str],
}

#[derive(Debug, Deserialize)]
struct EmbResp {
    embeddings: EmbBucket,
    meta: Option<EmbMeta>,
}

#[derive(Debug, Deserialize)]
struct EmbBucket {
    float: Vec<Vec<f32>>,
}

#[derive(Debug, Deserialize)]
struct EmbMeta {
    billed_units: Option<EmbBilled>,
}

#[derive(Debug, Deserialize)]
struct EmbBilled {
    input_tokens: Option<u32>,
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
    }
}

#[async_trait]
impl Provider for Cohere {
    fn name(&self) -> &'static str {
        "cohere"
    }

    async fn chat(&self, req: &ChatRequest) -> Result<ChatResponse, ProviderError> {
        let url = format!("{}/chat", self.base);
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
        let content = parsed
            .message
            .content
            .into_iter()
            .filter(|c| c.kind == "text")
            .filter_map(|c| c.text)
            .collect::<Vec<_>>()
            .join("");
        Ok(ChatResponse {
            content,
            model: req.model.clone(),
            tokens_in: parsed.usage.tokens.input_tokens,
            tokens_out: parsed.usage.tokens.output_tokens,
            finish_reason: parsed.finish_reason.unwrap_or_else(|| "unknown".into()),
        })
    }

    async fn embed(&self, req: &EmbeddingRequest) -> Result<EmbeddingResponse, ProviderError> {
        let url = format!("{}/embed", self.base);
        let body = EmbReq {
            model: &req.model,
            texts: &req.texts,
            input_type: "search_document",
            embedding_types: &["float"],
        };
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
            return Err(ProviderError::Rejected(format!("HTTP {status}: {text}")));
        }
        let parsed: EmbResp = serde_json::from_str(&text)?;
        let tokens_in = parsed
            .meta
            .and_then(|m| m.billed_units)
            .and_then(|b| b.input_tokens)
            .unwrap_or(0);
        Ok(EmbeddingResponse {
            model: req.model.clone(),
            vectors: parsed.embeddings.float,
            tokens_in,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_chat_response() {
        let raw = r#"{
            "message":{"role":"assistant","content":[{"type":"text","text":"hi"}]},
            "finish_reason":"COMPLETE",
            "usage":{"tokens":{"input_tokens":4,"output_tokens":1}}
        }"#;
        let parsed: ChatResp = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.usage.tokens.output_tokens, 1);
        assert_eq!(parsed.message.content[0].text.as_deref(), Some("hi"));
    }

    #[test]
    fn parses_embed_response() {
        let raw = r#"{
            "embeddings":{"float":[[0.1,0.2]]},
            "meta":{"billed_units":{"input_tokens":2}}
        }"#;
        let parsed: EmbResp = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.embeddings.float.len(), 1);
    }
}
