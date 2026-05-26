//! `agentic check freshness` — domain-freshness gate (ADR-0047 R9).
//!
//! Moving-domain facts (PQC deadlines, regulations, prices, "last verified"
//! stamps) go stale. This advisory gate scans deliverable markdown for explicit
//! verification/recency markers (`last verified <Mon YYYY>`, `as of <YYYY>`,
//! `accessed <Mon YYYY>`, `retrieved …`, `updated …`) and flags any whose date
//! is older than `--max-age-months` (default 12) relative to the supplied
//! "now". WARN-level only (consistent with Cascade-advisory); the SDD-Cycle
//! promotion gate decides whether stale critical facts block promotion.
//! "Now" is passed in so this crate stays free of a clock dependency.

use std::sync::LazyLock;

use anyhow::Result;
use regex::Regex;
use rusqlite::Connection;

use agentic_core::worktree;

use crate::{CheckReport, Finding, Severity};

/// A recency marker: a freshness keyword, an optional 3-letter month, a year.
static MARKER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(last verified|verified|as of|accessed|retrieved|updated)\b[^.\n]{0,24}?(?:(jan|feb|mar|apr|may|jun|jul|aug|sep|oct|nov|dec)[a-z]*\.?\s+)?(20\d\d)\b",
    )
    .unwrap()
});

fn month_num(m: &str) -> u32 {
    match m.to_ascii_lowercase().as_str() {
        "jan" => 1,
        "feb" => 2,
        "mar" => 3,
        "apr" => 4,
        "may" => 5,
        "jun" => 6,
        "jul" => 7,
        "aug" => 8,
        "sep" => 9,
        "oct" => 10,
        "nov" => 11,
        "dec" => 12,
        _ => 1,
    }
}

/// Stale recency markers in `text`: `(line, snippet, age_in_months)` for any
/// marker older than `max_months` relative to (`now_year`, `now_month`).
/// Fence-aware.
#[must_use]
pub fn stale_markers(
    text: &str,
    now_year: u32,
    now_month: u32,
    max_months: u32,
) -> Vec<(usize, String, u32)> {
    let now = now_year * 12 + now_month;
    let mut out = Vec::new();
    let mut in_fence = false;
    for (idx, ln) in text.lines().enumerate() {
        if ln.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        for cap in MARKER.captures_iter(ln) {
            let Ok(year) = cap[3].parse::<u32>() else {
                continue;
            };
            let month = cap.get(2).map_or(1, |m| month_num(m.as_str()));
            let then = year * 12 + month;
            if now > then {
                let age = now - then;
                if age > max_months {
                    let snip: String = cap[0].split_whitespace().collect::<Vec<_>>().join(" ");
                    out.push((idx + 1, snip.chars().take(48).collect(), age));
                }
            }
        }
    }
    out
}

pub fn run(
    conn: &Connection,
    project: &str,
    now_year: u32,
    now_month: u32,
    max_months: u32,
) -> Result<CheckReport> {
    let mut findings = Vec::new();
    let mut total = 0usize;
    for (path, _sha) in worktree::list(conn, project, agentic_core::paths::SOURCES_PREFIX)? {
        if !path.ends_with(".md") {
            continue;
        }
        let blob = worktree::read_at(conn, project, &path)?;
        let text = String::from_utf8_lossy(&blob.content);
        for (line, snip, age) in stale_markers(&text, now_year, now_month, max_months) {
            total += 1;
            findings.push(Finding {
                category: "FRESHNESS_STALE".into(),
                severity: Severity::Warn,
                message: format!(
                    "recency marker '{snip}' is {age} month(s) old (> {max_months}) — re-verify"
                ),
                location: Some(format!("{path}:{line}")),
            });
        }
    }
    findings.push(Finding {
        category: "FRESHNESS_SUMMARY".into(),
        severity: Severity::Info,
        message: format!(
            "{total} stale recency marker(s) older than {max_months} months (as of {now_year}-{now_month:02})"
        ),
        location: Some("freshness".into()),
    });
    Ok(CheckReport::new("freshness", findings))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_old_marker_keeps_recent() {
        // "now" = 2026-05; max 12 months.
        let old = stale_markers("Research links last verified May 2024.\n", 2026, 5, 12);
        assert_eq!(old.len(), 1);
        assert_eq!(old[0].2, 24);
        let recent = stale_markers("Web links last verified Jan 2026.\n", 2026, 5, 12);
        assert!(recent.is_empty());
    }

    #[test]
    fn year_only_marker() {
        let v = stale_markers("Figures accessed 2023 from the registry.\n", 2026, 5, 12);
        assert_eq!(v.len(), 1);
    }

    #[test]
    fn fence_skipped() {
        let md = "```\nlast verified May 2020\n```\n";
        assert!(stale_markers(md, 2026, 5, 12).is_empty());
    }
}
