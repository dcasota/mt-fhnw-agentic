//! `agentic check facts-integrity` — verified-facts integrity (ADR-0036/0042/0044).
//!
//! The ADR-0036 backstop for the verified-facts backbone: a sourced fact must
//! carry a real source (never invented). A `needs_verification` placeholder is
//! allowed but surfaced (WARN) as outstanding HITL work.
//!
//! ADR-0044 adds two ARS-parity passes:
//!   * **claim-audit anchored-judge** — each `claim_audit_results` entry must
//!     carry a locator anchor (`quote`/`page`/`section`/`paragraph`/`locator`);
//!     an unanchored judgement cannot be re-checked → WARN `CLAIM_AUDIT_UNANCHORED`.
//!   * **leakage scan** — `[MATERIAL GAP]` markers left in deliverables flag
//!     knowledge that was never materialised from sources → WARN `MATERIAL_GAP`.

use anyhow::Result;
use rusqlite::Connection;
use serde_json::Value;

use agentic_core::passport::{self, Section};
use agentic_core::worktree;

use crate::{CheckReport, Finding, Severity};

/// Locator-anchor keys an ARS claim-audit result should carry.
const ANCHOR_KEYS: &[&str] = &["quote", "page", "section", "paragraph", "locator", "anchor"];

/// Does a claim-audit result carry at least one non-empty locator anchor?
#[must_use]
pub fn has_locator_anchor(v: &Value) -> bool {
    ANCHOR_KEYS.iter().any(|k| {
        v.get(*k)
            .map(|x| match x {
                Value::String(s) => !s.trim().is_empty(),
                Value::Null => false,
                _ => true,
            })
            .unwrap_or(false)
    })
}

pub fn run(conn: &Connection, project: &str) -> Result<CheckReport> {
    let mut findings = Vec::new();
    let facts = passport::current(conn, project, Section::VerifiedFacts)?;
    let mut needs = 0usize;
    for e in &facts {
        let Ok(v) = serde_json::from_str::<Value>(&e.payload_json) else {
            continue;
        };
        let kind = v.get("kind").and_then(Value::as_str).unwrap_or("");
        let claim = v.get("claim").and_then(Value::as_str).unwrap_or("?");
        let source = v.get("source").and_then(Value::as_str).unwrap_or("");
        if kind == "needs_verification" {
            needs += 1;
            continue;
        }
        // A `duplicate` tombstone (from `facts dedupe`) is bookkeeping, not a
        // sourced fact — it legitimately carries an empty source.
        if kind == "duplicate" {
            continue;
        }
        // ADR-0036: a sourced fact must have a real, non-empty source.
        if source.trim().is_empty() {
            findings.push(Finding {
                category: "FACT_UNSOURCED".into(),
                severity: Severity::Error,
                message: format!(
                    "verified fact #{} ('{}', kind {kind}) has no source — ADR-0036 forbids an unsourced anchored fact",
                    e.id,
                    claim.chars().take(60).collect::<String>()
                ),
                location: Some("verified_facts".into()),
            });
        }
    }
    if needs > 0 {
        findings.push(Finding {
            category: "FACTS_NEEDS_VERIFICATION".into(),
            severity: Severity::Warn,
            message: format!(
                "{needs} fact(s) await HITL sign-off (`agentic facts verify <id>`); these stay flagged in deliverables"
            ),
            location: Some("verified_facts".into()),
        });
    }

    // ADR-0044 pass 1: claim-audit anchored-judge.
    let audits = passport::current(conn, project, Section::ClaimAuditResults)?;
    let mut unanchored = 0usize;
    for e in &audits {
        let Ok(v) = serde_json::from_str::<Value>(&e.payload_json) else {
            continue;
        };
        if !has_locator_anchor(&v) {
            unanchored += 1;
        }
    }
    if unanchored > 0 {
        findings.push(Finding {
            category: "CLAIM_AUDIT_UNANCHORED".into(),
            severity: Severity::Warn,
            message: format!(
                "{unanchored} claim-audit result(s) carry no locator anchor (quote/page/section/paragraph) — the judgement cannot be re-checked"
            ),
            location: Some("claim_audit_results".into()),
        });
    }

    // ADR-0044 pass 2: leakage `[MATERIAL GAP]` markers in deliverables.
    let mut gaps = 0usize;
    for (path, sha) in worktree::list(conn, project, "out/sources/")? {
        if !path.ends_with(".md") {
            continue;
        }
        let Ok(blob) = agentic_core::content::blob::get_blob(conn, &sha) else {
            continue;
        };
        let text = String::from_utf8_lossy(&blob.content);
        for (i, ln) in text.lines().enumerate() {
            if ln.to_uppercase().contains("[MATERIAL GAP]") {
                gaps += 1;
                findings.push(Finding {
                    category: "MATERIAL_GAP".into(),
                    severity: Severity::Warn,
                    message: "[MATERIAL GAP] marker — supply the source or remove the claim".into(),
                    location: Some(format!("{path}:{}", i + 1)),
                });
            }
        }
    }
    if gaps == 0 {
        findings.push(Finding {
            category: "MATERIAL_GAP_SUMMARY".into(),
            severity: Severity::Info,
            message: "no unresolved [MATERIAL GAP] markers in deliverables".into(),
            location: Some("facts_integrity".into()),
        });
    }

    Ok(CheckReport::new("facts_integrity", findings))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchor_detection() {
        let with = serde_json::json!({"claim":"x","section":"§3.2"});
        let without = serde_json::json!({"claim":"x","judgement":"aligned"});
        let empty = serde_json::json!({"claim":"x","quote":"  "});
        assert!(has_locator_anchor(&with));
        assert!(!has_locator_anchor(&without));
        assert!(!has_locator_anchor(&empty));
    }
}
