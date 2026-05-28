//! `agentic check model-review` — surfaces the LLM document/ranking review
//! verdicts (ADR-0049) recorded by `agentic review run`.
//!
//! `review run` stores per-document and per-ranking verdicts as
//! `claim_audit_results` entries (kind=model_review). This gate reports them in
//! the cascade gate suite so they are visible and sealed each run: a model
//! recommendation to `exclude` a document from the mainline is a WARN (advisory
//! — adoption is decided downstream), `revise` is INFO, `accept` is counted. It
//! never escalates beyond WARN (the reviewing model may be unconfigured).

use std::collections::HashMap;

use anyhow::Result;
use rusqlite::Connection;
use serde_json::Value;

use agentic_core::passport::{self, Section};

use crate::{CheckReport, Finding, Severity};

pub fn run(conn: &Connection, project: &str) -> Result<CheckReport> {
    let mut findings = Vec::new();
    let entries = passport::current(conn, project, Section::ClaimAuditResults)?;

    // Per-path latest-wins: passport::current keeps any entry not pointed at by
    // a later `replaces`, which can leave an orphaned legacy verdict alive
    // alongside its real successor (a fresh chain that started from a newer id
    // never linked back to the orphan). Dedupe by path so the display matches
    // `excluded_paths()` (core), and never double-list one path.
    let mut latest: HashMap<String, (i64, Value)> = HashMap::new();
    let mut rankings: Vec<(i64, Value)> = Vec::new();
    for e in &entries {
        let Ok(v) = serde_json::from_str::<Value>(&e.payload_json) else {
            continue;
        };
        if v.get("kind").and_then(Value::as_str) != Some("model_review") {
            continue;
        }
        if v.get("scope").and_then(Value::as_str) == Some("rankings") {
            rankings.push((e.id, v));
            continue;
        }
        let Some(path) = v.get("path").and_then(Value::as_str) else {
            continue;
        };
        let cur = latest.get(path).map(|(id, _)| *id).unwrap_or(0);
        if e.id > cur {
            latest.insert(path.to_string(), (e.id, v));
        }
    }

    // Emit the rankings-scope review(s); a path-less review still belongs in
    // the report so it stays auditable, but never participates in path dedupe.
    for (_, v) in &rankings {
        let assessment = v
            .get("assessment")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
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
    }

    let (mut accept, mut revise, mut exclude, mut unknown) = (0usize, 0usize, 0usize, 0usize);
    // Iterate by path so the per-path emit order is stable across runs.
    let mut paths: Vec<&String> = latest.keys().collect();
    paths.sort();
    for path in &paths {
        let (_, v) = &latest[*path];
        let assessment = v
            .get("assessment")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        match assessment {
            "exclude" => {
                exclude += 1;
                findings.push(Finding {
                    category: "MODEL_REVIEW_EXCLUDE".into(),
                    severity: Severity::Warn,
                    message: format!("model recommends EXCLUDE from the mainline: {path}"),
                    location: Some((*path).clone()),
                });
            }
            "revise" => {
                revise += 1;
                findings.push(Finding {
                    category: "MODEL_REVIEW_REVISE".into(),
                    severity: Severity::Info,
                    message: format!("model recommends revision: {path}"),
                    location: Some((*path).clone()),
                });
            }
            "accept" => accept += 1,
            _ => unknown += 1,
        }
    }
    let reviewed = latest.len();

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
    fn orphan_legacy_verdict_is_deduped_by_latest_wins() {
        // Regression: passport::current can keep TWO entries alive for one path
        // when a fresh chain (Grok exclude → Grok revise → Opus exclude) never
        // linked back to an even-earlier orphan exclude that nothing replaces.
        // The gate must dedupe by path so the orphan does not double-list and
        // double-count alongside the real latest verdict.
        let conn = open_in_memory().unwrap();
        let pid = create_project(&conn, "T", ProjectKind::Thesis, "en", None).unwrap();
        // The orphan that no later entry replaces.
        add(
            &conn,
            &pid,
            r#"{"kind":"model_review","path":"a.md","assessment":"exclude","reviewer":"legacy"}"#,
        );
        // A new chain starts; its tail (latest by id) is the Opus override.
        let head = passport::append(
            &conn,
            &pid,
            Section::ClaimAuditResults,
            r#"{"kind":"model_review","path":"a.md","assessment":"exclude","reviewer":"grok"}"#,
            None,
            None,
        )
        .unwrap();
        passport::append(
            &conn,
            &pid,
            Section::ClaimAuditResults,
            r#"{"kind":"model_review","path":"a.md","assessment":"accept","reviewer":"opus"}"#,
            None,
            Some(head as i64),
        )
        .unwrap();
        let report = run(&conn, &pid).unwrap();
        // Exactly ONE per-path finding for a.md, and it is the latest (accept,
        // i.e. not EXCLUDE/REVISE). The orphan must not produce a second row.
        let per_path: Vec<_> = report
            .findings
            .iter()
            .filter(|f| {
                f.category == "MODEL_REVIEW_EXCLUDE" || f.category == "MODEL_REVIEW_REVISE"
            })
            .collect();
        assert_eq!(per_path.len(), 0, "latest is accept; no exclude/revise row");
        // SUMMARY counts unique paths, not raw entries.
        let summary = report
            .findings
            .iter()
            .find(|f| f.category == "MODEL_REVIEW_SUMMARY")
            .expect("summary present");
        assert!(
            summary.message.starts_with("1 document review(s)"),
            "summary should count 1 unique path, got: {}",
            summary.message
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
