//! `agentic verify cross-model` — independent 2nd-model attestation (ADR-0028).
//!
//! Routes a verified fact to a *different* provider to re-derive/confirm its
//! value, then records the attestation on the fact (superseding it). Requires a
//! configured second provider at runtime; if none is available it degrades
//! gracefully (records a deferral — never a fabricated agreement, ADR-0036).

use anyhow::{Result, anyhow};
use serde_json::{Value, json};

use agentic_core::passport::{self, Section};
use agentic_core::worktree;
use agentic_providers::ProviderKind;
use agentic_providers::registry;
use agentic_providers::traits::{ChatMessage, ChatRequest, Role};

use crate::cli::VerifyAction;

const CLOUD: &[ProviderKind] = &[
    ProviderKind::Anthropic,
    ProviderKind::OpenAi,
    ProviderKind::Google,
    ProviderKind::Mistral,
    ProviderKind::Cohere,
    ProviderKind::Grok,
];

fn parse_kind(s: &str) -> Option<ProviderKind> {
    Some(match s.to_lowercase().as_str() {
        "anthropic" => ProviderKind::Anthropic,
        "openai" => ProviderKind::OpenAi,
        "google" => ProviderKind::Google,
        "mistral" => ProviderKind::Mistral,
        "cohere" => ProviderKind::Cohere,
        "grok" => ProviderKind::Grok,
        "ollama" => ProviderKind::Ollama,
        _ => return None,
    })
}

pub async fn run(db_path: &std::path::Path, action: VerifyAction, _json_out: bool) -> Result<()> {
    let VerifyAction::CrossModel {
        project,
        fact,
        provider,
        model,
    } = action;
    let conn = agentic_core::db::open(db_path)?;
    let facts = passport::current(&conn, &project, Section::VerifiedFacts)?;
    let target = facts
        .iter()
        .find(|e| e.id == fact)
        .ok_or_else(|| anyhow!("no current verified fact #{fact}"))?;
    let v: Value = serde_json::from_str(&target.payload_json)?;
    let claim = v.get("claim").and_then(Value::as_str).unwrap_or("");
    let value = v.get("value").and_then(Value::as_str).unwrap_or("");

    // Select a *configured* second provider.
    let kind = match &provider {
        Some(p) => parse_kind(p).ok_or_else(|| anyhow!("unknown provider '{p}'"))?,
        None => match CLOUD.iter().copied().find(|k| registry::has_key(*k)) {
            Some(k) => k,
            None => {
                record_deferral(&conn, &project, fact, &v, "no second provider configured (set AGENTIC_<P>_KEY)")?;
                println!("cross-model attestation DEFERRED for fact #{fact}: no second provider configured.");
                return Ok(());
            }
        },
    };
    let Some(model) = model else {
        return Err(anyhow!("--model is required for provider {}", kind.as_str()));
    };

    let prov = match registry::build(kind) {
        Ok(p) => p,
        Err(e) => {
            record_deferral(&conn, &project, fact, &v, &format!("provider build failed: {e}"))?;
            println!("cross-model attestation DEFERRED for fact #{fact}: {e}");
            return Ok(());
        }
    };
    let req = ChatRequest {
        model: model.clone(),
        messages: vec![ChatMessage {
            role: Role::User,
            content: format!(
                "Independently verify this factual claim — do not guess.\nClaim: \"{claim}\"\nStated value: \"{value}\"\nReply on one line: AGREE or DISAGREE or UNKNOWN, then the value you find."
            ),
        }],
        temperature: Some(0.0),
        max_tokens: Some(200),
        seed: None,
        system: Some("You are an independent fact-checker.".into()),
    };
    match prov.chat(&req).await {
        Ok(resp) => {
            let up = resp.content.to_uppercase();
            let verdict = if up.contains("DISAGREE") {
                "disagree"
            } else if up.contains("AGREE") {
                "agree"
            } else {
                "unknown"
            };
            let mut nv = v.clone();
            nv["cross_model"] = json!({
                "provider": kind.as_str(), "model": resp.model, "verdict": verdict,
                "response": resp.content.chars().take(240).collect::<String>(),
            });
            let head = worktree::head_commit(&conn, &project)?.map(|c| c.sha256);
            let id = passport::append(&conn, &project, Section::VerifiedFacts, &nv.to_string(), head.as_deref(), Some(fact))?;
            println!("Cross-model ({}) verdict on fact #{fact}: {verdict} → #{id}", kind.as_str());
        }
        Err(e) => {
            record_deferral(&conn, &project, fact, &v, &format!("provider error: {e}"))?;
            println!("cross-model attestation DEFERRED for fact #{fact}: {e}");
        }
    }
    Ok(())
}

fn record_deferral(
    conn: &rusqlite::Connection,
    project: &str,
    fact: i64,
    v: &Value,
    why: &str,
) -> Result<()> {
    let mut nv = v.clone();
    nv["cross_model"] = json!({ "verdict": "deferred", "reason": why });
    let head = worktree::head_commit(conn, project)?.map(|c| c.sha256);
    passport::append(conn, project, Section::VerifiedFacts, &nv.to_string(), head.as_deref(), Some(fact))?;
    Ok(())
}
