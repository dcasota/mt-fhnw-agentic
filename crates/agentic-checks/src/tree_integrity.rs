//! Tree-integrity check — does the on-disk working tree still match the DB?
//!
//! The boot-time consistency gate. When source files (e.g. `specs/`, `inbox/`)
//! are materialised on disk *and* stored in the database, they can drift. This
//! check fails (Error → Verdict::Fail → exit 1) if any on-disk file differs from
//! its DB blob, warns on files present on disk but not yet ingested, and notes
//! (Info) DB paths not materialised on disk (expected when the DB is the sole
//! home of a file).

use std::path::Path;

use agentic_core::{Connection, worktree};
use anyhow::Result;

use crate::{CheckReport, Finding, Severity};

pub fn run(conn: &Connection, project_id: &str, root: &Path, prefix: &str) -> Result<CheckReport> {
    let rep = worktree::reconcile(conn, project_id, prefix, root)?;
    let mut findings = Vec::new();

    for p in rep.modified.iter().take(50) {
        findings.push(Finding {
            category: "tree-drift".into(),
            severity: Severity::Error,
            message: format!("on-disk file differs from its DB blob: {p}"),
            location: Some(p.clone()),
        });
    }
    if rep.modified.len() > 50 {
        findings.push(Finding {
            category: "tree-drift".into(),
            severity: Severity::Error,
            message: format!("...and {} more modified file(s)", rep.modified.len() - 50),
            location: None,
        });
    }
    if !rep.extra_on_disk.is_empty() {
        findings.push(Finding {
            category: "tree-untracked".into(),
            severity: Severity::Warn,
            message: format!(
                "{} on-disk file(s) are not in the DB (run `content ingest` to capture): {}",
                rep.extra_on_disk.len(),
                rep.extra_on_disk
                    .iter()
                    .take(8)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            location: None,
        });
    }
    if !rep.missing_on_disk.is_empty() {
        findings.push(Finding {
            category: "tree-unmaterialised".into(),
            severity: Severity::Info,
            message: format!(
                "{} DB path(s) not materialised on disk (expected; restore via `content checkout`)",
                rep.missing_on_disk.len()
            ),
            location: None,
        });
    }
    findings.push(Finding {
        category: "tree-ok".into(),
        severity: Severity::Info,
        message: format!("{} on-disk file(s) match the DB byte-for-byte", rep.matched),
        location: None,
    });

    Ok(CheckReport::new("tree", findings))
}
