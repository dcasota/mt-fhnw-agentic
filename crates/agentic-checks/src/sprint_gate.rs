//! `agentic check sprint` — sprint-contract status gate.
//!
//! Inspects the `sprint_contracts` table for this project. The P1 schema
//! (migration 0001) has NO `status` column — its columns are `contract_id`,
//! `agent`, `phase`, `scoring_plan_json`, `applied_results_json`,
//! `dissent_json`, `linked_phase_1_id`. So, as specified, this gate simply
//! counts the contracts and emits an INFO summary; with no contracts it reports
//! INFO `SPRINT_NONE` (a clean PASS). It is advisory only — never blocks.

use anyhow::Result;
use rusqlite::Connection;

use crate::{CheckReport, Finding, Severity};

pub fn run(conn: &Connection, project: &str) -> Result<CheckReport> {
    let mut findings = Vec::new();

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sprint_contracts WHERE project_id = ?1",
            [project],
            |r| r.get(0),
        )
        .unwrap_or(0);

    if count == 0 {
        findings.push(Finding {
            category: "SPRINT_NONE".into(),
            severity: Severity::Info,
            message: "no sprint contracts for this project".into(),
            location: Some("sprint_contracts".into()),
        });
        return Ok(CheckReport::new("sprint", findings));
    }

    // The schema carries no status column; a `dissent` phase row is the closest
    // signal of an unresolved contract — surface it as an advisory WARN.
    let dissent: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sprint_contracts WHERE project_id = ?1 AND phase = 'dissent'",
            [project],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if dissent > 0 {
        findings.push(Finding {
            category: "SPRINT_VIOLATED".into(),
            severity: Severity::Warn,
            message: format!("{dissent} sprint contract(s) in 'dissent' phase — review resolution"),
            location: Some("sprint_contracts".into()),
        });
    }

    findings.push(Finding {
        category: "SPRINT_SUMMARY".into(),
        severity: Severity::Info,
        message: format!("{count} sprint contract(s) for this project ({dissent} in dissent)"),
        location: Some("sprint".into()),
    });

    Ok(CheckReport::new("sprint", findings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentic_core::{
        db::open_in_memory,
        project::{ProjectKind, create as create_project},
    };

    #[test]
    fn no_contracts_is_info_pass() {
        let conn = open_in_memory().unwrap();
        let pid = create_project(&conn, "T", ProjectKind::Thesis, "en", None).unwrap();
        let report = run(&conn, &pid).unwrap();
        assert_eq!(report.verdict, crate::Verdict::Pass);
        assert!(report.findings.iter().any(|f| f.category == "SPRINT_NONE"));
    }

    #[test]
    fn dissent_phase_warns() {
        let conn = open_in_memory().unwrap();
        let pid = create_project(&conn, "T", ProjectKind::Thesis, "en", None).unwrap();
        conn.execute(
            "INSERT INTO sprint_contracts (contract_id, project_id, agent, phase) \
             VALUES ('c1', ?1, 'a', 'dissent')",
            [&pid],
        )
        .unwrap();
        let report = run(&conn, &pid).unwrap();
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.category == "SPRINT_VIOLATED")
        );
    }
}
