//! `agentic check integrity` — the ARS 7-mode integrity gate (ADR-0044).
//!
//! Deterministic heuristics over deliverable markdown that surface the seven
//! integrity-failure modes the ARS pipeline blocks on at its Stage 2.5/4.5
//! gates. All are fence-aware and advisory by default (WARN), except the two
//! "confirm this happened" modes which are INFO:
//!
//!   1. `INTEGRITY_HALLUCINATED_RESULT` — a results assertion with a number but
//!      no citation/anchor on the line.
//!   2. `INTEGRITY_METHOD_FABRICATION` (INFO) — an empirical-work claim
//!      ("we trained/ran/evaluated") to confirm the work was actually performed.
//!   3. `INTEGRITY_SHORTCUT` — a left-in TODO/FIXME/placeholder/stub.
//!   4. `INTEGRITY_IMPL_BUG` — a "broken / does not work / known bug" admission.
//!   5. `INTEGRITY_FRAME_LOCK` — a non-trivial sentence repeated ≥3× verbatim.
//!   6. `INTEGRITY_UNVERIFIED_RESULT` — a quantitative claim sharing a line with
//!      a `NEEDS-VERIFICATION` marker.
//!   7. `INTEGRITY_OVERCLAIM` (INFO) — absolute proof language ("proves",
//!      "guarantees", "first ever", "definitively").

use std::collections::HashMap;
use std::sync::LazyLock;

use anyhow::Result;
use regex::Regex;
use rusqlite::Connection;

use agentic_core::worktree;

use crate::{CheckReport, Finding, Severity};

static RESULT_ASSERT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(results show|we (?:achieve|achieved|obtain|obtained|report|measured)|accuracy of|precision of|recall of|f1 of|\b\d+(?:\.\d+)?\s*%\s*(?:improvement|increase|reduction|gain))").unwrap()
});
static HAS_NUM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d").unwrap());
static CITE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)et al\.|\(\d{4}|\[\d+\]|https?://|doi|table\s+\d|figure\s+\d|appendix")
        .unwrap()
});
static METHOD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bwe (?:trained|ran|conducted|evaluated|implemented and tested|deployed|benchmarked|surveyed)\b|our (?:experiment|user study|benchmark)\b").unwrap()
});
static SHORTCUT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(todo|fixme|xxx|placeholder|lorem ipsum|tbd|stub)\b").unwrap()
});
static IMPL_BUG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(known bug|does not work|doesn't work|is broken|currently broken|fails to (?:build|compile|run))\b").unwrap()
});
static OVERCLAIM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(proves|proven|guarantees|definitively|first ever|the first to|without any doubt|undeniably)\b").unwrap()
});
static NEEDS_VERIFY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)NEEDS-VERIFICATION").unwrap());
/// A sentence worth tracking for frame-lock: ≥6 words.
static URLDOI: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)https?://|doi\.org|10\.\d{4,}").unwrap());

/// Per-document integrity findings (modes 1–4, 6, 7). `path` for location.
#[must_use]
pub fn line_findings(text: &str, path: &str) -> Vec<Finding> {
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
        let push = |out: &mut Vec<Finding>, cat: &str, sev: Severity, msg: String| {
            out.push(Finding {
                category: cat.into(),
                severity: sev,
                message: msg,
                location: Some(format!("{path}:{i}")),
            });
        };
        if RESULT_ASSERT.is_match(ln) && HAS_NUM.is_match(ln) && !CITE.is_match(ln) {
            push(
                &mut out,
                "INTEGRITY_HALLUCINATED_RESULT",
                Severity::Warn,
                "numeric result assertion with no citation/anchor on the line — verify or cite"
                    .into(),
            );
        }
        if METHOD.is_match(ln) {
            push(&mut out, "INTEGRITY_METHOD_FABRICATION", Severity::Info,
                "empirical-work claim — confirm the work was actually performed and is reproducible".into());
        }
        if SHORTCUT.is_match(ln) {
            push(
                &mut out,
                "INTEGRITY_SHORTCUT",
                Severity::Warn,
                "left-in shortcut marker (TODO/FIXME/placeholder/stub) in a deliverable".into(),
            );
        }
        if IMPL_BUG.is_match(ln) {
            push(
                &mut out,
                "INTEGRITY_IMPL_BUG",
                Severity::Warn,
                "implementation-defect admission in a deliverable — resolve or qualify".into(),
            );
        }
        if NEEDS_VERIFY.is_match(ln) && HAS_NUM.is_match(ln) && !URLDOI.is_match(ln) {
            push(
                &mut out,
                "INTEGRITY_UNVERIFIED_RESULT",
                Severity::Warn,
                "quantitative claim still tagged NEEDS-VERIFICATION".into(),
            );
        }
        if OVERCLAIM.is_match(ln) {
            push(
                &mut out,
                "INTEGRITY_OVERCLAIM",
                Severity::Info,
                "absolute proof language — hedge unless formally established".into(),
            );
        }
    }
    out
}

