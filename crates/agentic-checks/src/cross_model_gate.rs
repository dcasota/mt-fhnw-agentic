//! `agentic check cross_model` — cross-model attestation coverage (ADR-0028).
//!
//! A high-stakes verified fact (an external statistic or a measured value)
//! should be independently re-derived by a *second* provider (`agentic verify
//! cross-model`), which stamps a `cross_model` field onto the fact. This gate
//! surfaces facts of those kinds that carry NO `cross_model` attestation as a
//! WARN `CROSS_MODEL_UNATTESTED`. It is purely advisory — providers may be
//! unconfigured — so it never escalates beyond WARN.

use anyhow::Result;
use rusqlite::Connection;
use serde_json::Value;

use agentic_core::passport::{self, Section};

use crate::{CheckReport, Finding, Severity};

/// Fact kinds that warrant a second-model attestation.
const ATTESTABLE: &[&str] = &["external_stat", "measured"];

pub fn run(conn: &Connection, project: &str) -> Result<CheckReport> {
    let mut findings = Vec::new();
    let facts = passport::current(conn, project, Section::VerifiedFacts)?;
    let mut attestable = 0usize;
    let mut attested = 0usize;

    for e in &facts {
        let Ok(v) = serde_json::from_str::<Value>(&e.payload_json) else {
            continue;
        };
        let kind = v.get("kind").and_then(Value::as_str).unwrap_or("");
        if !ATTESTABLE.contains(&kind) {
            continue;
        }
        attestable += 1;
        if v.get("cross_model").is_some() {
            attested += 1;
            continue;
        }
        let claim = v.get("claim").and_then(Value::as_str).unwrap_or("?");
        findings.push(Finding {
            category: "CROSS_MODEL_UNATTESTED".into(),
            severity: Severity::Warn,
            message: format!(
                "fact #{} ('{}', kind {kind}) has no cross_model attestation — run `agentic verify cross-model`",
                e.id,
                claim.chars().take(60).collect::<String>()
            ),
            location: Some("verified_facts".into()),
        });
    }

    findings.push(Finding {
        category: "CROSS_MODEL_SUMMARY".into(),
        severity: Severity::Info,
        message: format!(
            "{attested}/{attestable} attestable fact(s) carry a cross_model attestation"
        ),
        location: Some("cross_model".into()),
    });

    Ok(CheckReport::new("cross_model", findings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentic_core::{
        db::open_in_memory,
        project::{ProjectKind, create as create_project},
    };

    #[test]
    fn unattested_external_stat_warns() {
        let conn = open_in_memory().unwrap();
        let pid = create_project(&conn, "T", ProjectKind::Thesis, "en", None).unwrap();
        passport::append(
            &conn,
            &pid,
            Section::VerifiedFacts,
            r#"{"kind":"external_stat","claim":"X grew 12%","source":"http://x"}"#,
            None,
            None,
        )
        .unwrap();
        let report = run(&conn, &pid).unwrap();
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.category == "CROSS_MODEL_UNATTESTED")
        );
    }

    #[test]
    fn attested_fact_clean() {
        let conn = open_in_memory().unwrap();
        let pid = create_project(&conn, "T", ProjectKind::Thesis, "en", None).unwrap();
        passport::append(
            &conn,
            &pid,
            Section::VerifiedFacts,
            r#"{"kind":"measured","claim":"42 packages","cross_model":{"verdict":"agree"}}"#,
            None,
            None,
        )
        .unwrap();
        let report = run(&conn, &pid).unwrap();
        assert!(
            !report
                .findings
                .iter()
                .any(|f| f.category == "CROSS_MODEL_UNATTESTED")
        );
    }
}
