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

/// Secondary governance representations (ADR-0047 R10): the canonical cascade
/// schema and the cross-tool mission-control agent-defs. A missing one — or one
/// that no longer references the SDD-chain canonical model — is representation
/// drift (advisory: these are derived/secondary, not the 6 core docs).
pub const REPRESENTATIONS: &[&str] = &[
    "CASCADE_PIPELINE.md",
    ".claude/agents/mission-control.md",
    ".factory/droids/mission-control.md",
];

static DATE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d{4}-\d{2}-\d{2}").unwrap());

/// Does any `specs/adr/0047*` canonical-contract ADR exist under `root`?
fn canonical_adr_present(root: &Path) -> bool {
    std::fs::read_dir(root.join("specs/adr"))
        .map(|rd| {
            rd.filter_map(std::result::Result::ok)
                .any(|e| e.file_name().to_string_lossy().starts_with("0047"))
        })
        .unwrap_or(false)
}

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

    // 3. Representation drift (ADR-0047 R10): the secondary governance
    // representations must exist and still reflect the SDD-chain canonical.
    for r in REPRESENTATIONS {
        let path = root.join(r);
        let Ok(text) = std::fs::read_to_string(&path) else {
            findings.push(finding(
                "REPRESENTATION_MISSING",
                Severity::Warn,
                format!("governance representation absent: {r}"),
                r,
            ));
            continue;
        };
        if !text.to_lowercase().contains("sdd") {
            findings.push(finding(
                "REPRESENTATION_DRIFT",
                Severity::Warn,
                format!("{r} no longer references the SDD-chain canonical model"),
                r,
            ));
        }
    }

    // 4. The canonical cascade contract (ADR-0047) must exist as the source the
    // other representations derive from / point to.
    if !canonical_adr_present(root) {
        findings.push(finding(
            "CANONICAL_ABSENT",
            Severity::Warn,
            "no specs/adr/0047* canonical cascade-contract ADR found".to_string(),
            "specs/adr",
        ));
    }

    Ok(CheckReport::new("docs", findings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentic_core::{
        db::open_in_memory,
        project::{ProjectKind, create as create_project},
    };

    fn populated_root() -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("docsgate_{}_{nanos}", std::process::id()));
        std::fs::create_dir_all(root.join("specs/adr")).unwrap();
        for d in DOCS {
            std::fs::write(root.join(d), "2026-05-26 entry").unwrap();
        }
        for r in REPRESENTATIONS {
            let p = root.join(r);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "references the SDD-chain canonical").unwrap();
        }
        std::fs::write(
            root.join("specs/adr/0047-cascade-parameterised-family.md"),
            "x",
        )
        .unwrap();
        root
    }

    #[test]
    fn complete_governance_set_has_no_representation_findings() {
        let conn = open_in_memory().unwrap();
        let pid = create_project(&conn, "T", ProjectKind::Thesis, "en", None).unwrap();
        let root = populated_root();
        let report = run(&conn, &pid, &root).unwrap();
        assert!(!report.findings.iter().any(|f| {
            matches!(
                f.category.as_str(),
                "REPRESENTATION_MISSING"
                    | "REPRESENTATION_DRIFT"
                    | "CANONICAL_ABSENT"
                    | "DOC_MISSING"
            )
        }));
        // Removing a representation surfaces drift; a non-SDD body surfaces drift.
        std::fs::remove_file(root.join("CASCADE_PIPELINE.md")).unwrap();
        std::fs::write(
            root.join(".claude/agents/mission-control.md"),
            "no canonical ref",
        )
        .unwrap();
        let report = run(&conn, &pid, &root).unwrap();
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.category == "REPRESENTATION_MISSING")
        );
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.category == "REPRESENTATION_DRIFT")
        );
        std::fs::remove_dir_all(&root).ok();
    }
}
