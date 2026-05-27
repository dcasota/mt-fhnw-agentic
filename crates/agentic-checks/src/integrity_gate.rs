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
/// Strong shortcut markers — imperative scaffolding that is essentially always
/// a left-in artifact, regardless of context.
static SHORTCUT_STRONG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(todo|fixme|xxx|lorem ipsum|tbd)\b").unwrap());
/// Soft shortcut words — also legitimate engineering nouns ("parallelisation
/// stub", "test stub", a config "placeholder value"). Only the *predicative
/// scaffolding* sense ("is a stub", "just a placeholder", "placeholder text")
/// is a genuine left-in marker; compound-noun and negated/quoted uses are not.
static SHORTCUT_SOFT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(placeholder|stub)\b").unwrap());
/// A negation just before a soft marker ("not a stub", "no placeholder").
static NEG_SHORTCUT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(not|no|never|neither|isn't|aren't|wasn't|doesn't|don't|cannot)\b[\s\w,'-]{0,20}$",
    )
    .unwrap()
});
/// A predicative article just before a soft marker — the scaffolding sense
/// ("is a stub", "just a placeholder", "the placeholder").
static ARTICLE_BEFORE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(a|an|the|this|that|just|mere|merely|only|is|was|be|been|remains?)\s+$")
        .unwrap()
});
static IMPL_BUG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(known bug|does not work|doesn't work|is broken|currently broken|fails to (?:build|compile|run))\b").unwrap()
});
static OVERCLAIM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(proves|proven|guarantees|definitively|first ever|the first to|without any doubt|undeniably)\b").unwrap()
});
/// A negation ending the text *before* a proof word ("cannot (be formally)
/// proven", "not yet proven", "never proven", "hard to prove") — a HEDGE, the
/// opposite of an overclaim, so it must not be flagged.
static NEG_BEFORE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(cannot|can ?not|can't|could not|couldn't|not|never|no|without|unable|impossible|difficult|hard|yet to|cease to|fail(?:s|ed)? to)\b[\w\s,'-]{0,24}$").unwrap()
});
/// A qualifier ending the text before `guarantees`/`proven` that makes it a
/// NOUN/adjective ("crypto guarantees", "security guarantees", "a proven …"),
/// not an absolute claim.
static NOUN_QUALIFIER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(crypto|cryptographic|security|safety|integrity|formal|strong|the|these|those|such|its|their|our|a|an|provide[sd]?|offers?|deliver[sd]?)\s+$").unwrap()
});

/// Is there a *genuine* (author-voice, non-negated, non-quoted, non-noun)
/// overclaim on the line? Filters the precision false positives surfaced in the
/// cascade triage: hedged "cannot be proven", quoted/glossed spans, and
/// `guarantees`/`proven` used as a noun/adjective.
#[must_use]
pub fn has_genuine_overclaim(ln: &str) -> bool {
    for m in OVERCLAIM.find_iter(ln) {
        let pre = &ln[..m.start()];
        // Inside a quote / italic gloss → not the author's own claim.
        if pre.matches('"').count() % 2 == 1
            || pre.matches('\u{201c}').count() > pre.matches('\u{201d}').count()
            || pre.matches('*').count() % 2 == 1
        {
            continue;
        }
        if NEG_BEFORE.is_match(pre) {
            continue; // a hedge ("cannot be proven"), not an overclaim
        }
        let w = m.as_str().to_lowercase();
        if (w == "guarantees" || w == "proven") && NOUN_QUALIFIER.is_match(pre) {
            continue; // noun/adjective use ("crypto guarantees", "a proven …")
        }
        return true;
    }
    false
}

/// Is `pre` (the text before a match) inside an open quotation or italic gloss?
fn in_quote_or_gloss(pre: &str) -> bool {
    pre.matches('"').count() % 2 == 1
        || pre.matches('\u{201c}').count() > pre.matches('\u{201d}').count()
        || pre.matches('*').count() % 2 == 1
}

/// Is there a *genuine* left-in shortcut marker on the line? Strong markers
/// (TODO/FIXME/XXX/TBD/lorem ipsum) always count unless quoted. Soft words
/// (placeholder/stub) count only in the predicative scaffolding sense — not as
/// compound nouns ("parallelisation stub"), nor when negated ("not a stub") or
/// quoted (the dismissed "weak placeholder").
#[must_use]
pub fn has_genuine_shortcut(ln: &str) -> bool {
    for m in SHORTCUT_STRONG.find_iter(ln) {
        if !in_quote_or_gloss(&ln[..m.start()]) {
            return true;
        }
    }
    for m in SHORTCUT_SOFT.find_iter(ln) {
        let pre = &ln[..m.start()];
        if in_quote_or_gloss(pre) || NEG_SHORTCUT.is_match(pre) {
            continue;
        }
        let post = ln[m.end()..].trim_start().to_lowercase();
        if ARTICLE_BEFORE.is_match(pre) || post.starts_with("text") {
            return true; // "is a stub" / "just a placeholder" / "placeholder text"
        }
    }
    false
}

