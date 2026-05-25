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
                .filter_map(|e| {
                    serde_json::from_str::<Value>(&e.payload_json)
                        .ok()
                        .map(|v| (e.id, v))
                })
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
        FactsAction::Resolve {
            project,
            id,
            source,
            kind,
            value,
            method,
        } => {
            if !KINDS.contains(&kind.as_str()) || kind == "needs_verification" {
                return Err(anyhow!(
                    "--kind must be a real source kind (measured|model_estimate|build_artifact|external_stat)"
                ));
            }
            // ADR-0036: machine-verification still requires a real source.
            if source.trim().is_empty() {
                return Err(anyhow!(
                    "--source is required (the confirmed DOI/URL/clause)"
                ));
            }
            let facts = passport::current(&conn, &project, Section::VerifiedFacts)?;
            let target = facts
                .iter()
                .find(|e| e.id == id)
                .ok_or_else(|| anyhow!("no current verified fact #{id}"))?;
            let mut v: Value = serde_json::from_str(&target.payload_json)?;
            v["kind"] = json!(kind);
            v["source"] = json!(source);
            v["verified_method"] = json!("machine");
            if !method.trim().is_empty() {
                v["verified_via"] = json!(method);
            }
            if let Some(val) = value {
                v["value"] = json!(val);
            }
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
            println!("Machine-resolved fact #{id} → #{new_id} [{kind}] source: {source}");
        }
        FactsAction::Scan { project, prefix } => {
            // Seed the seen-set from facts already in the passport, then keep
            // adding to it as we enqueue: this dedups WITHIN one scan run too, so
            // an identical marker present in two files (e.g. a per-dimension file
            // and the merged document) is enqueued once, not once per file.
            let mut existing: std::collections::HashSet<String> =
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
                let trim_set: &[char] = &['`', '#', '*', '>', '-', ' ', '('];
                let lines: Vec<&str> = text.lines().collect();
                for (i, line) in lines.iter().enumerate() {
                    if !line.contains("NEEDS-VERIFICATION") {
                        continue;
                    }
                    let marker = line
                        .trim()
                        .trim_start_matches(trim_set)
                        .chars()
                        .take(100)
                        .collect::<String>();
                    // The marker text alone is NOT unique (many references share a
                    // generic "full author list, venue, DOI" marker). Prefix the
                    // preceding non-empty line (the reference / claim it annotates)
                    // so distinct references become distinct queue entries — while
                    // the same reference in two files (per-dimension + merged) still
                    // yields an identical claim and dedups correctly.
                    let context: String = lines[..i]
                        .iter()
                        .rev()
                        .map(|l| l.trim())
                        .find(|l| !l.is_empty() && !l.contains("NEEDS-VERIFICATION"))
                        .unwrap_or("")
                        .trim_start_matches(trim_set)
                        .chars()
                        .take(80)
                        .collect();
                    let claim = if context.is_empty() {
                        marker
                    } else {
                        format!("{context} | {marker}")
                    };
                    if existing.contains(&claim) {
                        continue;
                    }
                    let payload = json!({
                        "claim": claim, "kind": "needs_verification", "value": "",
                        "source": "", "source_path": path, "verified_at": now_utc(),
                    });
                    passport::append(
                        &conn,
                        &project,
                        Section::VerifiedFacts,
                        &payload.to_string(),
                        head.as_deref(),
                        None,
                    )?;
                    existing.insert(claim);
                    enqueued += 1;
                }
            }
            println!("Enqueued {enqueued} NEEDS-VERIFICATION marker(s) into the HITL queue.");
        }
        FactsAction::Dedupe { project, dry_run } => {
            // Group current needs_verification placeholders by claim text; for any
            // claim with >1 copy, keep the lowest id and supersede the others.
            use std::collections::BTreeMap;
            let facts = passport::current(&conn, &project, Section::VerifiedFacts)?;
            let mut by_claim: BTreeMap<String, Vec<i64>> = BTreeMap::new();
            for e in &facts {
                let Ok(v) = serde_json::from_str::<Value>(&e.payload_json) else {
                    continue;
                };
                if v.get("kind").and_then(Value::as_str) != Some("needs_verification") {
                    continue;
                }
                if let Some(claim) = v.get("claim").and_then(Value::as_str) {
                    by_claim.entry(claim.to_string()).or_default().push(e.id);
                }
            }
            let head = worktree::head_commit(&conn, &project)?.map(|c| c.sha256);
            let mut collapsed = 0usize;
            let mut distinct = 0usize;
            for (claim, mut ids) in by_claim {
                distinct += 1;
                if ids.len() < 2 {
                    continue;
                }
                ids.sort_unstable();
                let keep = ids[0];
                for &dup in &ids[1..] {
                    if dry_run {
                        collapsed += 1;
                        continue;
                    }
                    // Supersede the duplicate by a tombstone that points back to
                    // the surviving copy (append-only: the dup stays in history).
                    // kind="duplicate" keeps it OUT of the HITL queue, out of the
                    // anchored-claims set, and out of the unsourced-fact gate.
                    let payload = json!({
                        "claim": claim, "kind": "duplicate", "value": "",
                        "source": "", "deduped_into": keep, "verified_at": now_utc(),
                    });
                    passport::append(
                        &conn,
                        &project,
                        Section::VerifiedFacts,
                        &payload.to_string(),
                        head.as_deref(),
                        Some(dup),
                    )?;
                    collapsed += 1;
                }
            }
            if dry_run {
                println!(
                    "[dry-run] {distinct} distinct claim(s); would collapse {collapsed} duplicate copy(ies)."
                );
            } else {
                println!(
                    "Collapsed {collapsed} duplicate placeholder(s); {distinct} distinct claim(s) remain in the queue."
                );
            }
        }
    }
    Ok(())
}

/// Claim strings of all current verified facts (used by the deliverable gate to
/// treat a matching numeric line as already-sourced).
pub fn anchored_claims(conn: &rusqlite::Connection, project: &str) -> Result<Vec<String>> {
    let facts = passport::current(conn, project, Section::VerifiedFacts)?;
    // Only the four real source kinds anchor a number. A `needs_verification`
    // placeholder is a HITL queue entry (must not rescue the number it tracks),
    // and a `duplicate` tombstone is bookkeeping — neither is a source.
    const SOURCE_KINDS: &[&str] = &[
        "measured",
        "model_estimate",
        "build_artifact",
        "external_stat",
    ];
    Ok(facts
        .iter()
        .filter_map(|e| serde_json::from_str::<Value>(&e.payload_json).ok())
        .filter(|v| {
            v.get("kind")
                .and_then(Value::as_str)
                .is_some_and(|k| SOURCE_KINDS.contains(&k))
        })
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
