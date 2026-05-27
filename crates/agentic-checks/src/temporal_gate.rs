//! `agentic check temporal` — five-pass temporal-integrity gate (ARS parity).
//!
//! Scans deliverable markdown (fence-aware) and `literature_corpus` years.
//! The five passes (ADR-0044, mirroring the ARS 5-pass verifier):
//!   1. **future-year** — a standalone `20\d\d` token `> --max-year` (default
//!      2026), almost always a typo or forward-dated source → WARN
//!      `TEMPORAL_FUTURE`.
//!   2. **retrospective-arithmetic** — `from YYYY to YYYY … N year(s)` where the
//!      stated span ≠ the year difference → WARN `TEMPORAL_ARITHMETIC`.
//!   3. **anachronistic-citation** — a citation year that post-dates the
//!      sentence's own asserted date (covered by future-year for `> max_year`;
//!      retained as part of pass 1).
//!   4. **comparator-unmaterialized** — an "N× faster"/"outperforms"/"SOTA"
//!      comparative claim with no citation anchor on the line → WARN
//!      `TEMPORAL_COMPARATOR`.
//!   5. **causal-inversion / deictic-present** — strong causal phrasing
//!      (`TEMPORAL_CAUSAL`) and deictic time words (`currently`, `today`, …)
//!      that silently date the text (`TEMPORAL_DEICTIC`) → advisory INFO.
//! URL/DOI and fenced lines are skipped so identifiers like `10.48550/...2099`
//! do not trip the heuristics.

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
/// A forward-looking / forecast frame on the line. A future year inside such a
/// frame is an intentional roadmap horizon (PQC migration, CRA/NIS2 deadlines,
/// market projections), not the typo `future_years` is meant to catch. Without
/// such a cue a bare future date ("the survey ran in 2031") is still flagged.
static FORECAST: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(→|->|\bby\b|\bthrough\b|\buntil\b|\btill\b|\bexpected\b|\banticipat\w*|\bproject(?:ed|ion)\w*|\bforecast\w*|\btarget\w*|\bdeadline\b|\bno later than\b|\benforceable\b|\bphas(?:e|es|ed|ing)\b|\bmigrat\w*|\btransition\w*|\bdeprecat\w*|\bsunset\w*|\broadmap\b|\bhorizon\b|\bplanned\b|\bscheduled\b|\bby the (?:late|early|mid)\b|\bend of\b|\bbeyond\b|\breaching\b|\bin year\b|\bcagr\b|\bwill\b|\bforthcoming\b|\bupcoming\b)").unwrap()
});
/// A URL/DOI on the line → skip (identifiers carry digit runs that aren't years).
static URLDOI: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)https?://|doi\.org|10\.\d{4,}").unwrap());
/// `from YYYY to YYYY` with an asserted span `N year(s)` somewhere on the line.
static SPAN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)from\s+(20\d\d)\s+to\s+(20\d\d)").unwrap());
static SPAN_YEARS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(\d{1,3})[\s-]*year").unwrap());
/// A comparative-performance claim that should cite a baseline.
static COMPARATOR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(\b\d+(?:\.\d+)?\s*[x×]\s*(?:faster|slower|higher|lower|better|more|less)|\boutperform(?:s|ed|ing)?\b|\bstate[- ]of[- ]the[- ]art\b|\bbest[- ]in[- ]class\b)").unwrap()
});
/// A citation/anchor on the line → the comparator is materialised.
static CITE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)et al\.|\(\d{4}|\[\d+\]|https?://|doi|table\s+\d|figure\s+\d|fig\.\s*\d")
        .unwrap()
});
/// Strong causal phrasing whose direction is worth re-checking.
static CAUSAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(causes|caused by|leads directly to|is the reason for|results directly in|because of which)\b").unwrap()
});
/// Deictic present-tense time words that silently date the text.
static DEICTIC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(currently|at present|as of (?:now|today)|nowadays|these days|right now|at the moment)\b").unwrap()
});

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
        // Skip URL/DOI lines, markdown table rows (horizon/timeline tables are
        // structured roadmap data), and forecast-framed lines (intentional
        // forward references).
        let lt = ln.trim_start();
        if in_fence || URLDOI.is_match(ln) || lt.starts_with('|') || FORECAST.is_match(ln) {
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

/// Passes 2–5 over `text`. Returns `(line, category, severity, message)`.
/// Fence- and URL/DOI-aware; advisory by design (causal/deictic are INFO).
#[must_use]
pub fn extra_passes(text: &str) -> Vec<(usize, &'static str, Severity, String)> {
    let mut out = Vec::new();
    let mut in_fence = false;
    for (idx, ln) in text.lines().enumerate() {
        let i = idx + 1;
        if ln.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        // Pass 2 — retrospective arithmetic: stated span vs. year difference.
        if let Some(c) = SPAN.captures(ln) {
            if let (Ok(a), Ok(b)) = (c[1].parse::<i64>(), c[2].parse::<i64>()) {
                let diff = (b - a).abs();
                if let Some(s) = SPAN_YEARS.captures(ln) {
                    if let Ok(stated) = s[1].parse::<i64>() {
                        if stated != diff {
                            out.push((
                                i,
                                "TEMPORAL_ARITHMETIC",
                                Severity::Warn,
                                format!(
                                    "stated span {stated} year(s) ≠ {diff} ({a}→{b}) — recompute"
                                ),
                            ));
                        }
                    }
                }
            }
        }
        // Pass 4 — comparator without a materialised baseline.
        if COMPARATOR.is_match(ln) && !CITE.is_match(ln) {
            let m = COMPARATOR.find(ln).map_or("", |x| x.as_str());
            out.push((
                i,
                "TEMPORAL_COMPARATOR",
                Severity::Warn,
                format!("comparative claim '{m}' has no baseline/citation on the line"),
            ));
        }
        // Pass 5a — causal direction.
        if CAUSAL.is_match(ln) {
            out.push((
                i,
                "TEMPORAL_CAUSAL",
                Severity::Info,
                "strong causal phrasing — verify the direction is not inverted".into(),
            ));
        }
        // Pass 5b — deictic present.
        if let Some(m) = DEICTIC.find(ln) {
            out.push((
                i,
                "TEMPORAL_DEICTIC",
                Severity::Info,
                format!(
                    "deictic time word '{}' dates the text — anchor with an explicit date",
                    m.as_str()
                ),
            ));
        }
    }
    out
}

pub fn run(conn: &Connection, project: &str, max_year: u32) -> Result<CheckReport> {
    let mut findings = Vec::new();
    let mut total = 0usize;

    // 1. Deliverable markdown under out/sources/.
    for (path, _sha) in worktree::list(conn, project, agentic_core::paths::SOURCES_PREFIX)? {
        if !is_markdown(&path) {
            continue;
        }
        // The merged dimensions doc is a deterministic concatenation of the
        // dimension sources (scanned individually); skip it so future-year and
        // comparator findings are not double-counted.
        if path == agentic_core::paths::MERGED_DOC {
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
        for (line, category, severity, message) in extra_passes(&text) {
            if matches!(severity, Severity::Warn) {
                total += 1;
            }
            findings.push(Finding {
                category: category.into(),
                severity,
                message,
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
    fn forecast_framed_future_years_are_intentional() {
        // Forward-looking roadmap horizons are not typos.
        assert!(future_years("the market reaches USD 1.45 B by 2030.\n", 2026).is_empty());
        assert!(future_years("CRA becomes enforceable in 2027.\n", 2026).is_empty());
        assert!(future_years("ML-KEM-512 is deprecated after 2030.\n", 2026).is_empty());
        // Horizon table rows carry roadmap years as structured data.
        assert!(future_years("| 10-yr (→2036) | ~USD 1.5 B | Increase |\n", 2026).is_empty());
        // A bare future date with no forecast frame is still flagged (typo guard).
        assert_eq!(
            future_years("The user survey was conducted in 2031.\n", 2026).len(),
            1
        );
    }

    #[test]
    fn retrospective_arithmetic_mismatch() {
        let v = extra_passes("Growth from 2019 to 2025, a span of 4 years, was steep.\n");
        assert!(v.iter().any(|(_, c, _, _)| *c == "TEMPORAL_ARITHMETIC"));
        // Correct span produces no arithmetic finding.
        let ok = extra_passes("From 2019 to 2025, over 6 years, it grew.\n");
        assert!(!ok.iter().any(|(_, c, _, _)| *c == "TEMPORAL_ARITHMETIC"));
    }

    #[test]
    fn comparator_needs_baseline() {
        let bare = extra_passes("Our method is 10x faster.\n");
        assert!(bare.iter().any(|(_, c, _, _)| *c == "TEMPORAL_COMPARATOR"));
        // With a citation on the line it is materialised.
        let cited = extra_passes("Our method is 10x faster (Kim et al., 2025).\n");
        assert!(!cited.iter().any(|(_, c, _, _)| *c == "TEMPORAL_COMPARATOR"));
    }

    #[test]
    fn deictic_and_causal_are_advisory() {
        let v = extra_passes("Currently the gap is large because of which costs rise.\n");
        assert!(
            v.iter()
                .any(|(_, c, s, _)| *c == "TEMPORAL_DEICTIC" && matches!(s, Severity::Info))
        );
        assert!(
            v.iter()
                .any(|(_, c, s, _)| *c == "TEMPORAL_CAUSAL" && matches!(s, Severity::Info))
        );
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
