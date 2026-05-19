//! Self-check — structural integrity at the DB level.

use anyhow::Result;
use rusqlite::Connection;

use crate::{CheckReport, Finding, Severity, Verdict};

pub fn run(conn: &Connection) -> Result<CheckReport> {
    let mut findings = Vec::new();

    // Verify the schema version table exists and matches.
    let version: i64 = conn
        .query_row("SELECT IFNULL(MAX(version), 0) FROM schema_version", [], |row| row.get(0))
        .unwrap_or(0);
    if version == 0 {
        findings.push(Finding {
            category: "schema".into(),
            severity: Severity::Blocking,
            message: "schema_version table is empty -- DB not initialised".into(),
        });
    }

    // Verify at least one project exists.
    let project_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM projects", [], |row| row.get(0))
        .unwrap_or(0);
    if project_count == 0 {
        findings.push(Finding {
            category: "projects".into(),
            severity: Severity::Warn,
            message: "no projects yet (run `agentic init` or `agentic project new`)".into(),
        });
    }

    let verdict = if findings.iter().any(|f| matches!(f.severity, Severity::Blocking | Severity::Error)) {
        Verdict::Fail
    } else if findings.iter().any(|f| matches!(f.severity, Severity::Warn)) {
        Verdict::Warn
    } else {
        Verdict::Pass
    };

    Ok(CheckReport {
        checker: "self".into(),
        verdict,
        findings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentic_core::{db::open_in_memory, project::{ProjectKind, create as create_project}};

    #[test]
    fn empty_db_warns_on_no_projects() {
        let conn = open_in_memory().unwrap();
        let report = run(&conn).unwrap();
        assert!(matches!(report.verdict, Verdict::Warn));
    }

    #[test]
    fn with_project_passes() {
        let conn = open_in_memory().unwrap();
        create_project(&conn, "P", ProjectKind::Thesis, "en", None).unwrap();
        let report = run(&conn).unwrap();
        assert!(matches!(report.verdict, Verdict::Pass));
    }
}
