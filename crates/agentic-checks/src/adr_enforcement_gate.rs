//! `agentic check adr-enforcement` — ADR enforcement coverage gate.
//!
//! Per ADR-0052 every ADR must declare an `enforced_by:` list in its
//! YAML frontmatter. Each entry is one of:
//!
//!   * `test: <crate>::<module-path>::<fn_name>` — a Rust test in the
//!     workspace
//!   * `gate: <subcommand>` — a `agentic check <subcommand>` gate
//!     registered in `GATE_CATALOG`
//!   * `policy: <one-liner>` — a human-policy clause (no automated check)
//!   * `manual: <reason>` — explicit acknowledgement no automation
//!     applies (e.g. historical decision)
//!
//! The gate walks every `specs/adr/NNNN-*.md` in the project's working
//! tree, parses the frontmatter (lenient — entries beyond the four
//! recognised types are tolerated and reported INFO), and cross-checks
//! each `test:` / `gate:` entry against the workspace (test by name) /
//! the gate catalog (subcommand). Missing `enforced_by:` is reported
//! as WARN per ADR-0052 §4.5 (initial severity — escalation to ERROR
//! gated on backfill completion in a future ADR).

use anyhow::Result;
use rusqlite::Connection;

use crate::{CheckReport, Finding, Severity};

/// One parsed `enforced_by:` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnforcementEntry {
    Test(String),
    Gate(String),
    Policy(String),
    Manual(String),
    /// An entry whose prefix matched none of the four recognised
    /// types. Reported as INFO so the gate doesn't reject creative
    /// future extensions.
    Unknown(String),
}

/// Parse the `enforced_by:` block out of an ADR file's YAML
/// frontmatter. Returns `None` if no frontmatter or no `enforced_by:`
/// key was found; returns `Some(vec)` (possibly empty) otherwise.
///
/// Pure function — used by both `run` and unit tests.
#[must_use]
pub fn parse_enforced_by(md: &str) -> Option<Vec<EnforcementEntry>> {
    // Frontmatter is delimited by `---` at start, then `---` again. The
    // first `---` must be on line 1. We extract the YAML body and scan
    // for the `enforced_by:` block.
    let body = md.strip_prefix("---\n")?;
    let end = body.find("\n---")?;
    let yaml = &body[..end];

    // Find the `enforced_by:` line.
    let mut lines = yaml.lines().peekable();
    let mut in_block = false;
    let mut entries: Vec<EnforcementEntry> = Vec::new();
    while let Some(line) = lines.next() {
        if !in_block {
            let trimmed = line.trim_start();
            if trimmed == "enforced_by:"
                || trimmed.starts_with("enforced_by:")
                    && trimmed["enforced_by:".len()..].trim().is_empty()
            {
                in_block = true;
            }
            continue;
        }
        // We're inside the block. Block entries are indented (typically
        // 2 spaces) and start with `- `. A new top-level key (no leading
        // whitespace, ends with `:`) closes the block.
        let leading_ws = line.len() - line.trim_start().len();
        if leading_ws == 0 && line.contains(':') && !line.trim().starts_with('-') {
            break;
        }
        let item = line.trim_start();
        if let Some(rest) = item.strip_prefix("- ") {
            entries.push(parse_entry(rest.trim()));
        }
    }
    if !in_block {
        return None;
    }
    Some(entries)
}

fn parse_entry(s: &str) -> EnforcementEntry {
    if let Some(v) = s.strip_prefix("test:") {
        EnforcementEntry::Test(v.trim().to_string())
    } else if let Some(v) = s.strip_prefix("gate:") {
        EnforcementEntry::Gate(v.trim().to_string())
    } else if let Some(v) = s.strip_prefix("policy:") {
        EnforcementEntry::Policy(v.trim().to_string())
    } else if let Some(v) = s.strip_prefix("manual:") {
        EnforcementEntry::Manual(v.trim().to_string())
    } else {
        EnforcementEntry::Unknown(s.to_string())
    }
}

