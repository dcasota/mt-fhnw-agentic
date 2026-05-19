//! Voyage AI — embeddings only.
//!
//! Endpoint: `POST https://api.voyageai.com/v1/embeddings`.
//! Header: `Authorization: Bearer <key>`.
//!
//! Voyage does not currently offer a chat API; [`Provider::chat`] returns
//! [`ProviderError::Unimplemented`].

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::traits::{
    ChatRequest, ChatResponse, EmbeddingRequest, EmbeddingResponse, Provider, ProviderError,
};

const API_BASE: &str = "https://api.voyageai.com/v1";

pub struct Voyage {
    api_key: String,
    base: String,
    client: Client,
}

impl Voyage {
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
struct EmbReq<'a> {
    input: &'a [String],
    model: &'a str,
    input_type: &'a str,
}

#[derive(Debug, Deserialize)]
struct EmbResp {
    model: String,
    data: Vec<EmbDatum>,
    usage: EmbUsage,
}

#[derive(Debug, Deserialize)]
struct EmbDatum {
    embedding: Vec<f32>,
}

#[derive(Debug, Deserialize)]
struct EmbUsage {
    total_tokens: u32,
}

#[async_trait]
impl Provider for Voyage {
    fn name(&self) -> &'static str {
        "voyage"
    }

    async fn chat(&self, _req: &ChatRequest) -> Result<ChatResponse, ProviderError> {
        Err(ProviderError::Unimplemented("voyage (no chat API)"))
    }

    async fn embed(&self, req: &EmbeddingRequest) -> Result<EmbeddingResponse, ProviderError> {
        let url = format!("{}/embeddings", self.base);
        let body = EmbReq {
            input: &req.texts,
            model: &req.model,
            input_type: "document",
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
        Ok(EmbeddingResponse {
            model: parsed.model,
            vectors: parsed.data.into_iter().map(|d| d.embedding).collect(),
            tokens_in: parsed.usage.total_tokens,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_embed_response() {
        let raw = r#"{
            "model":"voyage-3",
            "data":[{"embedding":[0.1,0.2,0.3]}],
            "usage":{"total_tokens":4}
        }"#;
        let parsed: EmbResp = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.data.len(), 1);
        assert_eq!(parsed.usage.total_tokens, 4);
    }
}
