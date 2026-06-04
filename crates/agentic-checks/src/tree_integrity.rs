//! Tree-integrity check — does the on-disk working tree still match the DB?
//!
//! The boot-time consistency gate. When source files (e.g. `specs/`, `inbox/`)
//! are materialised on disk *and* stored in the database, they can drift. This
//! check fails (Error → Verdict::Fail → exit 1) if any on-disk file differs from
//! its DB blob, warns on files present on disk but not yet ingested, and notes
//! (Info) DB paths not materialised on disk (expected when the DB is the sole
//! home of a file).

use std::collections::BTreeMap;
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

// ─────────────────────────────────────────────────────────────────────────
// ADR-0061 §3.4 — thesis-content drift guard
// ─────────────────────────────────────────────────────────────────────────

/// Compare the project's current `thesis/*.md` blob SHAs against a baseline
/// snapshot taken at cascade start. Used by the `master_thesis_bookkit`
/// cascade to enforce the "no content edits during a bookkit cascade"
/// constraint codified by ADR-0061 §3.4 (policy
/// `master-thesis-bookkit-no-thesis-content-edit`).
///
/// Returns one [`Finding`] per drifted `thesis/*.md` path (severity
/// `Severity::Error`) PLUS an info marker reporting how many thesis files
/// were inspected. If `baseline` is empty (no snapshot supplied), the gate
/// emits a single WARN — the cascade should always record a baseline first.
///
/// `baseline` maps `thesis/<file>.md → blob_sha_hex` as recorded by the
/// cascade harness before any wave-1 work runs. `current` is the DB's
/// current view of the same prefix (typically from
/// `agentic_core::worktree::list(conn, project_id, "thesis/")`).
pub fn scan_thesis_content_drift(
    baseline: &BTreeMap<String, String>,
    current: &BTreeMap<String, String>,
) -> Vec<Finding> {
    let mut findings = Vec::new();
    if baseline.is_empty() {
        findings.push(Finding {
            category: "thesis-drift-no-baseline".into(),
            severity: Severity::Warn,
            message: "no cascade baseline supplied — cannot enforce thesis-content immutability \
                      (ADR-0061 §3.4); the bookkit cascade harness must record a baseline before \
                      running work"
                .into(),
            location: Some("thesis/".into()),
        });
        return findings;
    }
    let mut inspected = 0usize;
    for (path, baseline_sha) in baseline {
        if !path.starts_with("thesis/") || !path.ends_with(".md") {
            continue;
        }
        inspected += 1;
        match current.get(path) {
            Some(cur_sha) if cur_sha == baseline_sha => { /* clean */ }
            Some(cur_sha) => findings.push(Finding {
                category: "thesis-content-drift".into(),
                severity: Severity::Error,
                message: format!(
                    "{path}: blob SHA changed during bookkit cascade (baseline={}, current={}); \
                     ADR-0061 §3.4 forbids thesis-content edits during a `master_thesis_bookkit` \
                     cascade (policy: master-thesis-bookkit-no-thesis-content-edit)",
                    short_sha(baseline_sha),
                    short_sha(cur_sha),
                ),
                location: Some(path.clone()),
            }),
            None => findings.push(Finding {
                category: "thesis-content-removed".into(),
                severity: Severity::Error,
                message: format!(
                    "{path}: present at cascade baseline (blob {}) but absent from current tree; \
                     ADR-0061 §3.4 forbids removing thesis content during a bookkit cascade",
                    short_sha(baseline_sha),
                ),
                location: Some(path.clone()),
            }),
        }
    }
    findings.push(Finding {
        category: "thesis-content-ok".into(),
        severity: if findings.iter().any(|f| f.severity == Severity::Error) {
            // If any drift was recorded, the summary is informational only.
            Severity::Info
        } else {
            Severity::Info
        },
        message: format!(
            "{inspected} thesis/*.md path(s) inspected against cascade baseline; \
             {} drift(s) detected",
            findings
                .iter()
                .filter(|f| f.severity == Severity::Error)
                .count()
        ),
        location: Some("thesis/".into()),
    });
    findings
}