/// Is there a *genuine* implementation-defect admission on the line? Filters the
/// benign uses surfaced in triage: quoted ("RSA is broken" headline being
/// dismissed), "broken into" (decomposed), and "broken by X" (a tamper-evidence
/// design property, e.g. a hash chain broken by any edit).
#[must_use]
pub fn has_genuine_impl_bug(ln: &str) -> bool {
    for m in IMPL_BUG.find_iter(ln) {
        let pre = &ln[..m.start()];
        if in_quote_or_gloss(pre) {
            continue;
        }
        if m.as_str().to_lowercase().contains("broken") {
            let post = ln[m.end()..].trim_start().to_lowercase();
            if post.starts_with("into") || post.starts_with("by ") || post.starts_with("by,") {
                continue;
            }
        }
        return true;
    }
    false
}
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
        if has_genuine_shortcut(ln) {
            push(
                &mut out,
                "INTEGRITY_SHORTCUT",
                Severity::Warn,
                "left-in shortcut marker (TODO/FIXME/placeholder/stub) in a deliverable".into(),
            );
        }
        if has_genuine_impl_bug(ln) {
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
        if has_genuine_overclaim(ln) {
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
            // Skip structural scaffolding, not narrative prose: markdown table
            // rows (start `|`), headings (start `#`), and section lead-in
            // labels (end `:`). Template-driven docs (campaign sheets, dimension
            // chapters) legitimately repeat these across parallel sections.
            if s.starts_with('|') || s.starts_with('#') || s.ends_with(':') {
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
        // The merged dimensions doc is a deterministic concatenation of the 11
        // dimension sources (scanned individually below); auditing it too would
        // double-count every finding and stack per-chapter boilerplate to ~11×
        // verbatim (spurious FRAME_LOCK). Skip the derived artifact.
        if path == agentic_core::paths::MERGED_DOC {
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
    fn shortcut_soft_word_noun_uses_not_flagged() {
        // "stub"/"placeholder" as engineering nouns or negated/quoted — benign.
        for ln in [
            "the parallelisation stub feeds the trace harness", // compound noun
            "validated against a PT-C02-2 stub; the cap is checked", // named test stub
            "this is not a placeholder: the solver is evidence-backed", // negated
            "read \"default solver\" as \"weak placeholder we tolerate\"", // quoted
            "M2 — Detection over stub",                         // prepositional noun
        ] {
            assert!(
                !has_genuine_shortcut(ln),
                "benign soft-marker use should not flag: {ln}"
            );
        }
        // Genuine scaffolding senses still flag.
        assert!(has_genuine_shortcut("this section is just a placeholder"));
        assert!(has_genuine_shortcut(
            "Placeholder text to be filled in later"
        ));
        assert!(has_genuine_shortcut("FIXME: revisit the threshold"));
    }

    #[test]
    fn impl_bug_benign_broken_senses_not_flagged() {
        for ln in [
            "A monolithic workflow is broken into Work Units", // decomposed
            "the hash chain is broken by any edit and re-checked", // tamper-evidence
            "dismiss the sensational \"RSA is broken\" headline", // quoted/dismissed
        ] {
            assert!(
                !has_genuine_impl_bug(ln),
                "benign 'broken' sense should not flag: {ln}"
            );
        }
        // Genuine defect admissions still flag.
        assert!(has_genuine_impl_bug("the build is currently broken"));
        assert!(has_genuine_impl_bug("this feature does not work yet"));
        assert!(has_genuine_impl_bug("the binary fails to compile on arm"));
    }

    #[test]
    fn frame_lock_counts_repeats() {
        let t = "the quick brown fox jumps high. ".repeat(3);
        assert!(!frame_lock_repeats(&t).is_empty());
    }

    #[test]
    fn frame_lock_skips_structural_labels() {
        // Section lead-in labels and headings repeat by design across the
        // parallel sections of a template-driven document — not frame-lock.
        let labels = "specific to this campaign (not generic):\n".repeat(4);
        assert!(frame_lock_repeats(&labels).is_empty());
        let headings = "## the assessment establishes campaign value here\n".repeat(4);
        assert!(frame_lock_repeats(&headings).is_empty());
    }

    #[test]
    fn overclaim_precision() {
        use super::has_genuine_overclaim;
        // Hedge ("cannot be ... proven") is NOT an overclaim.
        assert!(!has_genuine_overclaim(
            "ML security cannot be formally proven (CAR-04-001)."
        ));
        assert!(!has_genuine_overclaim("this has never been proven."));
        // Noun use of guarantees/proven is not an absolute claim.
        assert!(!has_genuine_overclaim(
            "crypto guarantees from C3/C5/C6 supply governance."
        ));
        assert!(!has_genuine_overclaim("a proven approach to backporting."));
        // Quoted/italic spans are not the author's own claim.
        assert!(!has_genuine_overclaim(
            "the report calls it \"the first to ship\"."
        ));
        // A genuine author-voice overclaim IS still flagged.
        assert!(has_genuine_overclaim("the dashboard proves the value."));
        assert!(has_genuine_overclaim("this guarantees zero downtime."));
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
