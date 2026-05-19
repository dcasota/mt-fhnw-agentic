//! `agentic config …` — provider keys + (eventually) project-scoped settings.

use std::io::Read;

use anyhow::{Context, Result, anyhow};
use serde_json::json;

use agentic_providers::{ProviderKind, keychain};

use crate::cli::ConfigAction;

pub fn run(action: ConfigAction, json: bool) -> Result<()> {
    match action {
        ConfigAction::SetKey { provider, value } => set_key(&provider, &value, json),
        ConfigAction::UnsetKey { provider } => unset_key(&provider, json),
        ConfigAction::WhereKey { provider } => where_key(&provider, json),
    }
}

fn canon(provider: &str) -> Result<ProviderKind> {
    provider
        .parse::<ProviderKind>()
        .map_err(|e| anyhow!("invalid provider '{provider}': {e}"))
}

fn set_key(provider: &str, value: &str, json: bool) -> Result<()> {
    let kind = canon(provider)?;
    let actual = if value == "-" {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        buf.trim().to_owned()
    } else {
        value.to_owned()
    };
    if actual.is_empty() {
        return Err(anyhow!("refusing to set empty key for {}", kind.as_str()));
    }
    keychain::set_key(kind.as_str(), &actual).context("write to OS keychain")?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "provider": kind.as_str(),
                "stored": "keychain",
            }))?
        );
    } else {
        println!("Stored {} key in OS keychain.", kind.as_str());
        println!(
            "(env-var override available: {}=...)",
            keychain::env_var_name(kind.as_str())
        );
    }
    Ok(())
}

fn unset_key(provider: &str, json: bool) -> Result<()> {
    let kind = canon(provider)?;
    keychain::delete_key(kind.as_str()).context("delete from OS keychain")?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "provider": kind.as_str(),
                "removed": true,
            }))?
        );
    } else {
        println!("Removed {} key from OS keychain.", kind.as_str());
    }
    Ok(())
}

fn where_key(provider: &str, json: bool) -> Result<()> {
    let kind = canon(provider)?;
    let env_name = keychain::env_var_name(kind.as_str());
    let from_env = std::env::var(&env_name)
        .map(|v| !v.is_empty())
        .unwrap_or(false);
    let from_keychain =
        matches!(keychain::get_key(kind.as_str()), Ok(Some(ref s)) if !s.is_empty());
    let source = if from_env {
        "env"
    } else if from_keychain {
        "keychain"
    } else {
        "missing"
    };
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "provider": kind.as_str(),
                "source": source,
                "env_var": env_name,
            }))?
        );
    } else {
        println!("provider: {}", kind.as_str());
        println!("source:   {source}");
        println!("env_var:  {env_name}");
    }
    Ok(())
}
