use anyhow::Result;
use serde_json::json;

use agentic_providers::{ProviderKind, keychain, registry};

pub fn run(json_out: bool) -> Result<()> {
    let cargo_pkg_version = env!("CARGO_PKG_VERSION");
    let target = env!("TARGET_TRIPLE");
    let host_os = std::env::consts::OS;
    let host_arch = std::env::consts::ARCH;
    let detected_cli = detect_cli_context();
    let providers = providers_detail();

    if json_out {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "version": cargo_pkg_version,
                "target": target,
                "host": { "os": host_os, "arch": host_arch },
                "detected_cli_context": detected_cli,
                "providers": providers,
            }))?
        );
    } else {
        println!("agentic {cargo_pkg_version}");
        println!("  Target:        {target}");
        println!("  Host:          {host_os}/{host_arch}");
        println!(
            "  CLI detected:  {}",
            detected_cli.unwrap_or_else(|| "none".into())
        );
        println!("  Providers:");
        for p in &providers {
            let configured = p["configured"].as_bool().unwrap_or(false);
            let source = p["source"].as_str().unwrap_or("");
            let marker = if configured { "✓" } else { "·" };
            let src_suffix = if configured && !source.is_empty() {
                format!(" (via {source})")
            } else {
                String::new()
            };
            println!(
                "    {marker} {:<10}{src_suffix}",
                p["provider"].as_str().unwrap_or("?")
            );
        }
    }
    Ok(())
}

fn detect_cli_context() -> Option<String> {
    if std::env::var("CLAUDECODE").is_ok() {
        return Some("claude-code".into());
    }
    if std::env::var("CURSOR_TRACE_ID").is_ok() {
        return Some("cursor".into());
    }
    if std::env::var("GEMINI_CLI").is_ok() {
        return Some("gemini-cli".into());
    }
    if std::env::var("CODEX_SESSION").is_ok() {
        return Some("openai-codex".into());
    }
    if std::env::var("FACTORYAI").is_ok() {
        return Some("factory-ai".into());
    }
    if std::env::var("GROK_BUILD").is_ok() || std::env::var("XAI_BUILD").is_ok() {
        return Some("grok-build".into());
    }
    None
}

/// Per-provider key status: which source (if any) produced the hit,
/// plus the env-var names that were checked. Used by both JSON and human modes.
fn providers_detail() -> Vec<serde_json::Value> {
    ProviderKind::all()
        .iter()
        .map(|kind| {
            let p = kind.as_str();
            if matches!(kind, ProviderKind::Ollama) {
                // Ollama is keyless; report reachability env override.
                let host = std::env::var("OLLAMA_HOST")
                    .unwrap_or_else(|_| "http://localhost:11434".into());
                return json!({
                    "provider": p,
                    "configured": registry::has_key(*kind),
                    "source": "no-key (local)",
                    "host": host,
                });
            }
            let agentic_env = keychain::env_var_name(p);
            let vendor_env = keychain::vendor_env_var_name(p).unwrap_or("");
            let (configured, source) = match keychain::get_key_with_source(p) {
                Ok(Some((_, src))) => (true, src.as_str().to_owned()),
                _ => (false, String::new()),
            };
            json!({
                "provider": p,
                "configured": configured,
                "source": source,
                "env_vars": {
                    "agentic": agentic_env,
                    "vendor": vendor_env,
                }
            })
        })
        .collect()
}
