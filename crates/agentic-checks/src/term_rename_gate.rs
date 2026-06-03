//! `agentic check term-rename` — deprecated-token surveillance gate.
//!
//! Why: after a term rename (e.g. "12-week AI-First plan" →
//! "100-days AI-First plan", 2026-05-31), occurrences of the old token
//! drift back in over time — as new prose, as forgotten variants
//! ("12-week-and-9-month"), or as meta-commentary about the rename.
//! This gate keeps every project-declared deprecated token under
//! continuous, case-insensitive surveillance across the live worktree,
//! excluding the small set of intentionally-historical paths.
//!
//! ## Registry format
//!
//! `specs/deprecated-terms.json` in the project worktree:
//!
//! ```json
//! {
//!   "version": 1,
//!   "terms": [
//!     {
//!       "token": "12-week",
//!       "replacement": "100-days",
//!       "since": "2026-05-31",
//!       "severity": "warn",
//!       "note": "Appendix A rename — see PROGRESS.md entry"
//!     }
//!   ]
//! }
//! ```
//!
//! ## Exclusion rules
//!
//! These paths preserve historical state by design and are not scanned:
//!
//!   - `inbox/*`               — raw user input (brownfield etiquette)
//!   - `thesis-draft-v*/`      — frozen prior-iteration snapshots
//!   - `proposal/citations/*`  — historical citation text
//!   - `out/AUDIT_*`           — regenerable audit reports (commit-msg ledger)
//!   - `out/sources/AI_Audit_BOM_EN.md` — chronological AIBOM ledger
//!   - any blob whose name contains `CHANGELOG`
//!   - `specs/deprecated-terms.json` — the registry itself

use std::collections::HashMap;

use anyhow::Result;
use rusqlite::Connection;
use serde::Deserialize;

use agentic_core::worktree;

use crate::{CheckReport, Finding, Severity};

