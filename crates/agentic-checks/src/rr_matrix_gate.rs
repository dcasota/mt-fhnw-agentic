//! `agentic check rr-matrix` — revise-and-resubmit traceability matrix (ADR-0044).
//!
//! The ARS R&R Traceability Matrix (Schema 11) tracks each reviewer point to the
//! author's response, the change location, and an independent `Verified?` flag.
//! When a deliverable contains such a matrix (a markdown table whose header row
//! carries a "Verified" column), this gate validates that the required columns
//! are present:
//!
//!   * reviewer point/comment,
//!   * author's claim/response,
//!   * change location (section/line/page),
//!   * `Verified?`.
//!
//! A matrix missing a required column → WARN `RR_MATRIX_INCOMPLETE`. No matrix
//! present → INFO (not applicable to this build).

use anyhow::Result;
use rusqlite::Connection;

use agentic_core::worktree;

use crate::{CheckReport, Finding, Severity};

/// Required column groups; each is a set of acceptable header substrings.
const REQUIRED: &[(&str, &[&str])] = &[
    (
        "reviewer point",
        &["reviewer", "comment", "point", "concern"],
    ),
    (
        "author's claim",
        &["claim", "response", "author", "action taken"],
    ),
    (
        "change location",
        &["location", "section", "line", "page", "where"],
    ),
    ("verified", &["verified"]),
];

/// R&R-context tokens — a `Verified?` column alone is not enough (many fact
/// tables carry one); the header must also name the review/response context.
const RR_CONTEXT: &[&str] = &[
    "reviewer",
    "review",
    "r&r",
    "resubmit",
    "rebuttal",
    "response to",
    "revision",
];

/// Is `header` (a lower-cased markdown table header row) an R&R matrix header?
/// Requires both a `verified` column and an R&R-context token, so ordinary
/// verified-facts tables are not mistaken for revision matrices.
#[must_use]
pub fn is_rr_header(header: &str) -> bool {
    header.contains("verified") && RR_CONTEXT.iter().any(|t| header.contains(t))
}

/// A markdown table separator row (`|---|:--:|…`) — only `|`, `-`, `:`, space.
/// Used to identify a real *header* row (the line before a separator), so data
/// rows that happen to contain "verified"/"review" are not mistaken for headers.
#[must_use]
pub fn is_separator_row(s: &str) -> bool {
    let t = s.trim();
    t.contains('-') && t.contains('|') && t.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
}

/// Missing required column groups for a lower-cased header row.
#[must_use]
pub fn missing_columns(header: &str) -> Vec<&'static str> {
    REQUIRED
        .iter()
        .filter(|(_, alts)| !alts.iter().any(|a| header.contains(a)))
        .map(|(label, _)| *label)
        .collect()
}

pub fn run(conn: &Connection, project: &str) -> Result<CheckReport> {
    let mut findings = Vec::new();
    let mut matrices = 0usize;
    for (path, sha) in worktree::list(conn, project, "")? {
        if !path.ends_with(".md") {
            continue;
        }
        let Ok(blob) = agentic_core::content::blob::get_blob(conn, &sha) else {
            continue;
        };
        let text = String::from_utf8_lossy(&blob.content);
        let lines: Vec<&str> = text.lines().collect();
        for (idx, ln) in lines.iter().enumerate() {
            // Only an actual table HEADER row (immediately followed by a `|---|`
            // separator) is a candidate — not a data row that merely contains
            // "verified"/"review" (e.g. a data-tier table listing a "Verified"
            // layer seen by "review agents").
            if ln.contains('|') && lines.get(idx + 1).is_some_and(|n| is_separator_row(n)) {
                let lower = ln.to_lowercase();
                if is_rr_header(&lower) {
                    matrices += 1;
                    let missing = missing_columns(&lower);
                    if !missing.is_empty() {
                        findings.push(Finding {
                            category: "RR_MATRIX_INCOMPLETE".into(),
                            severity: Severity::Warn,
                            message: format!(
                                "R&R matrix missing column(s): {}",
                                missing.join(", ")
                            ),
                            location: Some(format!("{path}:{}", idx + 1)),
                        });
                    }
                }
            }
        }
    }
    findings.push(Finding {
        category: "RR_MATRIX_SUMMARY".into(),
        severity: Severity::Info,
        message: if matrices == 0 {
            "no R&R traceability matrix found — not applicable to this build".into()
        } else {
            format!("{matrices} R&R matrix header(s) validated")
        },
        location: Some("rr_matrix".into()),
    });
    Ok(CheckReport::new("rr_matrix", findings))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_header_ok() {
        let h = "| reviewer comment | author response | section | verified? |";
        assert!(is_rr_header(h));
        assert!(missing_columns(h).is_empty());
    }

    #[test]
    fn missing_location_flagged() {
        let h = "| reviewer point | author claim | verified |";
        assert!(missing_columns(h).contains(&"change location"));
    }

    #[test]
    fn separator_row_detection() {
        assert!(is_separator_row("|---|:--:|---|"));
        assert!(is_separator_row("| --- | --- |"));
        assert!(!is_separator_row(
            "| Layer 2 -- Verified | x | review agents |"
        ));
        assert!(!is_separator_row("plain text - with a dash"));
    }

    #[test]
    fn data_row_not_mistaken_for_header() {
        // A data-tier table: its header has no `verified`/R&R token; a DATA row
        // contains "Verified" (a tier name) + "review" (a role). Only the header
        // is a candidate, so this must NOT be treated as an R&R matrix.
        let header = "| tier | examples | who may see |".to_lowercase();
        assert!(
            !is_rr_header(&header),
            "data-tier header is not an R&R header"
        );
        let data = "| layer 2 -- verified | artefacts | drafting + review agents |";
        // The gate would test `is_rr_header` only on a header row; the data row
        // matching is the bug we fixed. Confirm the data row, if mis-tested,
        // would have matched (documenting why header-only gating is required).
        assert!(is_rr_header(data));
    }
}
