//! `agentic check predatory` — predatory-venue heuristic gate.
//!
//! Flags `literature_corpus` entries whose `venue`/`publisher` matches a SMALL,
//! conservative deny-list of well-known predatory or low-quality publishers. The
//! list is a HEURISTIC, not an authority — a match is a WARN `PREDATORY_VENUE`
//! prompting human review, never a block. Most corpora will produce zero
//! findings.

use anyhow::Result;
use rusqlite::Connection;
use serde_json::Value;

use agentic_core::passport::{self, Section};

use crate::{CheckReport, Finding, Severity};

/// HEURISTIC predatory/low-quality deny-list (case-insensitive substring match).
/// Kept deliberately tiny and conservative — extend only with strong evidence.
/// Matching a token here means "needs a human look", not "is fraudulent".
const DENYLIST: &[&str] = &[
    "OMICS",
    "Scientific Research Publishing",
    "SCIRP",
    "Bentham Open",
    "Hindawi (legacy)",
    "IJSER",
    "predatory",
];

pub fn run(conn: &Connection, project: &str) -> Result<CheckReport> {
    let mut findings = Vec::new();
    let corpus = passport::current(conn, project, Section::LiteratureCorpus)?;
    let mut flagged = 0usize;

    for e in &corpus {
        let Ok(v) = serde_json::from_str::<Value>(&e.payload_json) else {
            continue;
        };
        let key = v.get("citation_key").and_then(Value::as_str).unwrap_or("?");
        let venue = v.get("venue").and_then(Value::as_str).unwrap_or("");
        let publisher = v.get("publisher").and_then(Value::as_str).unwrap_or("");
        let hay = format!("{venue} {publisher}").to_lowercase();
        if let Some(tok) = DENYLIST.iter().find(|t| hay.contains(&t.to_lowercase())) {
            flagged += 1;
            findings.push(Finding {
                category: "PREDATORY_VENUE".into(),
                severity: Severity::Warn,
                message: format!(
                    "'{key}': venue/publisher matches predatory deny-list token '{tok}' (heuristic — review)"
                ),
                location: Some("literature_corpus".into()),
            });
        }
    }

    findings.push(Finding {
        category: "PREDATORY_SUMMARY".into(),
        severity: Severity::Info,
        message: format!("{flagged} entry/entries matched the predatory deny-list (heuristic)"),
        location: Some("predatory".into()),
    });

    Ok(CheckReport::new("predatory", findings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentic_core::{
        db::open_in_memory,
        project::{ProjectKind, create as create_project},
    };

    #[test]
    fn predatory_venue_warns() {
        let conn = open_in_memory().unwrap();
        let pid = create_project(&conn, "T", ProjectKind::Thesis, "en", None).unwrap();
        passport::append(
            &conn,
            &pid,
            Section::LiteratureCorpus,
            r#"{"citation_key":"x2020","venue":"OMICS Journal of Things"}"#,
            None,
            None,
        )
        .unwrap();
        let report = run(&conn, &pid).unwrap();
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.category == "PREDATORY_VENUE")
        );
    }

    #[test]
    fn reputable_venue_clean() {
        let conn = open_in_memory().unwrap();
        let pid = create_project(&conn, "T", ProjectKind::Thesis, "en", None).unwrap();
        passport::append(
            &conn,
            &pid,
            Section::LiteratureCorpus,
            r#"{"citation_key":"y2021","venue":"IEEE Transactions on Software Engineering"}"#,
            None,
            None,
        )
        .unwrap();
        let report = run(&conn, &pid).unwrap();
        assert!(
            !report
                .findings
                .iter()
                .any(|f| f.category == "PREDATORY_VENUE")
        );
    }
}
