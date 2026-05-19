//! Provider traits.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("no API key configured for provider {0}")]
    NoApiKey(&'static str),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("provider rejected request: {0}")]
    Rejected(String),

    #[error("rate-limited; retry after {0:?}")]
    RateLimited(Option<std::time::Duration>),

    #[error("serialisation: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("not yet implemented for provider {0}")]
    Unimplemented(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub seed: Option<u64>,
    pub system: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatResponse {
    pub content: String,
    pub model: String,
    pub tokens_in: u32,
    pub tokens_out: u32,
    pub finish_reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingRequest {
    pub model: String,
    pub texts: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingResponse {
    pub model: String,
    pub vectors: Vec<Vec<f32>>,
    pub tokens_in: u32,
}

#[async_trait]
pub trait Provider: Send + Sync {
    fn name(&self) -> &'static str;

    async fn chat(&self, req: &ChatRequest) -> Result<ChatResponse, ProviderError>;

    async fn embed(&self, req: &EmbeddingRequest) -> Result<EmbeddingResponse, ProviderError> {
        let _ = req;
        Err(ProviderError::Unimplemented(self.name()))
    }
}
