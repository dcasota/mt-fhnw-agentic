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
        FactsAction::List {
            project,
            needs_verification,
        } => {
            let facts = passport::current(&conn, &project, Section::VerifiedFacts)?;
            let rows: Vec<(i64, Value)> = facts
                .iter()
                .filter_map(|e| serde_json::from_str::<Value>(&e.payload_json).ok().map(|v| (e.id, v)))
                .filter(|(_, v)| {
                    !needs_verification
                        || v.get("kind").and_then(Value::as_str) == Some("needs_verification")
                })
                .collect();
            if json_out {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&rows.iter().map(|(_, v)| v).collect::<Vec<_>>())?
                );
                return Ok(());
            }
            println!("{} verified fact(s):", rows.len());
            for (id, v) in &rows {
                println!(
                    "  #{id} [{}] {} — source: {}",
                    v.get("kind").and_then(Value::as_str).unwrap_or("?"),
                    v.get("claim").and_then(Value::as_str).unwrap_or("?"),
                    v.get("source").and_then(Value::as_str).unwrap_or("(none)"),
                );
            }
        }
        FactsAction::Verify {
            project,
            id,
            by,
            evidence,
        } => {
            let facts = passport::current(&conn, &project, Section::VerifiedFacts)?;
            let target = facts
                .iter()
                .find(|e| e.id == id)
                .ok_or_else(|| anyhow!("no current verified fact #{id}"))?;
            let mut v: Value = serde_json::from_str(&target.payload_json)?;
            let kind = v.get("kind").and_then(Value::as_str).unwrap_or("measured");
            // Promote needs_verification → measured once a human confirms.
            let new_kind = if kind == "needs_verification" {
                "measured"
            } else {
                kind
            };
            v["kind"] = json!(new_kind);
            v["source"] = json!(format!("HITL sign-off by {by}: {evidence}"));
            v["verified_by"] = json!(by);
            v["verified_at"] = json!(now_utc());
            let head = worktree::head_commit(&conn, &project)?.map(|c| c.sha256);
            let new_id = passport::append(
                &conn,
                &project,
                Section::VerifiedFacts,
                &v.to_string(),
                head.as_deref(),
                Some(id),
            )?;
            println!("Resolved fact #{id} → #{new_id} via HITL sign-off ({by}).");
        }
        FactsAction::Scan { project, prefix } => {
            let existing: std::collections::HashSet<String> =
                passport::current(&conn, &project, Section::VerifiedFacts)?
                    .iter()
                    .filter_map(|e| serde_json::from_str::<Value>(&e.payload_json).ok())
                    .filter_map(|v| v.get("claim").and_then(Value::as_str).map(str::to_string))
                    .collect();
            let head = worktree::head_commit(&conn, &project)?.map(|c| c.sha256);
            let mut enqueued = 0usize;
            for (path, _sha) in worktree::list(&conn, &project, &prefix)? {
                if !path.ends_with(".md") || path.contains("_resolved") {
                    continue;
                }
                let blob = worktree::read_at(&conn, &project, &path)?;
                let text = String::from_utf8_lossy(&blob.content);
                for line in text.lines().filter(|l| l.contains("NEEDS-VERIFICATION")) {
                    let claim = line.trim().trim_start_matches(['`', '#', '*', '>', '-', ' ']).chars().take(120).collect::<String>();
                    if existing.contains(&claim) {
                        continue;
                    }
                    let payload = json!({
                        "claim": claim, "kind": "needs_verification", "value": "",
                        "source": "", "source_path": path, "verified_at": now_utc(),
                    });
                    passport::append(&conn, &project, Section::VerifiedFacts, &payload.to_string(), head.as_deref(), None)?;
                    enqueued += 1;
                }
            }
            println!("Enqueued {enqueued} NEEDS-VERIFICATION marker(s) into the HITL queue.");
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
        // A `needs_verification` placeholder is a HITL queue entry, NOT a source:
        // it must not rescue the number it tracks (the marker stays flagged until
        // resolved). Only sourced facts anchor.
        .filter(|v| v.get("kind").and_then(Value::as_str) != Some("needs_verification"))
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
