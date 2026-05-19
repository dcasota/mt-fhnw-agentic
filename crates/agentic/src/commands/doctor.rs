use anyhow::Result;
use serde_json::json;

pub fn run(json_out: bool) -> Result<()> {
    let cargo_pkg_version = env!("CARGO_PKG_VERSION");
    let target = env!("TARGET_TRIPLE");
    let host_os = std::env::consts::OS;
    let host_arch = std::env::consts::ARCH;
    let detected_cli = detect_cli_context();
    let api_keys = detect_api_keys();
    if json_out {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "version": cargo_pkg_version,
                "target": target,
                "host": { "os": host_os, "arch": host_arch },
                "detected_cli_context": detected_cli,
                "api_keys_set": api_keys,
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
        println!(
            "  API keys set:  {}",
            if api_keys.is_empty() {
                "(none)".into()
            } else {
                api_keys.join(", ")
            }
        );
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

fn detect_api_keys() -> Vec<String> {
    let mut keys = Vec::new();
    for (var, label) in [
        ("AGENTIC_ANTHROPIC_KEY", "anthropic"),
        ("AGENTIC_OPENAI_KEY", "openai"),
        ("AGENTIC_GOOGLE_KEY", "google"),
        ("AGENTIC_MISTRAL_KEY", "mistral"),
        ("AGENTIC_COHERE_KEY", "cohere"),
        ("AGENTIC_VOYAGE_KEY", "voyage"),
        ("AGENTIC_GROK_KEY", "grok"),
        ("OLLAMA_HOST", "ollama"),
    ] {
        if std::env::var(var).is_ok() {
            keys.push(label.into());
        }
    }
    keys
}
