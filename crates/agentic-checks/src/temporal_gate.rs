//! `agentic check temporal` — future-year (temporal-contamination) gate.
//!
//! Scans deliverable markdown (fence-aware) and `literature_corpus` years for
//! any 4-digit year strictly greater than `--max-year` (default 2026). A future
//! year is almost always a typo or a hallucinated/forward-dated source, so it is
//! surfaced as a WARN `TEMPORAL_FUTURE` with the offending year. Conservative:
//! only standalone `20\d\d` tokens are matched, and URL/DOI lines are skipped so
//! identifiers like `10.48550/...2099` do not trip the heuristic.

use std::sync::LazyLock;

use anyhow::Result;
use regex::Regex;
use rusqlite::Connection;
use serde_json::Value;

use agentic_core::passport::{self, Section};
use agentic_core::worktree;

use crate::{CheckReport, Finding, Severity};

/// A standalone 21st-century year token (word-bounded `20\d\d`).
static YEAR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(20\d\d)\b").unwrap());
/// A URL/DOI on the line → skip (identifiers carry digit runs that aren't years).
static URLDOI: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)https?://|doi\.org|10\.\d{4,}").unwrap());

/// Case-insensitive `.md` extension test (avoids a locale-sensitive compare).
fn is_markdown(path: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("md"))
}

/// Future years (> `max_year`) found in `text`, fence- and URL/DOI-aware.
/// Returns `(line_number, year)` pairs.
#[must_use]
pub fn future_years(text: &str, max_year: u32) -> Vec<(usize, u32)> {
    let mut out = Vec::new();
    let mut in_fence = false;
    for (idx, ln) in text.lines().enumerate() {
        let i = idx + 1;
        if ln.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence || URLDOI.is_match(ln) {
            continue;
        }
        for cap in YEAR.captures_iter(ln) {
            if let Ok(y) = cap[1].parse::<u32>() {
                if y > max_year {
                    out.push((i, y));
                }
            }
        }
    }
    out
}

pub fn run(conn: &Connection, project: &str, max_year: u32) -> Result<CheckReport> {
    let mut findings = Vec::new();
    let mut total = 0usize;

    // 1. Deliverable markdown under out/sources/.
    for (path, _sha) in worktree::list(conn, project, "out/sources/")? {
        if !is_markdown(&path) {
            continue;
        }
        let blob = worktree::read_at(conn, project, &path)?;
        let text = String::from_utf8_lossy(&blob.content);
        for (line, year) in future_years(&text, max_year) {
            total += 1;
            findings.push(Finding {
                category: "TEMPORAL_FUTURE".into(),
                severity: Severity::Warn,
                message: format!("future year {year} (> {max_year}) — verify or correct"),
                location: Some(format!("{path}:{line}")),
            });
        }
    }

    // 2. literature_corpus declared years.
    for e in passport::current(conn, project, Section::LiteratureCorpus)? {
        let Ok(v) = serde_json::from_str::<Value>(&e.payload_json) else {
            continue;
        };
        if let Some(y) = v.get("year").and_then(Value::as_u64) {
            if y > u64::from(max_year) {
                total += 1;
                let key = v.get("citation_key").and_then(Value::as_str).unwrap_or("?");
                findings.push(Finding {
                    category: "TEMPORAL_FUTURE".into(),
                    severity: Severity::Warn,
                    message: format!("corpus '{key}' declares future year {y} (> {max_year})"),
                    location: Some("literature_corpus".into()),
                });
            }
        }
    }

    findings.push(Finding {
        category: "TEMPORAL_SUMMARY".into(),
        severity: Severity::Info,
        message: format!("{total} future-year token(s) found (max-year {max_year})"),
        location: Some("temporal".into()),
    });

    Ok(CheckReport::new("temporal", findings))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_future_year() {
        let v = future_years("Released in 2099 according to plan.\n", 2026);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].1, 2099);
    }

    #[test]
    fn current_year_ok() {
        assert_eq!(future_years("Published in 2024.\n", 2026).len(), 0);
    }

    #[test]
    fn url_and_fence_skipped() {
        // DOI/URL line skipped.
        assert_eq!(
            future_years("see https://x.org/2099/paper for details\n", 2026).len(),
            0
        );
        // Fenced line skipped.
        let fenced = "```\nyear = 2099\n```\n";
        assert_eq!(future_years(fenced, 2026).len(), 0);
    }
}
