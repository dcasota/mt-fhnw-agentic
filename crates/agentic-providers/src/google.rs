//! Google Gemini API.
//!
//! Endpoints:
//!   * `POST https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent`
//!   * `POST https://generativelanguage.googleapis.com/v1beta/models/{embed_model}:embedContent`
//! Header: `x-goog-api-key: <key>`.

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::traits::{
    ChatRequest, ChatResponse, EmbeddingRequest, EmbeddingResponse, Provider, ProviderError, Role,
};

#[cfg(test)]
use crate::traits::ChatMessage;

const API_BASE: &str = "https://generativelanguage.googleapis.com/v1beta";

pub struct Google {
    api_key: String,
    base: String,
    client: Client,
}

impl Google {
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
struct GenReq<'a> {
    contents: Vec<GenContent<'a>>,
    #[serde(rename = "systemInstruction", skip_serializing_if = "Option::is_none")]
    system_instruction: Option<GenContent<'a>>,
    #[serde(rename = "generationConfig")]
    generation_config: GenConfig,
}

#[derive(Debug, Serialize)]
struct GenContent<'a> {
    role: &'static str, // "user" | "model"
    parts: Vec<GenPart<'a>>,
}

#[derive(Debug, Serialize)]
struct GenPart<'a> {
    text: &'a str,
}

#[derive(Debug, Serialize, Default)]
struct GenConfig {
    #[serde(rename = "temperature", skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(rename = "maxOutputTokens", skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct GenResp {
    candidates: Vec<GenCandidate>,
    #[serde(rename = "usageMetadata")]
    usage_metadata: GenUsage,
    #[serde(rename = "modelVersion")]
    model_version: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GenCandidate {
    content: GenCandContent,
    #[serde(rename = "finishReason")]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GenCandContent {
    parts: Vec<GenCandPart>,
}

#[derive(Debug, Deserialize)]
struct GenCandPart {
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GenUsage {
    #[serde(rename = "promptTokenCount")]
    prompt_token_count: u32,
    #[serde(rename = "candidatesTokenCount")]
    candidates_token_count: u32,
}

#[derive(Debug, Serialize)]
struct EmbReq<'a> {
    content: EmbContent<'a>,
}

#[derive(Debug, Serialize)]
struct EmbContent<'a> {
    parts: Vec<EmbPart<'a>>,
}

#[derive(Debug, Serialize)]
struct EmbPart<'a> {
    text: &'a str,
}

#[derive(Debug, Deserialize)]
struct EmbResp {
    embedding: EmbVec,
}

#[derive(Debug, Deserialize)]
struct EmbVec {
    values: Vec<f32>,
}

fn build_gen_body<'a>(req: &'a ChatRequest) -> GenReq<'a> {
    let mut contents = Vec::with_capacity(req.messages.len());
    let mut system_text: Vec<&str> = Vec::new();
    if let Some(s) = req.system.as_deref() {
        system_text.push(s);
    }
    for m in &req.messages {
        match m.role {
            Role::System => system_text.push(&m.content),
            Role::User => contents.push(GenContent {
                role: "user",
                parts: vec![GenPart { text: &m.content }],
            }),
            Role::Assistant => contents.push(GenContent {
                role: "model",
                parts: vec![GenPart { text: &m.content }],
            }),
        }
    }
    let system_instruction = if system_text.is_empty() {
        None
    } else {
        let joined = system_text.join("\n");
        Some(GenContent {
            role: "system",
            parts: vec![GenPart {
                text: Box::leak(joined.into_boxed_str()),
            }],
        })
    };
    GenReq {
        contents,
        system_instruction,
        generation_config: GenConfig {
            temperature: req.temperature,
            max_output_tokens: req.max_tokens,
        },
    }
}

#[async_trait]
impl Provider for Google {
    fn name(&self) -> &'static str {
        "google"
    }

    async fn chat(&self, req: &ChatRequest) -> Result<ChatResponse, ProviderError> {
        let url = format!("{}/models/{}:generateContent", self.base, req.model);
        let body = build_gen_body(req);
        let resp = self
            .client
            .post(&url)
            .header("x-goog-api-key", &self.api_key)
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
        let parsed: GenResp = serde_json::from_str(&text)?;
        let first = parsed
            .candidates
            .into_iter()
            .next()
            .ok_or_else(|| ProviderError::Rejected("no candidates in response".into()))?;
        let content = first
            .content
            .parts
            .into_iter()
            .filter_map(|p| p.text)
            .collect::<Vec<_>>()
            .join("");
        Ok(ChatResponse {
            content,
            model: parsed.model_version.unwrap_or_else(|| req.model.clone()),
            tokens_in: parsed.usage_metadata.prompt_token_count,
            tokens_out: parsed.usage_metadata.candidates_token_count,
            finish_reason: first.finish_reason.unwrap_or_else(|| "unknown".into()),
        })
    }

    async fn embed(&self, req: &EmbeddingRequest) -> Result<EmbeddingResponse, ProviderError> {
        // Gemini embeds one text per call; aggregate locally.
        let mut vectors = Vec::with_capacity(req.texts.len());
        for text in &req.texts {
            let url = format!("{}/models/{}:embedContent", self.base, req.model);
            let body = EmbReq {
                content: EmbContent {
                    parts: vec![EmbPart { text }],
                },
            };
            let resp = self
                .client
                .post(&url)
                .header("x-goog-api-key", &self.api_key)
                .header("content-type", "application/json")
                .json(&body)
                .send()
                .await?;
            let status = resp.status();
            let body_text = resp.text().await?;
            if !status.is_success() {
                return Err(ProviderError::Rejected(format!(
                    "HTTP {status}: {body_text}"
                )));
            }
            let parsed: EmbResp = serde_json::from_str(&body_text)?;
            vectors.push(parsed.embedding.values);
        }
        Ok(EmbeddingResponse {
            model: req.model.clone(),
            vectors,
            tokens_in: 0, // Gemini embed API doesn't return token counts in v1beta.
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_gen_body_maps_assistant_to_model_role() {
        let req = ChatRequest {
            model: "gemini-2.5-pro".into(),
            messages: vec![
                ChatMessage {
                    role: Role::User,
                    content: "hi".into(),
                },
                ChatMessage {
                    role: Role::Assistant,
                    content: "hello".into(),
                },
            ],
            temperature: Some(0.1),
            max_tokens: Some(128),
            seed: None,
            system: Some("be brief".into()),
        };
        let body = build_gen_body(&req);
        assert!(body.system_instruction.is_some());
        assert_eq!(body.contents.len(), 2);
        assert_eq!(body.contents[0].role, "user");
        assert_eq!(body.contents[1].role, "model");
    }

    #[test]
    fn parses_minimal_gen_response() {
        let raw = r#"{
            "candidates":[{"content":{"parts":[{"text":"hi"}]},"finishReason":"STOP"}],
            "usageMetadata":{"promptTokenCount":5,"candidatesTokenCount":1,"totalTokenCount":6},
            "modelVersion":"gemini-2.5-pro-001"
        }"#;
        let parsed: GenResp = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.usage_metadata.candidates_token_count, 1);
        assert_eq!(
            parsed.candidates[0].content.parts[0].text.as_deref(),
            Some("hi")
        );
    }
}
