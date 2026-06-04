//! Rust port of the typo-detection subset of
//! `MT-Template/dist/_inconsistency_scan.py` (categories 1 + 7 — heading
//! typos + numbering gaps). The other categories in the Python file are
//! already covered by existing Rust gates:
//!
//!   * **2. Duplicate bib entries** → [`crate::bibliography_gate`]
//!   * **3. Stale `§X.Y.Z` cross-refs** → [`crate::toc_coverage_gate`]
//!   * **4. German residues in body** → [`crate::bookkit_gate`] (NON_ENGLISH rule)
//!   * **5. Acronym table coverage** → [`crate::undefined_terms`] + [`crate::acronym_xcheck`]
//!   * **6. Figure / Table caption issues** → [`crate::figure_quality_gate`] + [`crate::caption_location`]
//!   * **8. Subchapter label inconsistencies** → [`crate::term_rename_gate`]
//!
//! Wave-2 Agent C (Python→Rust migration, 2026-06-04). Curated typo
//! patterns + numbering-gap detector are exposed as library primitives the
//! existing gates can call; they're not registered as separate `agentic
//! check …` subcommands.

use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;

/// A `(typo_regex, suggested_correction)` pair — verbatim from the Python
/// `typo_patterns` list with the same correction strings.
pub struct TypoPattern {
    pub pattern: Regex,
    pub suggested: &'static str,
    pub raw: &'static str,
}

/// The 18 curated typos shipped with the original Python scanner. Each entry
/// keeps the raw source pattern string so callers can echo it in findings.
pub static TYPO_PATTERNS: LazyLock<Vec<TypoPattern>> = LazyLock::new(|| {
    [
        (r"\bLeaderhsip\b", "Leadership"),
        (r"\bArticial\b", "Artificial"),
        (r"\bprovenanve\b", "provenance"),
        (r"\bcontructs\b", "constructs"),
        (r"\bManagment\b", "Management"),
        (r"\bgovernace\b", "governance"),
        (r"\bcybersecuirty\b", "cybersecurity"),
        (r"\benviorn\b", "environ"),
        (r"\bquantam\b", "quantum"),
        (r"\boperatin\b", "operating"),
        (r"\binteligence\b", "intelligence"),
        (r"\binteligent\b", "intelligent"),
        (r"\bopen-souce\b", "open-source"),
        (r"\bsupervized\b", "supervised"),
        (r"\beffeciency\b", "efficiency"),
        (r"\boccured\b", "occurred"),
        (r"\bsuccessfull\b", "successful"),
        (r"\bautonomus\b", "autonomous"),
    ]
    .iter()
    .map(|(p, s)| TypoPattern {
        pattern: Regex::new(p).unwrap(),
        suggested: s,
        raw: p,
    })
    .collect()
});

/// One typo hit suitable for inclusion in a check report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TypoHit {
    pub line: usize,
    pub heading: String,
    pub pattern: String,
    pub suggested: String,
}

/// Scan a markdown body for heading lines (`# …` through `#### …`) and
/// report any typo matches. Mirrors the Python heuristic: only headings are
/// scanned (the broader body is the `writing_quality` gate's territory).
#[must_use]
pub fn heading_typos(text: &str) -> Vec<TypoHit> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with('#') {
            continue;
        }
        // Strip the leading `#`s + space to leave the heading text.
        let heading = trimmed.trim_start_matches('#').trim_start();
        for tp in TYPO_PATTERNS.iter() {
            if tp.pattern.is_match(heading) {
                out.push(TypoHit {
                    line: i,
                    heading: heading.to_string(),
                    pattern: tp.raw.to_string(),
                    suggested: tp.suggested.to_string(),
                });
            }
        }
    }
    out
}

/// Detect numbering gaps in subsection sequences (category 7 of the Python
/// script). Walks `Hn` headings, parses the leading `§X.Y.Z` numeric prefix,
/// and reports any sibling that skips a number under the same parent.
///
/// Example: under "5.1 Intro" we see "5.1.1", "5.1.3" → reports a gap before
/// "5.1.3" (missing "5.1.2").
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NumberingGap {
    /// Line of the heading where the gap was detected (the over-jumping one).
    pub line: usize,
    /// Parent prefix (e.g. "5.1").
    pub parent: String,
    /// Last sibling number seen under the parent before the jump.
    pub last_seen: u32,
    /// First sibling that jumped (the heading at `line`).
    pub jumped_to: u32,
}