/// Mode 5 — frame-lock: a non-trivial *prose* sentence repeated ≥3× verbatim in
/// `text`. Markdown table rows (a segment starting with `|`, e.g. a `| --- |`
/// separator) are structural, not prose, and are skipped; a segment must carry
/// ≥6 *alphabetic* words (so pipe/dash runs are not mistaken for a sentence).
#[must_use]
pub fn frame_lock_repeats(text: &str) -> Vec<(String, usize)> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut in_fence = false;
    for ln in text.lines() {
        if ln.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        // Skip fenced blocks (figspec/code) and JSON-ish lines (figure captions
        // such as `{"caption":"…"}` are data, not repeated prose).
        let lt = ln.trim_start();
        if in_fence || lt.starts_with('{') || lt.starts_with('}') || lt.starts_with('"') {
            continue;
        }
        for seg in ln.split(|c| c == '.' || c == '!' || c == '?') {
            let s = seg.trim();
            // Skip markdown table rows (separators + data rows) — structural.
            if s.starts_with('|') {
                continue;
            }
            let prose_words = s
                .split_whitespace()
                .filter(|w| w.chars().any(char::is_alphabetic))
                .count();
            if prose_words >= 6 {
                *counts.entry(s.to_lowercase()).or_default() += 1;
            }
        }
    }
    let mut v: Vec<(String, usize)> = counts.into_iter().filter(|(_, n)| *n >= 3).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1));
    v
}

pub fn run(conn: &Connection, project: &str) -> Result<CheckReport> {
    let mut findings = Vec::new();
    let mut files = 0usize;
    for (path, sha) in worktree::list(conn, project, agentic_core::paths::SOURCES_PREFIX)? {
        if !path.ends_with(".md") {
            continue;
        }
        files += 1;
        let Ok(blob) = agentic_core::content::blob::get_blob(conn, &sha) else {
            continue;
        };
        let text = String::from_utf8_lossy(&blob.content);
        findings.extend(line_findings(&text, &path));
        for (sentence, n) in frame_lock_repeats(&text) {
            findings.push(Finding {
                category: "INTEGRITY_FRAME_LOCK".into(),
                severity: Severity::Warn,
                message: format!(
                    "sentence repeated {n}× verbatim: \"{}…\"",
                    sentence.chars().take(50).collect::<String>()
                ),
                location: Some(path.clone()),
            });
        }
    }
    findings.push(Finding {
        category: "INTEGRITY_SUMMARY".into(),
        severity: Severity::Info,
        message: format!("7-mode integrity scan over {files} deliverable file(s)"),
        location: Some("integrity".into()),
    });
    Ok(CheckReport::new("integrity", findings))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hallucinated_result_needs_citation() {
        let bare = line_findings("Results show accuracy of 97.3% on the test set.", "c.md");
        assert!(
            bare.iter()
                .any(|f| f.category == "INTEGRITY_HALLUCINATED_RESULT")
        );
        let cited = line_findings("Results show accuracy of 97.3% (Kim et al., 2025).", "c.md");
        assert!(
            !cited
                .iter()
                .any(|f| f.category == "INTEGRITY_HALLUCINATED_RESULT")
        );
    }

    #[test]
    fn shortcut_and_bug_flagged() {
        let f = line_findings("TODO: rewrite this. The parser is broken.", "c.md");
        assert!(f.iter().any(|x| x.category == "INTEGRITY_SHORTCUT"));
        assert!(f.iter().any(|x| x.category == "INTEGRITY_IMPL_BUG"));
    }

    #[test]
    fn frame_lock_counts_repeats() {
        let t = "the quick brown fox jumps high. ".repeat(3);
        assert!(!frame_lock_repeats(&t).is_empty());
    }

    #[test]
    fn table_separators_not_frame_locked() {
        // Repeated markdown table separators / rows are structural, not prose —
        // must NOT be flagged (the cascade false positive).
        let md = "| a | b | c |\n| --- | --- | --- |\n| 1 | 2 | 3 |\n\n\
                  | x | y | z |\n| --- | --- | --- |\n| 4 | 5 | 6 |\n\n\
                  | p | q | r |\n| --- | --- | --- |\n| 7 | 8 | 9 |\n";
        assert!(
            frame_lock_repeats(md).is_empty(),
            "table separators/rows must not be frame-locked"
        );
        // Genuine repeated prose is still caught.
        let prose = "the quick brown fox jumps over the lazy dog. ".repeat(3);
        assert!(!frame_lock_repeats(&prose).is_empty());
    }
}
