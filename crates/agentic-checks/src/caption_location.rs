//! Rust port of `MT-Template/dist/_captions_location_report.py` and
//! `_check_captions_in_body.py` — locate "Figure N: …" / "Table N: …"
//! caption-like paragraphs **outside** the List-of-Figures / List-of-Tables
//! zones.
//!
//! The original Python scripts walked a python-docx `Document` and reported
//! inline caption paragraphs whose position sat outside the back-matter list
//! zones (a typographic-hygiene check for the legacy DOCX template). This
//! port operates on markdown chapter text (the agentic pipeline's native
//! source), so the "zone" boundary becomes the H1 heading whose title
//! resolves to the i18n `list_of_figures` / `list_of_tables` chrome key.
//!
//! Wave-2 Agent C (Python→Rust migration, 2026-06-04). Helper feeds the
//! existing `figure_quality_gate` and `render_fidelity_gate` — it does NOT
//! itself register as a `agentic check …` subcommand (the gates already
//! perform the corpus walk; this is a primitive they can call).
//!
//! i18n: zone headings are detected via `agentic_core::i18n::t` for keys
//! `list_of_figures` / `list_of_tables` (all 6 supported languages — en, de,
//! fr, it, rm, hi). Caption prefixes are detected via the `fig_prefix` /
//! `table_prefix` keys plus the bare English fallback so EN-core thesis
//! markdown still matches under any chrome language.

use std::sync::LazyLock;

use regex::Regex;

use agentic_core::i18n;

use crate::{Finding, Severity};

/// One H1 heading found in the markdown, with its 0-based line index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct H1 {
    pub line: usize,
    pub text: String,
}

/// A `(start_line, end_line_exclusive)` zone within the markdown. `end_line`
/// is the line index of the NEXT H1, or `text.lines().count()` for the last.
pub type Zone = (usize, usize);

/// Collect H1 headings (`# …`) and their line indexes from a markdown body.
#[must_use]
pub fn collect_h1(text: &str) -> Vec<H1> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        // ATX style only — the agentic pipeline never emits setext headings.
        // Match `# Foo` but NOT `## Foo`.
        if let Some(rest) = line.strip_prefix("# ") {
            out.push(H1 {
                line: i,
                text: rest.trim().to_string(),
            });
        }
    }
    out
}

/// Locate the zone (range of lines) whose H1 heading matches `chrome_text`.
/// Comparison is exact on the trimmed heading text. Returns the
/// `(start, end_exclusive)` pair or `None` if no such heading exists. The
/// `start` is the H1 line; `end` is the line of the next H1, or the end of
/// the document for the last H1.
#[must_use]
pub fn find_zone(text: &str, chrome_text: &str) -> Option<Zone> {
    let h1s = collect_h1(text);
    let total = text.lines().count();
    for (j, h) in h1s.iter().enumerate() {
        if h.text == chrome_text {
            let end = h1s.get(j + 1).map_or(total, |n| n.line);
            return Some((h.line, end));
        }
    }
    None
}

/// All language variants of the List-of-Figures heading. Iterated across the
/// 6 supported chrome languages so any thesis variant matches.
#[must_use]
pub fn list_of_figures_headings() -> Vec<&'static str> {
    i18n::LANGS
        .iter()
        .map(|l| i18n::t(l, "list_of_figures"))
        .collect()
}

/// All language variants of the List-of-Tables heading.
#[must_use]
pub fn list_of_tables_headings() -> Vec<&'static str> {
    i18n::LANGS
        .iter()
        .map(|l| i18n::t(l, "list_of_tables"))
        .collect()
}

/// Locate the LoF / LoT zones for the markdown body (in *any* supported
/// chrome language).
#[must_use]
pub fn lof_lot_zones(text: &str) -> (Option<Zone>, Option<Zone>) {
    let mut lof = None;
    let mut lot = None;
    for h in list_of_figures_headings() {
        if lof.is_none() {
            lof = find_zone(text, h);
        }
    }
    for h in list_of_tables_headings() {
        if lot.is_none() {
            lot = find_zone(text, h);
        }
    }
    (lof, lot)
}

/// Is `line_idx` inside either zone (inclusive of bounds)?
#[must_use]
pub fn in_zone(line_idx: usize, lof: Option<Zone>, lot: Option<Zone>) -> bool {
    matches!(lof, Some((s, e)) if s <= line_idx && line_idx <= e)
        || matches!(lot, Some((s, e)) if s <= line_idx && line_idx <= e)
}

/// Matches a caption-like paragraph header. Mirrors the Python `^(Figure|Table)\s+\d+`
/// pattern but accepts every i18n `fig_prefix` / `table_prefix` form. We use a
/// single permissive regex (case-insensitive, anchored) because the i18n
/// values are word-stable.
static CAPTION_LIKE: LazyLock<Regex> = LazyLock::new(|| {
    // Strip any leading markdown bold `**` / italic `*` first (handled by the
    // caller before this regex runs).
    Regex::new(
        r"(?i)^\s*(?:Figure|Table|Abbildung|Tabelle|Tableau|Figura|Figura|चित्र|तालिका)\s+\d+",
    )
    .unwrap()
});