/// Strict `^\d+(\.\d+)+\b` heading-number prefix (e.g. "5.1.2 Intro").
static NUM_PREFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(\d+(?:\.\d+)+)\b").unwrap());

/// Walk markdown lines, extract `X.Y.Z` prefixes from heading text, and
/// flag any gaps in the trailing component for headings that share a parent.
///
/// Only fires when the trailing component jumps forward by >1 — duplicates
/// and out-of-order siblings are deliberately ignored here (the Python
/// script treated them as separate categories handled elsewhere).
#[must_use]
pub fn numbering_gaps(text: &str) -> Vec<NumberingGap> {
    use std::collections::BTreeMap;
    // For each parent prefix, track the last trailing number seen.
    let mut last: BTreeMap<String, u32> = BTreeMap::new();
    let mut out = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let trimmed = raw.trim_start();
        if !trimmed.starts_with('#') {
            continue;
        }
        let heading = trimmed.trim_start_matches('#').trim_start();
        let Some(cap) = NUM_PREFIX.captures(heading) else {
            continue;
        };
        let prefix = cap.get(1).unwrap().as_str();
        let parts: Vec<&str> = prefix.split('.').collect();
        if parts.len() < 2 {
            continue;
        }
        let parent = parts[..parts.len() - 1].join(".");
        let Ok(tail) = parts.last().unwrap().parse::<u32>() else {
            continue;
        };
        if let Some(prev) = last.get(&parent).copied() {
            if tail > prev + 1 {
                out.push(NumberingGap {
                    line: i,
                    parent: parent.clone(),
                    last_seen: prev,
                    jumped_to: tail,
                });
            }
            // Only advance when the new tail is greater — don't regress on
            // duplicates / out-of-order siblings.
            if tail > prev {
                last.insert(parent, tail);
            }
        } else {
            last.insert(parent, tail);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typo_patterns_loaded_in_full() {
        assert_eq!(TYPO_PATTERNS.len(), 18);
    }

    #[test]
    fn heading_typos_flag_curated_words_only_in_headings() {
        let md = "# Agile Leaderhsip\n\nbody Leaderhsip stays unflagged here.\n\
## quantam computing\n\n# All good\n";
        let hits = heading_typos(md);
        assert_eq!(hits.len(), 2);
        assert!(hits.iter().any(|h| h.suggested == "Leadership"));
        assert!(hits.iter().any(|h| h.suggested == "quantum"));
    }

    #[test]
    fn heading_typos_ignore_body_text() {
        let md = "Just a paragraph with Leaderhsip in it.\n";
        let hits = heading_typos(md);
        assert!(hits.is_empty());
    }

    #[test]
    fn numbering_gaps_detects_skipped_subsection() {
        let md = "# 5 Solution\n## 5.1 Intro\n### 5.1.1 First\n### 5.1.3 Third\n";
        let gaps = numbering_gaps(md);
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].parent, "5.1");
        assert_eq!(gaps[0].last_seen, 1);
        assert_eq!(gaps[0].jumped_to, 3);
    }

    #[test]
    fn numbering_gaps_quiet_for_complete_sequence() {
        let md = "## 5.1\n### 5.1.1\n### 5.1.2\n### 5.1.3\n";
        let gaps = numbering_gaps(md);
        assert!(gaps.is_empty());
    }

    #[test]
    fn numbering_gaps_isolates_per_parent() {
        // 5.1.1 → 5.1.2 (no gap)   5.2.1 → 5.2.3 (gap)
        let md = "### 5.1.1\n### 5.1.2\n### 5.2.1\n### 5.2.3\n";
        let gaps = numbering_gaps(md);
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].parent, "5.2");
    }

    #[test]
    fn typo_hit_line_is_zero_indexed() {
        let md = "intro\n# governace plan\n";
        let hits = heading_typos(md);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].line, 1);
    }
}
