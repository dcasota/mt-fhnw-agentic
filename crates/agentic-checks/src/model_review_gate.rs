//! `agentic check model-review` — surfaces the LLM document/ranking review
//! verdicts (ADR-0049) recorded by `agentic review run`.
//!
//! `review run` stores per-document and per-ranking verdicts as
//! `claim_audit_results` entries (kind=model_review). This gate reports them in
//! the cascade gate suite so they are visible and sealed each run: a model
//! recommendation to `exclude` a document from the mainline is a WARN (advisory
//! — adoption is decided downstream), `revise` is INFO, `accept` is counted. It
//! never escalates beyond WARN (the reviewing model may be unconfigured).

use anyhow::Result;
use rusqlite::Connection;
use serde_json::Value;

use agentic_core::passport::{self, Section};

use crate::{CheckReport, Finding, Severity};

pub fn run(conn: &Connection, project: &str) -> Result<CheckReport> {
    let mut findings = Vec::new();
    let entries = passport::current(conn, project, Section::ClaimAuditResults)?;
    let mut reviewed = 0usize;
    let (mut accept, mut revise, mut exclude, mut unknown) = (0usize, 0usize, 0usize, 0usize);

    for e in &entries {
        let Ok(v) = serde_json::from_str::<Value>(&e.payload_json) else {
            continue;
        };
        if v.get("kind").and_then(Value::as_str) != Some("model_review") {
            continue;
        }
        let assessment = v
            .get("assessment")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        // The whole-rankings review is reported separately from per-doc ones.
        if v.get("scope").and_then(Value::as_str) == Some("rankings") {
            let sev = if assessment == "exclude" || assessment == "revise" {
                Severity::Warn
            } else {
                Severity::Info
            };
            let fb = v
                .get("ranking_feedback")
                .and_then(Value::as_str)
                .unwrap_or("");
            findings.push(Finding {
                category: "MODEL_REVIEW_RANKINGS".into(),
                severity: sev,
                message: format!("rankings review: {assessment} — {fb}"),
                location: Some("claim_audit_results".into()),
            });
            continue;
        }
        reviewed += 1;
        let path = v.get("path").and_then(Value::as_str).unwrap_or("?");
        match assessment {
            "exclude" => {
                exclude += 1;
                findings.push(Finding {
                    category: "MODEL_REVIEW_EXCLUDE".into(),
                    severity: Severity::Warn,
                    message: format!("model recommends EXCLUDE from the mainline: {path}"),
                    location: Some(path.to_string()),
                });
            }
            "revise" => {
                revise += 1;
                findings.push(Finding {
                    category: "MODEL_REVIEW_REVISE".into(),
                    severity: Severity::Info,
                    message: format!("model recommends revision: {path}"),
                    location: Some(path.to_string()),
                });
            }
            "accept" => accept += 1,
            _ => unknown += 1,
        }
    }

    if reviewed == 0 {
        findings.push(Finding {
            category: "MODEL_REVIEW_NONE".into(),
            severity: Severity::Info,
            message: "no model reviews recorded — run `agentic review run`".into(),
            location: Some("model_review".into()),
        });
    } else {
        findings.push(Finding {
            category: "MODEL_REVIEW_SUMMARY".into(),
            severity: Severity::Info,
            message: format!(
                "{reviewed} document review(s): {accept} accept, {revise} revise, {exclude} exclude, {unknown} unknown"
            ),
            location: Some("model_review".into()),
        });
    }

    Ok(CheckReport::new("model_review", findings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentic_core::{
        db::open_in_memory,
        project::{ProjectKind, create as create_project},
    };

    fn add(conn: &Connection, pid: &str, payload: &str) {
        passport::append(conn, pid, Section::ClaimAuditResults, payload, None, None).unwrap();
    }

    #[test]
    fn exclude_warns_accept_clean() {
        let conn = open_in_memory().unwrap();
        let pid = create_project(&conn, "T", ProjectKind::Thesis, "en", None).unwrap();
        add(
            &conn,
            &pid,
            r#"{"kind":"model_review","path":"a.md","assessment":"accept"}"#,
        );
        add(
            &conn,
            &pid,
            r#"{"kind":"model_review","path":"b.md","assessment":"exclude"}"#,
        );
        // A non-review claim_audit_results entry must be ignored.
        add(&conn, &pid, r#"{"kind":"ranking","tier":"Critical"}"#);
        let report = run(&conn, &pid).unwrap();
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.category == "MODEL_REVIEW_EXCLUDE")
        );
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.category == "MODEL_REVIEW_SUMMARY")
        );
        assert_eq!(
            report
                .findings
                .iter()
                .filter(|f| f.category == "MODEL_REVIEW_EXCLUDE")
                .count(),
            1
        );
    }

    #[test]
    fn no_reviews_reports_none() {
        let conn = open_in_memory().unwrap();
        let pid = create_project(&conn, "T", ProjectKind::Thesis, "en", None).unwrap();
        let report = run(&conn, &pid).unwrap();
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.category == "MODEL_REVIEW_NONE")
        );
    }
}
