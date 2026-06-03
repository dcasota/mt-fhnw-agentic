//! Back-of-book index harvest + render (Wave 7, AI-Norms parity, 2026-06-03).
//!
//! Reference parity goal: the AI_Norms_and_Regulations_BOOK.docx ships a
//! 113-entry back-of-book index, grouped under 20 A-Z `IndexHeading`
//! dividers. The previous engine emitted only Word's `INDEX` field driven
//! by XE marks (32 curated terms, no letter dividers). This module
//! introduces a deterministic harvester + renderer that:
//!
//!   * **Part A (`collect_index_entries`)** — walks every chapter markdown
//!     and harvests:
//!       1. Explicit `{{index:term}}` markers (curator-placed in the
//!          chapter source — the canonical signal).
//!       2. Auto-extracted proper-noun candidates matched against a
//!          curated allowlist (`specs/books/ai_norms/index_allowlist.toml`)
//!          so unmarked chapters still contribute to the index when their
//!          prose mentions an allowlisted term.
//!     The output is a deduplicated [`Vec<IndexEntry>`] sorted
//!     case-insensitively by `term`.
//!
//!   * **Part B (`emit_index_section`)** — produces the docx blocks for
//!     the rendered index: one [`IndexBlock::Heading`] per letter that has
//!     at least one entry, followed by one [`IndexBlock::Entry`] per term
//!     under that letter. Empty letters are skipped (no "Q" heading when
//!     no Q entries exist). The caller maps each block onto a
//!     `docx_rs::Paragraph` with `pStyle=IndexHeading` / `pStyle=Index1`
//!     so the rendered docx uses the verbatim reference styles emitted by
//!     [`crate::styles_xml`].
//!
//! Page-number resolution is deferred to Word's PAGEREF field update on
//! open — the renderer emits a PAGEREF placeholder that Word fills in at
//! field-update time, matching the reference book's "dot-leader to page N"
//! visual.

use std::collections::BTreeMap;

/// One back-of-book index entry: the term + the ordered set of chapter
/// references where it was harvested.
///
/// `refs` records, in document order, every chapter path the term was
/// seen in. The renderer collapses these into a comma-separated list of
/// PAGEREF placeholders that Word resolves to page numbers on field
/// update (the leader-dot + page-number visual of the reference book).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexEntry {
    /// The display term (preserves the curated capitalisation, e.g.
    /// "ATT&CK" stays mixed-case; "AI Risk Management Framework" stays
    /// title-case). Sorting downstream is case-insensitive.
    pub term: String,
    /// Chapter references where the term was harvested, in document
    /// order, de-duplicated by chapter path. Empty when the term came
    /// from the allowlist but did not appear in any chapter (shouldn't
    /// happen in practice — the auto-harvester only emits a hit when the
    /// term is present in body text).
    pub refs: Vec<ChapterPageRef>,
}

/// A single in-chapter reference for an index entry. The `chapter`
/// field is the chapter's logical path (manifest key / file path); the
/// `bookmark` is the chapter's heading anchor name, used to construct a
/// PAGEREF field that resolves to the chapter's start page on Word
/// field-update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChapterPageRef {
    /// Chapter logical path (e.g. `out/sources/norms/06_norms_EN.md`).
    pub chapter: String,
    /// Bookmark name to PAGEREF against. Empty → the renderer falls
    /// back to a plain `?` placeholder (no PAGEREF) so the visual at
    /// least shows the entry even without a resolvable target.
    pub bookmark: String,
}

/// One emitted block in the rendered index. `Heading("A")` becomes a
/// `pStyle=IndexHeading` paragraph; `Entry { term, refs }` becomes a
/// `pStyle=Index1` paragraph with the term + leader-dot tab +
/// PAGEREF cross-references.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexBlock {
    /// A letter divider (A, B, C, …). Rendered as an `IndexHeading`
    /// paragraph.
    Heading(String),
    /// One curated entry. Rendered as an `Index1` paragraph.
    Entry {
        term: String,
        refs: Vec<ChapterPageRef>,
    },
}

