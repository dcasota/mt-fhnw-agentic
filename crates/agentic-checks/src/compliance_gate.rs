//! `agentic check compliance` — contamination-report consolidation gate.
//!
//! The `contamination` gate writes a `contamination_status` compliance report
//! (with a PRISMA disposition) to the passport. This gate reads the NEWEST such
//! report and consolidates its verdict:
//!   * no report at all → WARN `COMPLIANCE_NO_REPORT` (run `check contamination`),
//!   * `fabricated > 0`  → ERROR `COMPLIANCE_FABRICATED` (blocks),
//!   * `suspect > 0`     → WARN `COMPLIANCE_SUSPECT`,
//!   * otherwise (matched / not-indexed only) → PASS.
//!
//! An INFO summary always echoes the PRISMA buckets.

use anyhow::Result;
use rusqlite::Connection;
use serde_json::Value;

use agentic_core::passport::{self, Section};

use crate::{CheckReport, Finding, Severity};

pub fn run(conn: &Connection, project: &str) -> Result<CheckReport> {
    let mut findings = Vec::new();
    let reports = passport::current(conn, project, Section::ComplianceReports)?;

    // Newest contamination_status report (entries are id-ordered ascending).
    let latest = reports
        .iter()
        .rev()
        .filter_map(|e| serde_json::from_str::<Value>(&e.payload_json).ok())
        .find(|v| v.get("report").and_then(Value::as_str) == Some("contamination_status"));

    let Some(v) = latest else {
        findings.push(Finding {
            category: "COMPLIANCE_NO_REPORT".into(),
            severity: Severity::Warn,
            message: "no contamination_status report — run `agentic check contamination`".into(),
            location: Some("compliance_reports".into()),
        });
        return Ok(CheckReport::new("compliance", findings));
    };

    let prisma = v.get("prisma").cloned().unwrap_or(Value::Null);
    let bucket = |k: &str| prisma.get(k).and_then(Value::as_u64).unwrap_or(0);
    let (matched, not_indexed, suspect, fabricated) = (
        bucket("matched"),
        bucket("not_indexed"),
        bucket("suspect"),
        bucket("fabricated"),
    );

    if fabricated > 0 {
        findings.push(Finding {
            category: "COMPLIANCE_FABRICATED".into(),
            severity: Severity::Error,
            message: format!(
                "{fabricated} fabricated reference(s) in the latest contamination report"
            ),
            location: Some("compliance_reports".into()),
        });
    }
    if suspect > 0 {
        findings.push(Finding {
            category: "COMPLIANCE_SUSPECT".into(),
            severity: Severity::Warn,
            message: format!("{suspect} suspect reference(s) — route to cross-model / HITL"),
            location: Some("compliance_reports".into()),
        });
    }

    findings.push(Finding {
        category: "COMPLIANCE_SUMMARY".into(),
        severity: Severity::Info,
        message: format!(
            "PRISMA buckets: {matched} matched, {not_indexed} not-indexed, {suspect} suspect, {fabricated} fabricated"
        ),
        location: Some("compliance".into()),
    });

    Ok(CheckReport::new("compliance", findings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentic_core::{
        db::open_in_memory,
        project::{ProjectKind, create as create_project},
    };

    #[test]
    fn no_report_warns() {
        let conn = open_in_memory().unwrap();
        let pid = create_project(&conn, "T", ProjectKind::Thesis, "en", None).unwrap();
        let report = run(&conn, &pid).unwrap();
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.category == "COMPLIANCE_NO_REPORT")
        );
    }

    #[test]
    fn fabricated_blocks() {
        let conn = open_in_memory().unwrap();
        let pid = create_project(&conn, "T", ProjectKind::Thesis, "en", None).unwrap();
        passport::append(
            &conn,
            &pid,
            Section::ComplianceReports,
            r#"{"report":"contamination_status","prisma":{"matched":3,"not_indexed":0,"suspect":1,"fabricated":2}}"#,
            None,
            None,
        )
        .unwrap();
        let report = run(&conn, &pid).unwrap();
        assert_eq!(report.verdict, crate::Verdict::Fail);
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.category == "COMPLIANCE_FABRICATED")
        );
    }

    #[test]
    fn clean_report_passes() {
        let conn = open_in_memory().unwrap();
        let pid = create_project(&conn, "T", ProjectKind::Thesis, "en", None).unwrap();
        passport::append(
            &conn,
            &pid,
            Section::ComplianceReports,
            r#"{"report":"contamination_status","prisma":{"matched":5,"not_indexed":1,"suspect":0,"fabricated":0}}"#,
            None,
            None,
        )
        .unwrap();
        let report = run(&conn, &pid).unwrap();
        assert_eq!(report.verdict, crate::Verdict::Pass);
    }
}
