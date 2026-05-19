//! `agentic-providers` — LLM provider abstraction.
//!
//! Concrete implementations for Anthropic, OpenAI, Google (Gemini), Mistral,
//! Cohere, Voyage, Ollama and xAI Grok all live in this crate. Selection
//! happens via [`router`] which respects the priority chain documented in
//! ADR-0028:
//!
//!   CLI-context auto-detect > per-task config > project default > user default.
//!
//! Each concrete implementation is a struct that holds its HTTP client +
//! API key (resolved through [`keychain`]) and impls the [`Provider`] trait.

#![warn(clippy::pedantic)]
#![allow(clippy::missing_errors_doc, clippy::module_name_repetitions, dead_code)]

pub mod anthropic;
pub mod cohere;
pub mod google;
pub mod grok;
pub mod keychain;
pub mod mistral;
pub mod ollama;
pub mod openai;
pub mod registry;
pub mod router;
pub mod traits;
pub mod voyage;

use serde::{Deserialize, Serialize};

pub use traits::{
    ChatMessage, ChatRequest, ChatResponse, EmbeddingRequest, EmbeddingResponse, Provider,
    ProviderError, Role,
};

/// Stable provider identifier — one of eight supported back-ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    Anthropic,
    OpenAi,
    Google,
    Mistral,
    Cohere,
    Voyage,
    Ollama,
    Grok,
}

impl ProviderKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
            Self::Google => "google",
            Self::Mistral => "mistral",
            Self::Cohere => "cohere",
            Self::Voyage => "voyage",
            Self::Ollama => "ollama",
            Self::Grok => "grok",
        }
    }

    #[must_use]
    pub fn all() -> [Self; 8] {
        [
            Self::Anthropic,
            Self::OpenAi,
            Self::Google,
            Self::Mistral,
            Self::Cohere,
            Self::Voyage,
            Self::Ollama,
            Self::Grok,
        ]
    }
}

impl std::str::FromStr for ProviderKind {
    type Err = ProviderError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "anthropic" | "claude" => Ok(Self::Anthropic),
            "openai" | "gpt" => Ok(Self::OpenAi),
            "google" | "gemini" => Ok(Self::Google),
            "mistral" => Ok(Self::Mistral),
            "cohere" => Ok(Self::Cohere),
            "voyage" => Ok(Self::Voyage),
            "ollama" | "local" => Ok(Self::Ollama),
            "grok" | "xai" | "x-ai" => Ok(Self::Grok),
            _ => Err(ProviderError::Rejected(format!("unknown provider: {s}"))),
        }
    }
}

/// Task categories. Each task can have its own provider preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Task {
    Chat,
    Judge,
    Embed,
    Extract,
    Classify,
    Translate,
}

impl Task {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Judge => "judge",
            Self::Embed => "embed",
            Self::Extract => "extract",
            Self::Classify => "classify",
            Self::Translate => "translate",
        }
    }
}

/// Route selection result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Route {
    pub kind: ProviderKind,
    pub model: String,
    pub reason: String,
}