/// Curated allowlist for the auto-harvest pass.
///
/// Loaded from `specs/books/ai_norms/index_allowlist.toml`. Each entry
/// is a string matched case-insensitively against chapter prose. Hits
/// promote the entry to the index even when no explicit
/// `{{index:term}}` marker is present in the chapter source.
#[derive(Debug, Clone, Default)]
pub struct IndexAllowlist {
    pub terms: Vec<String>,
}

impl IndexAllowlist {
    /// Parse a TOML allowlist of the form
    /// ```toml
    /// [allowlist]
    /// terms = [
    ///   "GDPR",
    ///   "ISO/IEC 42001",
    /// ]
    /// ```
    ///
    /// Intentionally NOT a general TOML parser — the workspace does not
    /// depend on the `toml` crate, and we only need a narrow slice of the
    /// grammar (a single `[allowlist]` table with a single `terms =
    /// [...]` array of double-quoted strings). The parser is line-
    /// oriented:
    ///
    ///   * lines starting with `#` (after optional whitespace) and blank
    ///     lines are skipped (comments).
    ///   * `[allowlist]` enters the active table; anything outside it is
    ///     rejected as a parse error.
    ///   * `terms = [` opens the array; subsequent lines are parsed for
    ///     `"…",`-style entries until a closing `]` is seen.
    ///   * Backslash escapes inside strings are NOT supported (the
    ///     allowlist is a curated list of canonical names — none of the
    ///     terms in the reference book contain quotes or backslashes).
    ///
    /// Returns an error with a brief hint on the first malformed line so
    /// curators get an actionable message when authoring the allowlist.
    pub fn parse_toml(src: &str) -> Result<Self, String> {
        #[derive(PartialEq, Eq)]
        enum State {
            Outside,
            InAllowlistTable,
            InTermsArray,
        }
        let mut state = State::Outside;
        let mut terms: Vec<String> = Vec::new();
        let mut saw_allowlist = false;

        for (lineno, raw) in src.lines().enumerate() {
            let line = raw.trim();
            // Strip an end-of-line `# comment` (only outside a string;
            // strings can't contain `#` in our curated allowlist).
            let line = match line.find('#') {
                Some(0) => "",
                Some(pos) if !line[..pos].contains('"') => line[..pos].trim_end(),
                _ => line,
            };
            if line.is_empty() {
                continue;
            }

            // Table header.
            if let Some(name) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                let n = name.trim();
                if n == "allowlist" {
                    state = State::InAllowlistTable;
                    saw_allowlist = true;
                    continue;
                }
                return Err(format!("line {}: unexpected table [{n}]", lineno + 1));
            }

            match state {
                State::Outside => {
                    return Err(format!(
                        "line {}: content outside [allowlist] table",
                        lineno + 1
                    ));
                }
                State::InAllowlistTable => {
                    // Expect `terms = [` (possibly followed by entries on
                    // the same line).
                    if let Some(rest) = line.strip_prefix("terms").map(str::trim_start) {
                        let rest = rest
                            .strip_prefix('=')
                            .ok_or_else(|| {
                                format!("line {}: expected `terms = [...]`", lineno + 1)
                            })?
                            .trim_start();
                        let rest = rest.strip_prefix('[').ok_or_else(|| {
                            format!("line {}: terms value must start with `[`", lineno + 1)
                        })?;
                        state = State::InTermsArray;
                        if matches!(
                            parse_array_line(rest, &mut terms, lineno + 1)?,
                            ArrayLineResult::Closed
                        ) {
                            state = State::InAllowlistTable;
                        }
                    } else {
                        return Err(format!(
                            "line {}: only `terms = [...]` permitted inside [allowlist]",
                            lineno + 1
                        ));
                    }
                }
                State::InTermsArray => {
                    if matches!(
                        parse_array_line(line, &mut terms, lineno + 1)?,
                        ArrayLineResult::Closed
                    ) {
                        state = State::InAllowlistTable;
                    }
                }
            }
        }

