//! Provider registry: resolves a [`ProviderKind`] to a concrete boxed [`Provider`].
//!
//! Reads the API key from the keychain (with env-var fallback) so callers only
//! need to know "I want Anthropic" — the registry figures out the rest.

use std::sync::Arc;

use crate::traits::{Provider, ProviderError};
use crate::{
    ProviderKind, anthropic::Anthropic, cohere::Cohere, google::Google, keychain, mistral::Mistral,
    ollama::Ollama, openai::OpenAi, voyage::Voyage,
};

/// Build a boxed provider for the given kind, looking up its API key as needed.
pub fn build(kind: ProviderKind) -> Result<Arc<dyn Provider>, ProviderError> {
    let key_label = kind.as_str();
    match kind {
        ProviderKind::Ollama => Ok(Arc::new(Ollama::new()) as Arc<dyn Provider>),
        _ => {
            let key = keychain::get_key(key_label)
                .map_err(|e| ProviderError::Rejected(format!("keychain: {e}")))?
                .ok_or(ProviderError::NoApiKey(static_label(kind)))?;
            Ok(match kind {
                ProviderKind::Anthropic => Arc::new(Anthropic::new(key)) as Arc<dyn Provider>,
                ProviderKind::OpenAi => Arc::new(OpenAi::new(key)),
                ProviderKind::Google => Arc::new(Google::new(key)),
                ProviderKind::Mistral => Arc::new(Mistral::new(key)),
                ProviderKind::Cohere => Arc::new(Cohere::new(key)),
                ProviderKind::Voyage => Arc::new(Voyage::new(key)),
                ProviderKind::Ollama => unreachable!(),
            })
        }
    }
}

fn static_label(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Anthropic => "anthropic",
        ProviderKind::OpenAi => "openai",
        ProviderKind::Google => "google",
        ProviderKind::Mistral => "mistral",
        ProviderKind::Cohere => "cohere",
        ProviderKind::Voyage => "voyage",
        ProviderKind::Ollama => "ollama",
    }
}

/// Check whether the given provider currently has credentials available.
#[must_use]
pub fn has_key(kind: ProviderKind) -> bool {
    if matches!(kind, ProviderKind::Ollama) {
        // Ollama needs no key; we treat it as always available. (Actual
        // reachability is verified by `Provider::chat` against the URL.)
        return true;
    }
    matches!(keychain::get_key(kind.as_str()), Ok(Some(ref s)) if !s.is_empty())
}
