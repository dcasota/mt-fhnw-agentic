//! Ollama (local LLM runtime).
//!
//! Endpoints (default `http://localhost:11434`):
//!   * `POST /api/chat`
//!   * `POST /api/embed`
//!
//! No API key required. Override host via env: `OLLAMA_HOST=http://hostname:11434`.

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::traits::{
    ChatRequest, ChatResponse, EmbeddingRequest, EmbeddingResponse, Provider, ProviderError, Role,
};

#[cfg(test)]
use crate::traits::ChatMessage;

const DEFAULT_BASE: &str = "http://localhost:11434";

pub struct Ollama {
    base: String,
    client: Client,
}

impl Default for Ollama {
    fn default() -> Self {
        Self::new()
    }
}

impl Ollama {
    #[must_use]
    pub fn new() -> Self {
        let base = std::env::var("OLLAMA_HOST").unwrap_or_else(|_| DEFAULT_BASE.into());
        Self {
            base,
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(300))
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
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    options: Option<ChatOptions>,
}

#[derive(Debug, Serialize)]
struct ChatReqMsg<'a> {
    role: &'static str,
    content: &'a str,
}

#[derive(Debug, Serialize, Default)]
struct ChatOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "num_predict")]
    num_predict: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    seed: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct ChatResp {
    model: String,
    message: ChatRespMsg,
    done: bool,
    done_reason: Option<String>,
    prompt_eval_count: Option<u32>,
    eval_count: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct ChatRespMsg {
    content: String,
}

#[derive(Debug, Serialize)]
struct EmbReq<'a> {
    model: &'a str,
    input: &'a [String],
}

#[derive(Debug, Deserialize)]
struct EmbResp {
    model: String,
    embeddings: Vec<Vec<f32>>,
    prompt_eval_count: Option<u32>,
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
    let options = if req.temperature.is_some() || req.max_tokens.is_some() || req.seed.is_some() {
        Some(ChatOptions {
            temperature: req.temperature,
            num_predict: req.max_tokens,
            seed: req.seed,
        })
    } else {
        None
    };
    ChatReq {
        model: &req.model,
        messages,
        stream: false,
        options,
    }
}

#[async_trait]
impl Provider for Ollama {
    fn name(&self) -> &'static str {
        "ollama"
    }

    async fn chat(&self, req: &ChatRequest) -> Result<ChatResponse, ProviderError> {
        let url = format!("{}/api/chat", self.base);
        let body = build_chat_body(req);
        let resp = self.client.post(&url).json(&body).send().await?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            return Err(ProviderError::Rejected(format!("HTTP {status}: {text}")));
        }
        let parsed: ChatResp = serde_json::from_str(&text)?;
        Ok(ChatResponse {
            content: parsed.message.content,
            model: parsed.model,
            tokens_in: parsed.prompt_eval_count.unwrap_or(0),
            tokens_out: parsed.eval_count.unwrap_or(0),
            finish_reason: parsed.done_reason.unwrap_or_else(|| {
                if parsed.done {
                    "stop".into()
                } else {
                    "incomplete".into()
                }
            }),
        })
    }

    async fn embed(&self, req: &EmbeddingRequest) -> Result<EmbeddingResponse, ProviderError> {
        let url = format!("{}/api/embed", self.base);
        let body = EmbReq {
            model: &req.model,
            input: &req.texts,
        };
        let resp = self.client.post(&url).json(&body).send().await?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            return Err(ProviderError::Rejected(format!("HTTP {status}: {text}")));
        }
        let parsed: EmbResp = serde_json::from_str(&text)?;
        Ok(EmbeddingResponse {
            model: parsed.model,
            vectors: parsed.embeddings,
            tokens_in: parsed.prompt_eval_count.unwrap_or(0),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_chat_body_omits_options_when_unset() {
        let req = ChatRequest {
            model: "llama3:latest".into(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: "hi".into(),
            }],
            temperature: None,
            max_tokens: None,
            seed: None,
            system: None,
        };
        let body = build_chat_body(&req);
        assert!(body.options.is_none());
        let json = serde_json::to_string(&body).unwrap();
        assert!(json.contains("\"stream\":false"));
    }

    #[test]
    fn parses_chat_response() {
        let raw = r#"{
            "model":"llama3:latest",
            "message":{"role":"assistant","content":"hi"},
            "done":true,
            "done_reason":"stop",
            "prompt_eval_count":5,
            "eval_count":1
        }"#;
        let parsed: ChatResp = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.eval_count, Some(1));
    }

    #[test]
    fn parses_embed_response() {
        let raw = r#"{"model":"llama3:latest","embeddings":[[0.1,0.2]],"prompt_eval_count":2}"#;
        let parsed: EmbResp = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.embeddings.len(), 1);
    }
}