        if !saw_allowlist {
            return Err("missing [allowlist] table".to_string());
        }
        if matches!(state, State::InTermsArray) {
            return Err("unterminated terms = [...] array".to_string());
        }
        Ok(Self { terms })
    }
}

/// Outcome of parsing one segment of the `terms = [ ... ]` array.
enum ArrayLineResult {
    /// Closing `]` was seen on this segment; the array is fully parsed.
    Closed,
    /// No `]` seen; expect more entries on the next line.
    StillOpen,
}

/// Parse one segment of the `terms = [ ... ]` array (a line, or the
/// remainder of the `terms = [` line after the opening bracket).
///
/// Consumes `"…"` string entries separated by `,`; returns
/// [`ArrayLineResult::Closed`] when the closing `]` is encountered so
/// the caller can transition out of `InTermsArray` state.
fn parse_array_line(
    mut rest: &str,
    terms: &mut Vec<String>,
    lineno: usize,
) -> Result<ArrayLineResult, String> {
    loop {
        rest = rest.trim_start();
        if rest.is_empty() {
            return Ok(ArrayLineResult::StillOpen);
        }
        if let Some(after) = rest.strip_prefix(']') {
            let tail = after.trim();
            if !tail.is_empty() && !tail.starts_with('#') {
                return Err(format!(
                    "line {lineno}: unexpected content after closing `]`: `{tail}`"
                ));
            }
            return Ok(ArrayLineResult::Closed);
        }
        if let Some(after) = rest.strip_prefix(',') {
            rest = after;
            continue;
        }
        let after_open = rest.strip_prefix('"').ok_or_else(|| {
            format!(
                "line {lineno}: expected `\"` to start a string, got `{}`",
                rest.chars().next().unwrap_or(' ')
            )
        })?;
        let close = after_open
            .find('"')
            .ok_or_else(|| format!("line {lineno}: unterminated string in terms array"))?;
        let term = &after_open[..close];
        terms.push(term.to_string());
        rest = &after_open[close + 1..];
    }
}

/// Scan a chapter markdown body for `{{index:term}}` markers.
///
/// Returns each captured term in source order with duplicates preserved
/// — the caller de-dupes when assembling cross-references. Whitespace
/// around the term is trimmed; empty markers (`{{index:}}`) are
/// silently dropped.
fn scan_explicit_markers(md: &str) -> Vec<String> {
    const OPEN: &str = "{{index:";
    const CLOSE: &str = "}}";
    let mut out = Vec::new();
    let mut rest = md;
    while let Some(start) = rest.find(OPEN) {
        let after = &rest[start + OPEN.len()..];
        let Some(end) = after.find(CLOSE) else {
            break;
        };
        let term = after[..end].trim();
        if !term.is_empty() {
            out.push(term.to_string());
        }
        rest = &after[end + CLOSE.len()..];
    }
    out
}

/// Scan a chapter for auto-harvest hits from the allowlist (case-
/// insensitive substring match).
///
/// Each hit is reported once per (chapter, term) pair — the caller
/// collapses multiple chapters into one entry. The matched term is the
/// allowlist string (preserves its curated capitalisation), not the
/// chapter prose.
fn scan_allowlist_hits<'a>(md: &str, allowlist: &'a [String]) -> Vec<&'a str> {
    let lower = md.to_lowercase();
    let mut out = Vec::new();
    for term in allowlist {
        let needle = term.to_lowercase();
        if !needle.is_empty() && lower.contains(&needle) {
            out.push(term.as_str());
        }
    }
    out
}

