//! `agentic check facts-integrity` — verified-facts integrity (ADR-0036/0042).
//!
//! The ADR-0036 backstop for the verified-facts backbone: a sourced fact must
//! carry a real source (never invented). A `needs_verification` placeholder is
//! allowed but surfaced (WARN) as outstanding HITL work.

use anyhow::Result;
use rusqlite::Connection;
use serde_json::Value;

use agentic_core::passport::{self, Section};

use crate::{CheckReport, Finding, Severity};

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
    Ok(CheckReport::new("facts_integrity", findings))
}
