//! agentic-providers — LLM provider abstraction (P2+).
//!
//! P0 ships only the trait skeleton and provider enum. Real implementations
//! land in P2 (claim audit) and P5 (proposal import) phases.

#![warn(clippy::pedantic)]
#![allow(dead_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Anthropic,
    OpenAi,
    Google,
    Mistral,
    Cohere,
    Voyage,
    Ollama,
}

impl Provider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
            Self::Google => "google",
            Self::Mistral => "mistral",
            Self::Cohere => "cohere",
            Self::Voyage => "voyage",
            Self::Ollama => "ollama",
        }
    }
}

/// Task categories. Each task can have its own provider preference (per ADR-0017).
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

/// Route selection result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Route {
    pub provider: Provider,
    pub model:    String,
    pub reason:   String,
}
