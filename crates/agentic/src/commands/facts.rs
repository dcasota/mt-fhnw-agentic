//! `agentic facts` — verified-facts backbone (ADR-0016 / ADR-0042).
//!
//! Anchors a recurring claim (measured count, model estimate, build artefact,
//! external stat) to one provenance-bearing record so the deliverable gate
//! resolves the number against a signed record instead of a regex. A fact
//! without a real `source` is rejected (ADR-0036: never invent).

use anyhow::{Result, anyhow};
use serde_json::{Value, json};

use agentic_core::passport::{self, Section};
use agentic_core::worktree;

use crate::cli::FactsAction;

const KINDS: &[&str] = &[
    "measured",
    "model_estimate",
    "build_artifact",
    "external_stat",
    "needs_verification",
];

pub fn run(db_path: &std::path::Path, action: FactsAction, json_out: bool) -> Result<()> {
    let conn = agentic_core::db::open(db_path)?;
    match action {
        FactsAction::Add {
            project,
            claim,
            kind,
            source,
            value,
        } => {
            if !KINDS.contains(&kind.as_str()) {
                return Err(anyhow!("unknown kind '{kind}'; one of {KINDS:?}"));
            }
            // ADR-0036: a verified fact must carry a real source — except a
            // `needs_verification` placeholder, which is explicitly unsourced
            // pending HITL (ADR-0017) and is allowed so it can sit in the queue.
            if kind != "needs_verification" && source.trim().is_empty() {
                return Err(anyhow!(
                    "ADR-0036: a verified fact requires a non-empty --source (DOI/URL/manifest/RAMP-run/HITL); use --kind needs_verification for an unresolved placeholder"
                ));
            }
            let head = worktree::head_commit(&conn, &project)?.map(|c| c.sha256);
            let payload = json!({
                "claim": claim,
                "kind": kind,
                "value": value.unwrap_or_default(),
                "source": source,
                "verified_at": now_utc(),
            });
            let id = passport::append(
                &conn,
                &project,
                Section::VerifiedFacts,
                &payload.to_string(),
                head.as_deref(),
                None,
            )?;
            if json_out {
                println!("{}", json!({ "id": id, "claim": claim, "kind": kind }));
            } else {
                println!("Anchored verified fact #{id} [{kind}]: \"{claim}\" (bound to HEAD)");
            }
        }
        FactsAction::List { project } => {
            let facts = passport::current(&conn, &project, Section::VerifiedFacts)?;
            if json_out {
                let arr: Vec<Value> = facts
                    .iter()
                    .filter_map(|e| serde_json::from_str(&e.payload_json).ok())
                    .collect();
                println!("{}", serde_json::to_string_pretty(&arr)?);
                return Ok(());
            }
            println!("{} verified fact(s):", facts.len());
            for e in &facts {
                if let Ok(v) = serde_json::from_str::<Value>(&e.payload_json) {
                    println!(
                        "  [{}] {} — source: {}",
                        v.get("kind").and_then(Value::as_str).unwrap_or("?"),
                        v.get("claim").and_then(Value::as_str).unwrap_or("?"),
                        v.get("source").and_then(Value::as_str).unwrap_or("(none)"),
                    );
                }
            }
        }
    }
    Ok(())
}

/// Claim strings of all current verified facts (used by the deliverable gate to
/// treat a matching numeric line as already-sourced).
pub fn anchored_claims(
    conn: &rusqlite::Connection,
    project: &str,
) -> Result<Vec<String>> {
    let facts = passport::current(conn, project, Section::VerifiedFacts)?;
    Ok(facts
        .iter()
        .filter_map(|e| serde_json::from_str::<Value>(&e.payload_json).ok())
        .filter_map(|v| {
            v.get("claim")
                .and_then(Value::as_str)
                .map(str::to_string)
                .filter(|s| !s.is_empty())
        })
        .collect())
}

fn now_utc() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}