fn short_sha(sha: &str) -> String {
    sha.chars().take(12).collect()
}

#[cfg(test)]
mod thesis_content_drift_tests {
    use super::*;
    use std::collections::BTreeMap;

    fn baseline() -> BTreeMap<String, String> {
        let mut m = BTreeMap::new();
        m.insert(
            "thesis/fhnw_1_introduction.md".to_string(),
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        );
        m.insert(
            "thesis/fhnw_2_theory.md".to_string(),
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
        );
        m
    }

    /// Clean run: current matches baseline ⇒ no error findings, only the
    /// info summary.
    #[test]
    fn clean_run_emits_no_error() {
        let base = baseline();
        let current = base.clone();
        let findings = scan_thesis_content_drift(&base, &current);
        assert!(
            !findings.iter().any(|f| f.severity == Severity::Error),
            "{findings:?}"
        );
        assert!(
            findings.iter().any(|f| f.category == "thesis-content-ok"
                && f.message.contains("2 thesis/*.md")
                && f.message.contains("0 drift(s)")),
            "{findings:?}"
        );
    }

    /// Mutation: fhnw_1_introduction.md blob SHA changed ⇒ ERROR finding,
    /// labelled `thesis-content-drift`.
    ///
    /// This is the test that demonstrates the guard catches a fake
    /// `thesis/*.md` change — required by Wave-1 brief task 5.
    #[test]
    fn guards_against_thesis_md_blob_change() {
        let base = baseline();
        let mut current = base.clone();
        current.insert(
            "thesis/fhnw_1_introduction.md".to_string(),
            "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc".to_string(),
        );
        let findings = scan_thesis_content_drift(&base, &current);
        let drift = findings
            .iter()
            .find(|f| f.category == "thesis-content-drift")
            .expect("drift finding emitted");
        assert_eq!(drift.severity, Severity::Error);
        assert_eq!(
            drift.location.as_deref(),
            Some("thesis/fhnw_1_introduction.md")
        );
        assert!(
            drift.message.contains("ADR-0061"),
            "drift message must cite ADR-0061: {}",
            drift.message
        );
        // The clean file must NOT show a drift finding.
        assert!(
            !findings.iter().any(|f| f.category == "thesis-content-drift"
                && f.location.as_deref() == Some("thesis/fhnw_2_theory.md"))
        );
    }

    /// Removed file ⇒ ERROR with category `thesis-content-removed`.
    #[test]
    fn flags_removed_thesis_file() {
        let base = baseline();
        let mut current = base.clone();
        current.remove("thesis/fhnw_2_theory.md");
        let findings = scan_thesis_content_drift(&base, &current);
        let removed = findings
            .iter()
            .find(|f| f.category == "thesis-content-removed")
            .expect("removed finding emitted");
        assert_eq!(removed.severity, Severity::Error);
    }

    /// Missing baseline ⇒ WARN, no errors.
    #[test]
    fn warns_when_baseline_absent() {
        let base: BTreeMap<String, String> = BTreeMap::new();
        let current = baseline();
        let findings = scan_thesis_content_drift(&base, &current);
        let warn = findings
            .iter()
            .find(|f| f.category == "thesis-drift-no-baseline")
            .expect("no-baseline warn emitted");
        assert_eq!(warn.severity, Severity::Warn);
        assert!(!findings.iter().any(|f| f.severity == Severity::Error));
    }

    /// Non-thesis paths in the baseline are ignored.
    #[test]
    fn ignores_non_thesis_paths_in_baseline() {
        let mut base = baseline();
        base.insert(
            "out/sources/foo.md".to_string(),
            "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff".to_string(),
        );
        let mut current = base.clone();
        // Drift in non-thesis path — should be ignored.
        current.insert(
            "out/sources/foo.md".to_string(),
            "1111111111111111111111111111111111111111111111111111111111111111".to_string(),
        );
        let findings = scan_thesis_content_drift(&base, &current);
        assert!(!findings.iter().any(|f| f.severity == Severity::Error));
    }
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
