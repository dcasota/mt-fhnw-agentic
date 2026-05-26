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
    scan_out_deprecated(root, &mut findings);

    findings.push(Finding {
        category: "tree-ok".into(),
        severity: Severity::Info,
        message: format!("{} on-disk file(s) match the DB byte-for-byte", rep.matched),
        location: None,
    });

    Ok(CheckReport::new("tree", findings))
}

/// `out/` working-tree deprecation guard. The `out/` working-tree prefix is
/// retired: authored content is DB-authoritative under `out/sources/` (the DB
/// keeps those paths — no rename) and is materialised only to ephemeral scratch
/// when a tool needs it; renders go to `snapshots/`. An on-disk `out/` folder in
/// the project tree is therefore a stale materialisation — regenerable from the
/// DB and not to be committed — so flag it for removal. WARN (not a hard stop)
/// while legacy trees are still being cleaned up.
fn scan_out_deprecated(root: &Path, findings: &mut Vec<Finding>) {
    let out_dir = root.join("out");
    if !out_dir.is_dir() {
        return;
    }
    let mut files = 0usize;
    let mut stack = vec![out_dir];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.filter_map(std::result::Result::ok) {
            if entry.path().is_dir() {
                stack.push(entry.path());
            } else {
                files += 1;
            }
        }
    }
    findings.push(Finding {
        category: "out-deprecated".into(),
        severity: Severity::Warn,
        message: format!(
            "on-disk out/ folder present ({files} file(s)) — out/ is deprecated as a working-tree path: \
             content is DB-authoritative under out/sources/ (materialised only to scratch when needed) and \
             renders go to snapshots/. Remove the working-tree out/ — it is regenerable from the DB and must \
             not be committed."
        ),
        location: Some("out/".into()),
    });
}

#[cfg(test)]
mod out_rule_tests {
    use super::*;

    #[test]
    fn flags_on_disk_out_folder_as_deprecated() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("treeout_{}_{nanos}", std::process::id()));
        // A working tree WITH an on-disk out/ → flagged (deprecated materialisation).
        std::fs::create_dir_all(root.join("out/sources/frontmatter")).unwrap();
        std::fs::write(root.join("out/sources/Dimension_01_EN.md"), b"x").unwrap();
        std::fs::write(root.join("out/book_manifest.json"), b"{}").unwrap();
        let mut findings = Vec::new();
        scan_out_deprecated(&root, &mut findings);
        let v = findings
            .iter()
            .find(|f| f.category == "out-deprecated")
            .expect("on-disk out/ flagged as deprecated");
        assert_eq!(v.severity, Severity::Warn);
        assert!(v.message.contains("2 file(s)"));
        std::fs::remove_dir_all(&root).ok();

        // A tree with NO out/ folder → no finding (the post-deprecation steady state).
        let clean = std::env::temp_dir().join(format!("treeclean_{}_{nanos}", std::process::id()));
        std::fs::create_dir_all(clean.join("snapshots/ts-books-cascade")).unwrap();
        let mut clean_findings = Vec::new();
        scan_out_deprecated(&clean, &mut clean_findings);
        assert!(
            !clean_findings
                .iter()
                .any(|f| f.category == "out-deprecated"),
            "no out/ folder ⇒ no deprecation finding"
        );
        std::fs::remove_dir_all(&clean).ok();
    }
}