/// One deprecated-term row in the registry.
#[derive(Debug, Clone, Deserialize)]
pub struct DeprecatedTerm {
    pub token: String,
    #[serde(default)]
    pub replacement: Option<String>,
    #[serde(default)]
    pub since: Option<String>,
    #[serde(default = "default_severity")]
    pub severity: String,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct Registry {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    terms: Vec<DeprecatedTerm>,
}

fn default_severity() -> String {
    "warn".into()
}

/// Registry path in the project worktree.
pub const REGISTRY_PATH: &str = "specs/deprecated-terms.json";

/// Return true if `path` is in the per-design historical-preservation set.
///
/// The mission-control documents `PROGRESS.md` and any `CHANGELOG*` are
/// the project's chronological history of past iterations — they
/// intentionally mention deprecated tokens in past-tense narration
/// ("the 2026-05-31 12-week → 100-days rename") and would otherwise
/// produce one finding per historical entry. We exclude them by name
/// rather than by heuristic because the surveillance is meant for *new
/// content drift*, not for retrospective entries about the rename event
/// itself.
#[must_use]
pub fn is_excluded(path: &str) -> bool {
    path.starts_with("inbox/")
        || path.starts_with("thesis-draft-v")
        || path.starts_with("proposal/citations/")
        || path.starts_with("out/AUDIT_")
        || path == "out/sources/AI_Audit_BOM_EN.md"
        || path == "PROGRESS.md"
        || path.contains("CHANGELOG")
        || path == REGISTRY_PATH
}

/// Parse a severity string (case-insensitive) into a [`Severity`].
fn parse_severity(s: &str) -> Severity {
    match s.to_ascii_lowercase().as_str() {
        "error" | "err" | "fail" => Severity::Error,
        "info" => Severity::Info,
        _ => Severity::Warn,
    }
}

/// Run the gate using the registry at [`REGISTRY_PATH`]. Returns an empty
/// report (with an INFO finding) when the registry is absent.
pub fn run(conn: &Connection, project: &str) -> Result<CheckReport> {
    let terms = match load_registry(conn, project)? {
        Some(t) => t,
        None => {
            return Ok(CheckReport::new(
                "term_rename",
                vec![Finding {
                    category: "TERM_RENAME_NO_REGISTRY".into(),
                    severity: Severity::Info,
                    message: format!(
                        "no {REGISTRY_PATH} in worktree — gate runs no checks; \
                         create one to surveil deprecated tokens"
                    ),
                    location: Some(REGISTRY_PATH.into()),
                }],
            ));
        }
    };
    run_with_terms(conn, project, &terms)
}

/// Run the gate with an explicit term list (ad-hoc CLI use).
pub fn run_with_terms(
    conn: &Connection,
    project: &str,
    terms: &[DeprecatedTerm],
) -> Result<CheckReport> {
    let mut findings = Vec::new();
    let mut hits_by_term: HashMap<String, usize> = HashMap::new();

    let all = worktree::list(conn, project, "")?;
    for (path, _sha) in &all {
        if is_excluded(path) {
            continue;
        }
        let blob = match worktree::read_at(conn, project, path) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let Ok(text) = std::str::from_utf8(&blob.content) else {
            continue;
        };
        for term in terms {
            scan_text(path, text, term, &mut findings, &mut hits_by_term);
        }
    }

    let remaining_count = findings
        .iter()
        .filter(|f| f.category == "TERM_RENAME_REMAINING")
        .count();
    findings.push(Finding {
        category: "TERM_RENAME_SUMMARY".into(),
        severity: Severity::Info,
        message: format!(
            "scanned {} term(s); {} hit(s) total: {}",
            terms.len(),
            remaining_count,
            hits_by_term
                .iter()
                .map(|(t, n)| format!("{t}={n}"))
                .collect::<Vec<_>>()
                .join(", "),
        ),
        location: None,
    });

    Ok(CheckReport::new("term_rename", findings))
}

fn scan_text(
    path: &str,
    text: &str,
    term: &DeprecatedTerm,
    findings: &mut Vec<Finding>,
    hits_by_term: &mut HashMap<String, usize>,
) {
    if term.token.is_empty() {
        return;
    }
    let needle = term.token.to_ascii_lowercase();
    let severity = parse_severity(&term.severity);
    for (lineno, line) in text.lines().enumerate() {
        let mut from = 0usize;
        let line_lc = line.to_ascii_lowercase();
        while let Some(at) = line_lc[from..].find(&needle) {
            let abs = from + at;
            let snippet = trim_snippet(line, abs, term.token.len());
            findings.push(Finding {
                category: "TERM_RENAME_REMAINING".into(),
                severity,
                message: format!(
                    "deprecated token '{}' still present (replace with '{}'{}{}): {}",
                    term.token,
                    term.replacement
                        .as_deref()
                        .unwrap_or("(no replacement declared)"),
                    term.since
                        .as_deref()
                        .map(|d| format!(", since {d}"))
                        .unwrap_or_default(),
                    term.note
                        .as_deref()
                        .map(|n| format!(", {n}"))
                        .unwrap_or_default(),
                    snippet
                ),
                location: Some(format!("{}:{}:{}", path, lineno + 1, abs + 1)),
            });
            *hits_by_term.entry(term.token.clone()).or_insert(0) += 1;
            from = abs + needle.len().max(1);
            if from >= line_lc.len() {
                break;
            }
        }
    }
}

fn trim_snippet(line: &str, at: usize, len: usize) -> String {
    let start = at.saturating_sub(30);
    let end = (at + len + 30).min(line.len());
    let mut s = String::new();
    if start > 0 {
        s.push('…');
    }
    s.push_str(line.get(start..end).unwrap_or(""));
    if end < line.len() {
        s.push('…');
    }
    s
}

fn load_registry(conn: &Connection, project: &str) -> Result<Option<Vec<DeprecatedTerm>>> {
    let blob = match worktree::read_at(conn, project, REGISTRY_PATH) {
        Ok(b) => b,
        Err(_) => return Ok(None),
    };
    let text = std::str::from_utf8(&blob.content)
        .map_err(|e| anyhow::anyhow!("{REGISTRY_PATH} is not UTF-8: {e}"))?;
    let reg: Registry = serde_json::from_str(text)
        .map_err(|e| anyhow::anyhow!("{REGISTRY_PATH} is not valid JSON: {e}"))?;
    let _ = reg.version; // reserved for future schema migration
    Ok(Some(reg.terms))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_excluded_skips_frozen_paths() {
        assert!(is_excluded("inbox/initial-formulation.md"));
        assert!(is_excluded("thesis-draft-v5/fhnw_1_introduction.md"));
        assert!(is_excluded("proposal/citations/1 Agile/1 Manifesto.md"));
        assert!(is_excluded("out/AUDIT_latest_EN.md"));
        assert!(is_excluded("out/AUDIT_latest_EN.json"));
        assert!(is_excluded("out/sources/AI_Audit_BOM_EN.md"));
        assert!(is_excluded("PROGRESS.md"));
        assert!(is_excluded("CHANGELOG.md"));
        assert!(is_excluded("specs/deprecated-terms.json"));
    }

    #[test]
    fn is_excluded_allows_live_paths() {
        assert!(!is_excluded("thesis/fhnw_1_introduction.md"));
        assert!(!is_excluded(
            "out/sources/Dimension_01_agile_leadership_EN.md"
        ));
        assert!(!is_excluded("specs/adr/0001-foo.md"));
        assert!(!is_excluded("PROGRESS.md"));
    }

    #[test]
    fn case_insensitive_scan_finds_all_variants() {
        let mut findings: Vec<Finding> = Vec::new();
        let mut hits = HashMap::new();
        let term = DeprecatedTerm {
            token: "12-week".into(),
            replacement: Some("100-days".into()),
            since: Some("2026-05-31".into()),
            severity: "warn".into(),
            note: None,
        };
        let text = "12-week plan\n\
                    A 12-Week note\n\
                    something 12-week-and-9-month here\n\
                    100-days plan\n";
        scan_text("test.md", text, &term, &mut findings, &mut hits);
        let hits_count = findings
            .iter()
            .filter(|f| f.category == "TERM_RENAME_REMAINING")
            .count();
        assert_eq!(hits_count, 3);
        assert_eq!(hits.get("12-week").copied(), Some(3));
    }

    #[test]
    fn empty_token_is_silent() {
        let mut findings = Vec::new();
        let mut hits = HashMap::new();
        let term = DeprecatedTerm {
            token: String::new(),
            replacement: None,
            since: None,
            severity: "warn".into(),
            note: None,
        };
        scan_text("test.md", "some text", &term, &mut findings, &mut hits);
        assert!(findings.is_empty());
    }

    #[test]
    fn parse_severity_handles_aliases() {
        assert!(matches!(parse_severity("Error"), Severity::Error));
        assert!(matches!(parse_severity("WARN"), Severity::Warn));
        assert!(matches!(parse_severity("info"), Severity::Info));
        assert!(matches!(parse_severity("garbage"), Severity::Warn));
    }
}
