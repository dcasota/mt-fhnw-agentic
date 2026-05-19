//! Self-check — structural integrity at the DB level.
//!
//! Expanded in P1 from "any project exists" to:
//!   * schema version present and at expected level
//!   * at least one project exists
//!   * for every active project: name is non-empty, working_lang valid, head_ref
//!     consistent (no dangling commit pointer)
//!   * journal pending approvals across the DB
//!   * ref naming follows `<project_id>/main` or `iter-NNN` conventions
//!   * passport entries have valid JSON payloads

use agentic_core::{Connection, db::NEWEST_SCHEMA_VERSION};
use anyhow::Result;

use crate::{CheckReport, Finding, Severity};

pub fn run(conn: &Connection) -> Result<CheckReport> {
    let mut findings = Vec::new();

    // 1. Schema version.
    let version: i64 = conn
        .query_row(
            "SELECT IFNULL(MAX(version), 0) FROM schema_version",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    if version == 0 {
        findings.push(Finding {
            category: "schema".into(),
            severity: Severity::Blocking,
            message: "schema_version table is empty -- DB not initialised".into(),
            location: None,
        });
    } else if version != i64::from(NEWEST_SCHEMA_VERSION) {
        findings.push(Finding {
            category: "schema".into(),
            severity: Severity::Warn,
            message: format!(
                "schema_version = {version}, build expects {NEWEST_SCHEMA_VERSION} -- `agentic migrate` may be needed",
            ),
            location: None,
        });
    }

    // 2. At least one project.
    let project_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM projects", [], |row| row.get(0))
        .unwrap_or(0);
    if project_count == 0 {
        findings.push(Finding {
            category: "projects".into(),
            severity: Severity::Warn,
            message: "no projects yet (run `agentic init` or `agentic project new`)".into(),
            location: None,
        });
    }

    // 3. Project integrity: head_ref points to an existing ref, working_lang valid.
    let mut stmt = conn.prepare("SELECT id, name, working_lang, status, head_ref FROM projects")?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, Option<String>>(4)?,
        ))
    })?;
    for row in rows {
        let (id, name, lang, _status, head_ref) = row?;
        if name.trim().is_empty() {
            findings.push(Finding {
                category: "project".into(),
                severity: Severity::Error,
                message: format!("project {id} has empty name"),
                location: Some(format!("projects.{id}")),
            });
        }
        if !matches!(lang.as_str(), "en" | "de" | "fr" | "it" | "rm" | "hi") {
            findings.push(Finding {
                category: "project".into(),
                severity: Severity::Error,
                message: format!("project {id} has invalid working_lang '{lang}'"),
                location: Some(format!("projects.{id}")),
            });
        }
        // Note: `projects.head_ref` is FK-constrained to `refs.name`, so a
        // dangling pointer is rejected at INSERT/UPDATE time. We keep this
        // branch for symmetry and as a belt-and-braces check should the FK
        // ever be relaxed.
        if let Some(href) = head_ref {
            let exists: i64 = conn.query_row(
                "SELECT COUNT(*) FROM refs WHERE name = ?1",
                rusqlite::params![href],
                |r| r.get(0),
            )?;
            if exists == 0 {
                findings.push(Finding {
                    category: "project".into(),
                    severity: Severity::Error,
                    message: format!("project {id} head_ref '{href}' does not exist in refs"),
                    location: Some(format!("projects.{id}.head_ref")),
                });
            }
        }
    }

    // 4. Journal pending approvals.
    let pending: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM journal_entries WHERE user_approval_required = 1 \
             AND (user_approval_given IS NULL OR user_approval_given = 'Pending')",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    if pending > 0 {
        findings.push(Finding {
            category: "journal".into(),
            severity: Severity::Warn,
            message: format!("{pending} journal entries are awaiting approval"),
            location: Some("journal_entries".into()),
        });
    }

    // 5. Passport entries: parseable JSON.
    let mut bad_passport: Vec<i64> = Vec::new();
    let mut p_stmt = conn.prepare("SELECT id, payload_json FROM passport_entries")?;
    let p_rows = p_stmt.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in p_rows {
        let (id, payload) = row?;
        if serde_json::from_str::<serde_json::Value>(&payload).is_err() {
            bad_passport.push(id);
        }
    }
    if !bad_passport.is_empty() {
        findings.push(Finding {
            category: "passport".into(),
            severity: Severity::Error,
            message: format!(
                "{} passport entries have invalid JSON payloads (ids: {:?})",
                bad_passport.len(),
                &bad_passport[..bad_passport.len().min(5)],
            ),
            location: Some("passport_entries".into()),
        });
    }

    // 6. Refs naming convention (warn-only): expect either `<ulid>/main`, `<ulid>/iter-NNN`,
    // or the global `main` from the outer git emulation.
    let mut r_stmt = conn.prepare("SELECT name FROM refs")?;
    let r_rows = r_stmt.query_map([], |row| row.get::<_, String>(0))?;
    let r_pattern =
        regex::Regex::new(r"^[0-9A-Z]{26}/(main|iter-\d{3,})$|^main$|^wizard-sealed$").unwrap();
    for row in r_rows {
        let name = row?;
        if !r_pattern.is_match(&name) {
            findings.push(Finding {
                category: "refs".into(),
                severity: Severity::Info,
                message: format!("ref '{name}' does not match conventional naming"),
                location: Some(format!("refs.{name}")),
            });
        }
    }

    Ok(CheckReport::new("self", findings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentic_core::{
        db::open_in_memory,
        project::{ProjectKind, create as create_project},
    };
    use pretty_assertions::assert_eq;

    #[test]
    fn empty_db_warns_on_no_projects() {
        let conn = open_in_memory().unwrap();
        let report = run(&conn).unwrap();
        assert_eq!(report.verdict, crate::Verdict::Warn);
    }

    #[test]
    fn with_project_passes() {
        let conn = open_in_memory().unwrap();
        create_project(&conn, "P", ProjectKind::Thesis, "en", None).unwrap();
        let report = run(&conn).unwrap();
        assert_eq!(report.verdict, crate::Verdict::Pass);
    }

    #[test]
    fn dangling_head_ref_rejected_by_fk() {
        // Belt-and-braces: the FK should prevent us from creating a dangling
        // head_ref in the first place, so the run() branch never reports it
        // for a healthy DB. Verify the FK actually blocks the bad UPDATE.
        let conn = open_in_memory().unwrap();
        let pid = create_project(&conn, "P", ProjectKind::Thesis, "en", None).unwrap();
        let result = conn.execute(
            "UPDATE projects SET head_ref = ?1 WHERE id = ?2",
            rusqlite::params!["does-not-exist", pid],
        );
        assert!(result.is_err(), "FK should reject dangling head_ref");
    }

    #[test]
    fn passport_with_bad_json_flagged() {
        let conn = open_in_memory().unwrap();
        let pid = create_project(&conn, "P", ProjectKind::Thesis, "en", None).unwrap();
        // Bypass passport::append's validation to insert malformed JSON.
        conn.execute(
            "INSERT INTO passport_entries (project_id, section, payload_json) VALUES (?1, 'timeline', ?2)",
            rusqlite::params![pid, "not json"],
        ).unwrap();
        let report = run(&conn).unwrap();
        assert!(report.findings.iter().any(|f| f.category == "passport"));
    }

    #[test]
    fn worktree_ref_passes_naming() {
        let conn = open_in_memory().unwrap();
        let pid = create_project(&conn, "P", ProjectKind::Thesis, "en", None).unwrap();
        agentic_core::worktree::put_at(
            &conn,
            &pid,
            "x.md",
            b"hi",
            "text/markdown",
            None,
            "u",
            "init",
        )
        .unwrap();
        let report = run(&conn).unwrap();
        // worktree creates `<ulid>/main` which matches the pattern -> no INFO.
        assert!(!report.findings.iter().any(|f| f.category == "refs"));
    }
}