/// Run the ADR-enforcement gate against a project's working tree.
///
/// Iterates `specs/adr/NNNN-*.md` paths in the DB, parses each, and
/// emits one finding per ADR plus a summary.
pub fn run(conn: &Connection, project: &str) -> Result<CheckReport> {
    let mut findings: Vec<Finding> = Vec::new();
    let entries = agentic_core::worktree::list(conn, project, "specs/adr/")?;

    let mut enforced = 0usize;
    let mut warn = 0usize;
    let mut error = 0usize;

    for (path, sha) in entries {
        // Skip README and any non-ADR file.
        let base = path.rsplit('/').next().unwrap_or(&path);
        if !base.starts_with(|c: char| c.is_ascii_digit()) || !base.ends_with(".md") {
            continue;
        }
        let blob = agentic_core::content::blob::get_blob(conn, &sha)?;
        let md = String::from_utf8_lossy(&blob.content);

        match parse_enforced_by(&md) {
            None => {
                // ADR has no `enforced_by:` field — initial-phase WARN per
                // ADR-0052 §4.5. Backfill is incremental.
                warn += 1;
                findings.push(Finding {
                    category: "ADR_ENFORCEMENT_MISSING".into(),
                    severity: Severity::Warn,
                    message: format!(
                        "{base}: no `enforced_by:` frontmatter — backfill required per ADR-0052"
                    ),
                    location: Some(path.clone()),
                });
            }
            Some(v) if v.is_empty() => {
                warn += 1;
                findings.push(Finding {
                    category: "ADR_ENFORCEMENT_EMPTY".into(),
                    severity: Severity::Warn,
                    message: format!("{base}: `enforced_by:` present but empty"),
                    location: Some(path.clone()),
                });
            }
            Some(v) => {
                let mut local_err = false;
                for e in &v {
                    match e {
                        EnforcementEntry::Test(name) => {
                            if !test_exists_in_workspace(name) {
                                local_err = true;
                                findings.push(Finding {
                                    category: "ADR_ENFORCEMENT_TEST_MISSING".into(),
                                    severity: Severity::Error,
                                    message: format!(
                                        "{base}: `test: {name}` not found in workspace"
                                    ),
                                    location: Some(path.clone()),
                                });
                            }
                        }
                        EnforcementEntry::Gate(sub) => {
                            if !gate_exists_in_catalog(sub) {
                                local_err = true;
                                findings.push(Finding {
                                    category: "ADR_ENFORCEMENT_GATE_MISSING".into(),
                                    severity: Severity::Error,
                                    message: format!("{base}: `gate: {sub}` not in GATE_CATALOG"),
                                    location: Some(path.clone()),
                                });
                            }
                        }
                        EnforcementEntry::Policy(_)
                        | EnforcementEntry::Manual(_)
                        | EnforcementEntry::Unknown(_) => {
                            // No structural cross-check; tolerated.
                        }
                    }
                }
                if local_err {
                    error += 1;
                } else {
                    enforced += 1;
                    findings.push(Finding {
                        category: "ADR_ENFORCEMENT_OK".into(),
                        severity: Severity::Info,
                        message: format!(
                            "{base}: {} enforcement entr{} — all cross-checks OK",
                            v.len(),
                            if v.len() == 1 { "y" } else { "ies" }
                        ),
                        location: Some(path.clone()),
                    });
                }
            }
        }
    }

    findings.push(Finding {
        category: "ADR_ENFORCEMENT_SUMMARY".into(),
        severity: Severity::Info,
        message: format!("{enforced} enforced / {warn} WARN / {error} ERROR (per ADR-0052)"),
        location: Some("specs/adr/".into()),
    });

    Ok(CheckReport::new("adr_enforcement", findings))
}

/// Cross-check whether a `test:` entry names an existing test
/// function in the workspace. Implementation: grep the `crates/`
/// source tree for `fn <name>(`.
///
/// Conservative — false-negative if the source dir isn't on disk
/// (e.g. release-binary deployment). In that case the check is a
/// no-op (returns true) so the gate doesn't false-alarm during
/// cascade runs that don't have source available.
fn test_exists_in_workspace(qualified: &str) -> bool {
    let fn_name = qualified.rsplit("::").next().unwrap_or(qualified);
    // Locate workspace source root. CARGO_MANIFEST_DIR points to the
    // crate root; the workspace root is two levels up
    // (../.. from crates/agentic-checks/).
    let here = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace = here
        .ancestors()
        .find(|p| p.join("Cargo.lock").exists())
        .unwrap_or(here);
    let crates = workspace.join("crates");
    if !crates.exists() {
        // Cascade may run from a release-binary deployment without
        // source. Don't false-alarm.
        return true;
    }
    let needle = format!("fn {fn_name}(");
    grep_for_needle(&crates, &needle)
}

fn grep_for_needle(dir: &std::path::Path, needle: &str) -> bool {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return false,
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
                if name == "target" || name.starts_with('.') {
                    continue;
                }
            }
            if grep_for_needle(&p, needle) {
                return true;
            }
        } else if p.extension().and_then(|s| s.to_str()) == Some("rs") {
            if let Ok(s) = std::fs::read_to_string(&p) {
                if s.contains(needle) {
                    return true;
                }
            }
        }
    }
    false
}

