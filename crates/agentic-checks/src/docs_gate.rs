//! `agentic check docs` — mission-control documentation currency gate.
//!
//! Enforces CLAUDE.md operating-rule 9: every SDD-chain cycle/iteration must
//! keep the mission-control governance docs current. FAILs if a doc is missing;
//! WARNs if `PROGRESS.md`'s newest dated entry is older than the latest recorded
//! work (journal), i.e. an iteration closed without logging it.

use std::path::Path;
use std::sync::LazyLock;

use anyhow::Result;
use regex::Regex;
use rusqlite::Connection;

use crate::{CheckReport, Finding, Severity};

/// The mission-control docs that must be kept in sync each cycle.
pub const DOCS: &[&str] = &[
    "AGENTS.md",
    "ARCHITECTURE.md",
    "INSTRUCTIONS.md",
    "PROGRESS.md",
    "README.md",
    "TEMPLATE.md",
];

static DATE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d{4}-\d{2}-\d{2}").unwrap());

fn finding(cat: &str, sev: Severity, msg: String, loc: &str) -> Finding {
    Finding {
        category: cat.to_string(),
        severity: sev,
        message: msg,
        location: Some(loc.to_string()),
    }
}

/// Newest `YYYY-MM-DD` found in `text` (lexical max == chronological max).
fn newest_date(text: &str) -> Option<String> {
    DATE.find_iter(text).map(|m| m.as_str().to_string()).max()
}

pub fn run(conn: &Connection, project: &str, root: &Path) -> Result<CheckReport> {
    let mut findings = Vec::new();

    // 1. Every mission-control doc must exist on disk.
    for d in DOCS {
        if !root.join(d).is_file() {
            findings.push(finding(
                "DOC_MISSING",
                Severity::Error,
                format!("mission-control doc absent: {d}"),
                d,
            ));
        }
    }

    // 2. PROGRESS.md must be at least as recent as the latest journal entry.
    let latest_journal: Option<String> = conn
        .query_row(
            "SELECT MAX(ts) FROM journal_entries WHERE project_id = ?1",
            [project],
            |r| r.get::<_, Option<String>>(0),
        )
        .unwrap_or(None);
    let progress = std::fs::read_to_string(root.join("PROGRESS.md")).unwrap_or_default();
    if let Some(j) = latest_journal {
        let jd = j.get(0..10).unwrap_or("").to_string(); // YYYY-MM-DD
        match newest_date(&progress) {
            Some(p) if !jd.is_empty() && p.as_str() < jd.as_str() => findings.push(finding(
                "PROGRESS_STALE",
                Severity::Warn,
                format!(
                    "PROGRESS.md newest entry {p} is older than the latest journal entry {jd} — append this cycle's iteration entry"
                ),
                "PROGRESS.md",
            )),
            None if !progress.is_empty() => findings.push(finding(
                "PROGRESS_NO_DATE",
                Severity::Warn,
                "PROGRESS.md has no dated entry to compare against the journal".to_string(),
                "PROGRESS.md",
            )),
            _ => {}
        }
    }

    Ok(CheckReport::new("docs", findings))
}
