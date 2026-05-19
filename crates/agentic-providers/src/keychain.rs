//! Read API keys from OS keychain or environment variables.
//!
//! Lookup order (first hit wins):
//!   1. `AGENTIC_<PROVIDER>_KEY` env var — explicit override.
//!   2. Vendor-native env var — `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`,
//!      `GOOGLE_API_KEY`, `MISTRAL_API_KEY`, `COHERE_API_KEY`,
//!      `VOYAGE_API_KEY`, `XAI_API_KEY`. These match the conventions
//!      published by each vendor's official SDK, so users who have
//!      already set them for vendor CLIs get zero-config integration.
//!   3. OS keychain entry under service `"agentic"`, account `<provider>`.
//!
//! Ollama is keyless and has no env var.

use anyhow::Result;

const SERVICE: &str = "agentic";

/// Vendor-native env-var names indexed by provider key.
const VENDOR_ENV: &[(&str, &str)] = &[
    ("anthropic", "ANTHROPIC_API_KEY"),
    ("openai", "OPENAI_API_KEY"),
    ("google", "GOOGLE_API_KEY"),
    ("mistral", "MISTRAL_API_KEY"),
    ("cohere", "COHERE_API_KEY"),
    ("voyage", "VOYAGE_API_KEY"),
    ("grok", "XAI_API_KEY"),
];

/// Identifies which env-var path produced a hit, for diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
    AgenticEnv,
    VendorEnv,
    Keychain,
}

impl KeySource {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AgenticEnv => "AGENTIC_env",
            Self::VendorEnv => "vendor_env",
            Self::Keychain => "keychain",
        }
    }
}

/// The `AGENTIC_<PROVIDER>_KEY` env var name (override path).
#[must_use]
pub fn env_var_name(provider: &str) -> String {
    format!("AGENTIC_{}_KEY", provider.to_uppercase())
}

/// The vendor-native env var name for a provider, if one exists.
#[must_use]
pub fn vendor_env_var_name(provider: &str) -> Option<&'static str> {
    VENDOR_ENV
        .iter()
        .find(|(p, _)| *p == provider)
        .map(|(_, v)| *v)
}

pub fn get_key(provider: &str) -> Result<Option<String>> {
    Ok(get_key_with_source(provider)?.map(|(v, _)| v))
}

/// Like [`get_key`] but also returns which source produced the hit.
pub fn get_key_with_source(provider: &str) -> Result<Option<(String, KeySource)>> {
    // 1. AGENTIC_<PROVIDER>_KEY override.
    if let Ok(v) = std::env::var(env_var_name(provider)) {
        if !v.is_empty() {
            return Ok(Some((v, KeySource::AgenticEnv)));
        }
    }
    // 2. Vendor-native env var.
    if let Some(vendor_var) = vendor_env_var_name(provider) {
        if let Ok(v) = std::env::var(vendor_var) {
            if !v.is_empty() {
                return Ok(Some((v, KeySource::VendorEnv)));
            }
        }
    }
    // 3. OS keychain.
    let entry = keyring::Entry::new(SERVICE, provider)?;
    match entry.get_password() {
        Ok(v) => Ok(Some((v, KeySource::Keychain))),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub fn set_key(provider: &str, value: &str) -> Result<()> {
    let entry = keyring::Entry::new(SERVICE, provider)?;
    entry.set_password(value)?;
    Ok(())
}

pub fn delete_key(provider: &str) -> Result<()> {
    let entry = keyring::Entry::new(SERVICE, provider)?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vendor_env_table_covers_all_keyed_providers() {
        // Every provider that needs a key has a vendor-native env-var name.
        for p in [
            "anthropic",
            "openai",
            "google",
            "mistral",
            "cohere",
            "voyage",
            "grok",
        ] {
            assert!(
                vendor_env_var_name(p).is_some(),
                "missing vendor env var for {p}"
            );
        }
        // Ollama is keyless, not in the table.
        assert!(vendor_env_var_name("ollama").is_none());
    }

    #[test]
    fn env_var_name_uppercases() {
        assert_eq!(env_var_name("anthropic"), "AGENTIC_ANTHROPIC_KEY");
        assert_eq!(env_var_name("grok"), "AGENTIC_GROK_KEY");
    }
}