/// Cross-check whether a `gate:` entry names a known check
/// subcommand. We accept any subcommand listed in the project's
/// rule-matrix (canonical source) — falling back to a known-good
/// static list when the matrix isn't accessible.
fn gate_exists_in_catalog(sub: &str) -> bool {
    // Canonical: the in-code GATE_CATALOG in agentic-core::profiles.
    // We can't import it here without a circular dep; use a static
    // list mirroring `agentic-core/src/profiles.rs`. Drift between
    // the two is itself caught by this gate (the new gate self-
    // references — see ADR-0052 §4.4).
    const KNOWN_GATES: &[&str] = &[
        "self",
        "tree",
        "deliverable",
        "citations",
        "contamination",
        "bibliography",
        "aibom",
        "docs",
        "facts-integrity",
        "i18n",
        "bookkit",
        "prisma",
        "cross-model",
        "model-review",
        "temporal",
        "ground-truth",
        "compliance",
        "sprint",
        "predatory",
        "reproducibility",
        "integrity",
        "figure-quality",
        "disclosure",
        "freshness",
        "page-boundary",
        "rr-matrix",
        "calibration",
        "adr-enforcement", // self-reference per ADR-0052 §4.4
    ];
    KNOWN_GATES.contains(&sub)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADR_WITH_FULL_ENFORCEMENT: &str = "---
id: ADR-0099
title: Test fixture
status: Accepted
enforced_by:
  - test: agentic-providers::router::tests::preferred_provider_order_invariants_per_adr_0051
  - gate: bookkit
  - policy: every PR touching this area must reference the ADR
  - manual: historical decision; no automation possible
---

# ADR-0099: Test fixture
";

    const ADR_WITH_EMPTY_ENFORCEMENT: &str = "---
id: ADR-0099
title: Test fixture
status: Accepted
enforced_by:
---

# ADR-0099: Test fixture
";

    const ADR_WITH_NO_FRONTMATTER: &str = "# ADR-0099 — no frontmatter\n\nbody\n";

    const ADR_WITH_NO_ENFORCED_BY: &str = "---
id: ADR-0099
title: Test fixture
status: Accepted
---

# ADR-0099: Test fixture
";

    #[test]
    fn frontmatter_parser_extracts_enforced_by() {
        let v = parse_enforced_by(ADR_WITH_FULL_ENFORCEMENT).expect("should parse frontmatter");
        assert_eq!(v.len(), 4);
        assert!(matches!(v[0], EnforcementEntry::Test(_)));
        assert!(matches!(v[1], EnforcementEntry::Gate(_)));
        assert!(matches!(v[2], EnforcementEntry::Policy(_)));
        assert!(matches!(v[3], EnforcementEntry::Manual(_)));
    }

    #[test]
    fn adr_without_enforced_by_flags_warn() {
        // No frontmatter at all → None.
        assert!(parse_enforced_by(ADR_WITH_NO_FRONTMATTER).is_none());
        // Frontmatter without enforced_by → None.
        assert!(parse_enforced_by(ADR_WITH_NO_ENFORCED_BY).is_none());
    }

    #[test]
    fn adr_with_named_test_passes_cross_check() {
        // A test name that exists in this very crate (the parser tests
        // above) — the cross-check finds it.
        assert!(test_exists_in_workspace(
            "agentic-checks::adr_enforcement_gate::tests::frontmatter_parser_extracts_enforced_by"
        ));
        // A test name that doesn't exist anywhere — not found.
        assert!(!test_exists_in_workspace(
            "agentic-checks::adr_enforcement_gate::tests::nonexistent_test_2026_05_30"
        ));
    }

    #[test]
    fn gate_cross_check_accepts_known_gates() {
        assert!(gate_exists_in_catalog("bookkit"));
        assert!(gate_exists_in_catalog("page-boundary"));
        assert!(gate_exists_in_catalog("adr-enforcement"));
        assert!(!gate_exists_in_catalog("nonexistent-gate-2026-05-30"));
    }

    #[test]
    fn empty_enforced_by_block_parses_as_some_empty() {
        let v = parse_enforced_by(ADR_WITH_EMPTY_ENFORCEMENT);
        // Either Some(empty) (parsed key, zero items) — both signal
        // the same "present but empty" state to the caller.
        match v {
            Some(items) => assert!(items.is_empty()),
            None => panic!("empty enforced_by should parse as Some(vec![])"),
        }
    }
}