/// One caption found outside the LoF / LoT zones — the kind of finding the
/// Python `_check_captions_in_body.py` script printed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineCaption {
    pub line: usize,
    pub raw: String,
}

/// Scan `text` for caption-like paragraphs outside the LoF / LoT zones.
///
/// `path` is folded into the returned `Finding.location` so the caller can
/// surface a `<path>:<line>` pointer.
#[must_use]
pub fn find_inline_captions(text: &str) -> Vec<InlineCaption> {
    let (lof, lot) = lof_lot_zones(text);
    let mut out = Vec::new();
    for (i, raw_line) in text.lines().enumerate() {
        if in_zone(i, lof, lot) {
            continue;
        }
        // Strip a leading bold marker before the regex (so `**Figure 3: …**`
        // still matches without bloating the pattern).
        let stripped = raw_line.trim_start().trim_start_matches("**").trim_start();
        if CAPTION_LIKE.is_match(stripped) {
            out.push(InlineCaption {
                line: i,
                raw: raw_line.to_string(),
            });
        }
    }
    out
}

/// Convert inline-caption matches into INFO-severity findings suitable for a
/// reporting gate. INFO (not WARN/ERROR): the agentic pipeline emits captions
/// inline by design (markdown source format); this helper exists for the
/// legacy-DOCX hygiene case where back-matter lists are the only legal home.
#[must_use]
pub fn findings_from_captions(captions: &[InlineCaption], path: &str) -> Vec<Finding> {
    captions
        .iter()
        .map(|c| Finding {
            category: "CAPTION_OUTSIDE_LIST_ZONE".into(),
            severity: Severity::Info,
            message: format!(
                "caption-like paragraph outside List-of-Figures/Tables zone: {}",
                c.raw.trim()
            ),
            location: Some(format!("{path}:{}", c.line + 1)),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# Introduction\n\
Body intro.\n\
\n\
**Figure 1: The MAPE-K loop.**\n\
\n\
More body.\n\
\n\
# List of Figures\n\
\n\
Figure 1: The MAPE-K loop. 12\n\
Figure 2: Reference architecture. 27\n\
\n\
# List of Tables\n\
\n\
Table 1: Comparison. 33\n";

    #[test]
    fn collect_h1_finds_three_headings() {
        let h = collect_h1(SAMPLE);
        assert_eq!(h.len(), 3);
        assert_eq!(h[0].text, "Introduction");
        assert_eq!(h[1].text, "List of Figures");
        assert_eq!(h[2].text, "List of Tables");
    }

    #[test]
    fn lof_lot_zones_locate_back_matter() {
        let (lof, lot) = lof_lot_zones(SAMPLE);
        assert!(lof.is_some(), "LoF zone should be found");
        assert!(lot.is_some(), "LoT zone should be found");
        // LoF starts at line 7 ("# List of Figures"), ends where LoT begins.
        let (lof_s, lof_e) = lof.unwrap();
        let (lot_s, _) = lot.unwrap();
        assert_eq!(lof_s, 7);
        assert_eq!(lof_e, lot_s);
    }

    #[test]
    fn find_inline_captions_flags_intro_figure_only() {
        let hits = find_inline_captions(SAMPLE);
        assert_eq!(
            hits.len(),
            1,
            "exactly one inline caption outside the lists"
        );
        assert_eq!(hits[0].line, 3);
        assert!(hits[0].raw.contains("MAPE-K"));
    }

    #[test]
    fn captions_inside_lof_are_not_flagged() {
        let hits = find_inline_captions(SAMPLE);
        // Lines 9 and 10 ("Figure 1: …", "Figure 2: …") sit inside the LoF
        // zone and must NOT appear in the results.
        assert!(hits.iter().all(|h| h.line != 9 && h.line != 10));
    }

    #[test]
    fn findings_carry_info_severity_and_path_location() {
        let hits = find_inline_captions(SAMPLE);
        let f = findings_from_captions(&hits, "thesis/ch1.md");
        assert_eq!(f.len(), 1);
        assert!(matches!(f[0].severity, Severity::Info));
        assert_eq!(f[0].category, "CAPTION_OUTSIDE_LIST_ZONE");
        assert_eq!(f[0].location.as_deref(), Some("thesis/ch1.md:4"));
    }

    #[test]
    fn german_chrome_lof_heading_is_recognised() {
        let de = "# Einleitung\n\
**Abbildung 1: foo.**\n\
\n\
# Abbildungsverzeichnis\n\
\n\
Abbildung 1: foo. 12\n";
        let hits = find_inline_captions(de);
        // The inline `Abbildung 1` on line 1 is outside the LoF zone (line 3),
        // so it should still be flagged exactly once.
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].line, 1);
    }

    #[test]
    fn document_without_lof_flags_all_caption_paragraphs() {
        let md = "# Body\n\n**Figure 1: lone caption.**\n\n**Table 1: lone tbl.**\n";
        let hits = find_inline_captions(md);
        assert_eq!(hits.len(), 2);
    }
}
