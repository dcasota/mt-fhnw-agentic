//! Read API keys from OS keychain (preferred) with env-var fallback.
//!
//! Lookup order:
//!   1. `AGENTIC_<PROVIDER>_KEY` env var (highest)
//!   2. OS keychain entry under service `"agentic"`, account `<provider>`
//!
//! The fallback to env vars is intentional: in CI / containers / dev machines,
//! env vars are convenient. In a user's daily setup, the keychain is more secure.

use anyhow::Result;

const SERVICE: &str = "agentic";

#[must_use]
pub fn env_var_name(provider: &str) -> String {
    format!("AGENTIC_{}_KEY", provider.to_uppercase())
}

pub fn get_key(provider: &str) -> Result<Option<String>> {
    if let Ok(v) = std::env::var(env_var_name(provider)) {
        if !v.is_empty() {
            return Ok(Some(v));
        }
    }
    let entry = keyring::Entry::new(SERVICE, provider)?;
    match entry.get_password() {
        Ok(v) => Ok(Some(v)),
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
