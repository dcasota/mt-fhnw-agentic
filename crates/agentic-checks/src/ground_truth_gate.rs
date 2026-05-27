//! `agentic check ground_truth` — concrete-anchor gate for measured facts.
//!
//! A `measured` or `build_artifact` verified fact must trace to a CONCRETE
//! ground-truth anchor (a file path, a commit-ish hex run, a URL, a RAMP run, a
//! package index, a DB/doc/markdown artefact, or an SRPMS path) — not a vague
//! prose source. A fact whose `source` contains none of those anchors surfaces a
//! WARN `GROUND_TRUTH_WEAK`. Advisory: it flags weak provenance, not absence
//! (the `facts_integrity` gate already blocks an empty source).

use std::sync::LazyLock;

use anyhow::Result;
use regex::Regex;
use rusqlite::Connection;
use serde_json::Value;

use agentic_core::passport::{self, Section};

use crate::{CheckReport, Finding, Severity};

/// Fact kinds that must carry a concrete ground-truth anchor.
const ANCHORED_KINDS: &[&str] = &["measured", "build_artifact"];
/// A 7+ hex commit-ish run.
static COMMITISH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b[0-9a-fA-F]{7,}\b").unwrap());
/// Literal anchor substrings (case-insensitive for the alpha tokens). Beyond
/// paths/URLs/DBs/docs, a named *measurement instrument* is concrete,
/// re-runnable ground truth: a script file (`.ps1`/`.sh`/`.py`) or the RAMP
/// estimator invocation (`risk invest`, ADR-0040 — same class as `RAMP`).
const ANCHORS: &[&str] = &[
    "/",
    "http",
    "RAMP",
    "risk invest",
    "packages.",
    ".db",
    ".docx",
    ".md",
    ".ps1",
    ".sh",
    ".py",
    "SRPMS",
];

/// Does `source` contain at least one concrete ground-truth anchor?
#[must_use]
pub fn has_anchor(source: &str) -> bool {
    if ANCHORS.iter().any(|a| source.contains(a)) {
        return true;
    }
    COMMITISH.is_match(source)
}

pub fn run(conn: &Connection, project: &str) -> Result<CheckReport> {
    let mut findings = Vec::new();
    let facts = passport::current(conn, project, Section::VerifiedFacts)?;
    let mut checked = 0usize;
    let mut anchored = 0usize;

    for e in &facts {
        let Ok(v) = serde_json::from_str::<Value>(&e.payload_json) else {
            continue;
        };
        let kind = v.get("kind").and_then(Value::as_str).unwrap_or("");
        if !ANCHORED_KINDS.contains(&kind) {
            continue;
        }
        checked += 1;
        let source = v.get("source").and_then(Value::as_str).unwrap_or("");
        if has_anchor(source) {
            anchored += 1;
            continue;
        }
        let claim = v.get("claim").and_then(Value::as_str).unwrap_or("?");
        findings.push(Finding {
            category: "GROUND_TRUTH_WEAK".into(),
            severity: Severity::Warn,
            message: format!(
                "fact #{} ('{}', kind {kind}) source lacks a concrete anchor (path/commit/URL/RAMP/…)",
                e.id,
                claim.chars().take(60).collect::<String>()
            ),
            location: Some("verified_facts".into()),
        });
    }

    findings.push(Finding {
        category: "GROUND_TRUTH_SUMMARY".into(),
        severity: Severity::Info,
        message: format!("{anchored}/{checked} measured/build fact(s) carry a concrete anchor"),
        location: Some("ground_truth".into()),
    });

    Ok(CheckReport::new("ground_truth", findings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentic_core::{
        db::open_in_memory,
        project::{ProjectKind, create as create_project},
    };

    #[test]
    fn anchors_recognised() {
        assert!(has_anchor("out/sources/x.md"));
        assert!(has_anchor("see RAMP run 3"));
        assert!(has_anchor("https://example.org"));
        assert!(has_anchor("commit 9ecddd6abc")); // hex run
        assert!(!has_anchor("trust me it is true"));
        // Measurement instruments are concrete, re-runnable ground truth.
        assert!(has_anchor("photonos-package-report.ps1 package walk"));
        assert!(has_anchor("parameterised as `agentic risk invest`")); // RAMP estimator
        assert!(has_anchor("computed by collect.sh"));
        assert!(has_anchor("counted via tally.py"));
    }

    #[test]
    fn weak_source_warns() {
        let conn = open_in_memory().unwrap();
        let pid = create_project(&conn, "T", ProjectKind::Thesis, "en", None).unwrap();
        passport::append(
            &conn,
            &pid,
            Section::VerifiedFacts,
            r#"{"kind":"measured","claim":"42 builds","source":"as observed"}"#,
            None,
            None,
        )
        .unwrap();
        let report = run(&conn, &pid).unwrap();
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.category == "GROUND_TRUTH_WEAK")
        );
    }
}