/// Strip the chapter's heading-anchor bookmark name from its first H1.
///
/// Mirrors `book::heading_anchor_name`: when the H1 starts with
/// `<digits> <space>` we use `chN`; otherwise we slugify the full text.
/// Falls back to `"section"` for headingless chapters.
fn chapter_bookmark(md: &str) -> String {
    let h1 = md.lines().find_map(|l| {
        let t = l.trim_start();
        t.strip_prefix("# ").map(|s| s.trim().to_string())
    });
    let Some(t) = h1 else {
        return "section".to_string();
    };
    if let Some((num, _)) = t.split_once(' ') {
        if !num.is_empty() && num.chars().all(|c| c.is_ascii_digit()) {
            return format!("ch{num}");
        }
    }
    let mut out = String::with_capacity(t.len());
    let mut prev_hyphen = true;
    for c in t.chars() {
        let lc = c.to_ascii_lowercase();
        if lc.is_ascii_alphanumeric() {
            out.push(lc);
            prev_hyphen = false;
        } else if matches!(c, ' ' | '\t' | '-' | '_' | '/' | '.' | ',' | ':' | ';') && !prev_hyphen
        {
            out.push('-');
            prev_hyphen = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "section".to_string()
    } else {
        out
    }
}

/// Harvest index entries from a `(chapter_path, markdown)` corpus.
///
/// Produces a sorted, deduplicated `Vec<IndexEntry>` from:
///   * **Explicit markers** — every `{{index:term}}` in chapter source
///     contributes an entry, even if `term` is NOT on the allowlist.
///   * **Auto-harvest** — every allowlist term whose lowercase form
///     appears anywhere in a chapter's prose contributes an entry
///     against that chapter.
///
/// Output ordering: case-insensitive by `term`, ties broken by the
/// curated capitalisation (preserves stable ordering across runs).
/// Within each entry, `refs` follow document order (input chapter
/// order), deduplicated by `chapter` (only one ref per chapter even if
/// the term appears multiple times — the reference book's index does
/// not list the same page twice per term).
pub fn collect_index_entries(
    chapters: &[(String, String)],
    allowlist: &IndexAllowlist,
) -> Vec<IndexEntry> {
    // BTreeMap keyed on the case-folded term so we get deterministic
    // case-insensitive ordering for free; the value is (canonical-term,
    // ordered refs).
    let mut bucket: BTreeMap<String, (String, Vec<ChapterPageRef>)> = BTreeMap::new();
    for (path, md) in chapters {
        let bookmark = chapter_bookmark(md);
        let mut seen_in_chapter: std::collections::HashSet<String> =
            std::collections::HashSet::new();

        // 1) Explicit markers always win on canonical capitalisation.
        for term in scan_explicit_markers(md) {
            let key = term.to_lowercase();
            if !seen_in_chapter.insert(key.clone()) {
                continue;
            }
            let entry = bucket.entry(key).or_insert_with(|| (term.clone(), vec![]));
            // Prefer the explicit-marker capitalisation over any earlier
            // auto-harvest one (the curator wrote it that way).
            entry.0 = term.clone();
            entry.1.push(ChapterPageRef {
                chapter: path.clone(),
                bookmark: bookmark.clone(),
            });
        }

        // 2) Auto-harvest from the allowlist.
        for term in scan_allowlist_hits(md, &allowlist.terms) {
            let key = term.to_lowercase();
            if !seen_in_chapter.insert(key.clone()) {
                continue;
            }
            let entry = bucket
                .entry(key)
                .or_insert_with(|| (term.to_string(), vec![]));
            entry.1.push(ChapterPageRef {
                chapter: path.clone(),
                bookmark: bookmark.clone(),
            });
        }
    }

    bucket
        .into_iter()
        .map(|(_lc, (term, refs))| IndexEntry { term, refs })
        .collect()
}

/// First non-whitespace ASCII letter of `term`, uppercased. Used as
/// the letter-divider key. Non-alphabetic leading characters fall back
/// to `'#'` (numbers, symbols) so terms like ".NET" still group under
/// a deterministic divider rather than crashing.
fn first_letter(term: &str) -> char {
    for c in term.chars() {
        if c.is_ascii_alphabetic() {
            return c.to_ascii_uppercase();
        }
    }
    '#'
}

/// Convert a sorted `Vec<IndexEntry>` into the renderer's block stream.
///
/// Emits one `IndexBlock::Heading(letter)` for each letter that has at
/// least one entry, followed by every `IndexBlock::Entry` for that
/// letter (in input order — caller must pre-sort). Empty letters are
/// skipped: when no Q-terms exist, no "Q" divider is emitted.
pub fn emit_index_section(entries: Vec<IndexEntry>) -> Vec<IndexBlock> {
    let mut out: Vec<IndexBlock> = Vec::new();
    let mut current_letter: Option<char> = None;
    for entry in entries {
        let letter = first_letter(&entry.term);
        if current_letter != Some(letter) {
            out.push(IndexBlock::Heading(letter.to_string()));
            current_letter = Some(letter);
        }
        out.push(IndexBlock::Entry {
            term: entry.term,
            refs: entry.refs,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allow(terms: &[&str]) -> IndexAllowlist {
        IndexAllowlist {
            terms: terms.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn index_scan_explicit_markers_extracts_terms() {
        let md = "Body text {{index:ISO/IEC 42001}} and more {{index:GDPR}}.";
        assert_eq!(
            scan_explicit_markers(md),
            vec!["ISO/IEC 42001".to_string(), "GDPR".to_string()]
        );
    }

    #[test]
    fn index_scan_explicit_markers_trims_whitespace() {
        let md = "see {{index:  GDPR  }} for details";
        assert_eq!(scan_explicit_markers(md), vec!["GDPR".to_string()]);
    }

    #[test]
    fn index_scan_explicit_markers_skips_empty() {
        let md = "{{index:}} {{index:   }} {{index:GDPR}}";
        assert_eq!(scan_explicit_markers(md), vec!["GDPR".to_string()]);
    }

    #[test]
    fn index_scan_explicit_markers_handles_unterminated() {
        let md = "{{index:GDPR}} and {{index:unterminated";
        assert_eq!(scan_explicit_markers(md), vec!["GDPR".to_string()]);
    }

    #[test]
    fn index_scan_allowlist_hits_case_insensitive() {
        let md = "We discuss gdpr and the EU AI Act here.";
        let allowlist = allow(&["GDPR", "EU AI Act", "ISO/IEC 42001"]);
        let hits = scan_allowlist_hits(md, &allowlist.terms);
        assert_eq!(hits, vec!["GDPR", "EU AI Act"]);
    }

    #[test]
    fn index_collect_dedupes_by_chapter() {
        let chapters = vec![(
            "c1".to_string(),
            "# 1 Foo\n\nGDPR and GDPR appear twice.".to_string(),
        )];
        let entries = collect_index_entries(&chapters, &allow(&["GDPR"]));
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].term, "GDPR");
        assert_eq!(entries[0].refs.len(), 1, "one ref per chapter even with multiple hits");
        assert_eq!(entries[0].refs[0].bookmark, "ch1");
    }

    #[test]
    fn index_collect_merges_across_chapters() {
        let chapters = vec![
            ("c1".to_string(), "# 1 Foo\n\nGDPR.".to_string()),
            ("c2".to_string(), "# 2 Bar\n\nGDPR.".to_string()),
        ];
        let entries = collect_index_entries(&chapters, &allow(&["GDPR"]));
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].refs.len(), 2);
        assert_eq!(entries[0].refs[0].chapter, "c1");
        assert_eq!(entries[0].refs[1].chapter, "c2");
    }

    #[test]
    fn index_collect_sorts_case_insensitive() {
        let chapters = vec![(
            "c1".to_string(),
            "# 1 Foo\n\nbar, Alpha, alpha, Beta.".to_string(),
        )];
        let entries = collect_index_entries(&chapters, &allow(&["Alpha", "bar", "Beta"]));
        let terms: Vec<&str> = entries.iter().map(|e| e.term.as_str()).collect();
        assert_eq!(terms, vec!["Alpha", "bar", "Beta"]);
    }

    #[test]
    fn index_collect_explicit_marker_overrides_capitalisation() {
        // Explicit `{{index:gdpr}}` wins over the allowlist "GDPR" because
        // the curator wrote it that way.
        let chapters = vec![(
            "c1".to_string(),
            "# 1 Foo\n\n{{index:gdpr}} and GDPR.".to_string(),
        )];
        let entries = collect_index_entries(&chapters, &allow(&["GDPR"]));
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].term, "gdpr");
    }

    #[test]
    fn index_emit_skips_empty_letters() {
        let entries = vec![
            IndexEntry {
                term: "Alpha".to_string(),
                refs: vec![],
            },
            IndexEntry {
                term: "Zulu".to_string(),
                refs: vec![],
            },
        ];
        let blocks = emit_index_section(entries);
        // 2 headings (A, Z) + 2 entries, no B-Y dividers.
        assert_eq!(blocks.len(), 4);
        assert!(matches!(&blocks[0], IndexBlock::Heading(h) if h == "A"));
        assert!(matches!(&blocks[2], IndexBlock::Heading(h) if h == "Z"));
    }

    #[test]
    fn index_emit_groups_entries_under_letter() {
        let entries = vec![
            IndexEntry {
                term: "Alpha".to_string(),
                refs: vec![],
            },
            IndexEntry {
                term: "Apex".to_string(),
                refs: vec![],
            },
            IndexEntry {
                term: "Beta".to_string(),
                refs: vec![],
            },
        ];
        let blocks = emit_index_section(entries);
        assert_eq!(blocks.len(), 5);
        assert!(matches!(&blocks[0], IndexBlock::Heading(h) if h == "A"));
        assert!(matches!(&blocks[3], IndexBlock::Heading(h) if h == "B"));
    }

    #[test]
    fn index_allowlist_parses_canonical_toml() {
        let src = r#"
            [allowlist]
            terms = ["GDPR", "ISO/IEC 42001"]
        "#;
        let a = IndexAllowlist::parse_toml(src).unwrap();
        assert_eq!(a.terms, vec!["GDPR".to_string(), "ISO/IEC 42001".to_string()]);
    }

    #[test]
    fn index_allowlist_rejects_missing_table() {
        let src = "terms = []";
        assert!(IndexAllowlist::parse_toml(src).is_err());
    }

    #[test]
    fn index_chapter_bookmark_numbered_chapter() {
        assert_eq!(chapter_bookmark("# 3 Foo Bar\n\nbody"), "ch3");
        assert_eq!(chapter_bookmark("# 12 Asia Pacific\n\nbody"), "ch12");
    }

    #[test]
    fn index_chapter_bookmark_unnumbered_chapter() {
        assert_eq!(chapter_bookmark("# Foreword\n\nbody"), "foreword");
        assert_eq!(chapter_bookmark("# AI Norms\n\nbody"), "ai-norms");
    }

    #[test]
    fn index_first_letter_falls_back_for_non_alpha() {
        assert_eq!(first_letter("42 Things"), 'T');
        assert_eq!(first_letter("___"), '#');
        assert_eq!(first_letter(""), '#');
        assert_eq!(first_letter("ATT&CK"), 'A');
    }

    #[test]
    fn index_visible_113_entry_target_when_fully_curated() {
        // Smoke test: when every allowlist term appears in some chapter,
        // we recover one entry per term — the Wave 7 parity target.
        let mut chapters = Vec::new();
        let mut allow_terms = Vec::new();
        for i in 0..113 {
            let term = format!("Term{i:03}");
            allow_terms.push(term.clone());
            chapters.push((format!("c{i}"), format!("# {i} Foo\n\n{term} body.")));
        }
        let entries = collect_index_entries(
            &chapters,
            &IndexAllowlist {
                terms: allow_terms,
            },
        );
        assert_eq!(entries.len(), 113);
    }
}
