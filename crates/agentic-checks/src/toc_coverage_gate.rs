//! `agentic check toc-coverage` — book-manifest ⇄ rendered-TOC coverage gate.
//!
//! Why: when a thesis-class book composes from many markdown chapters
//! (FHNW master_thesis profile has 14+ files: front-matter, body, back-matter),
//! it is easy for a chapter to drop out of the rendered docx without the
//! pipeline noticing — a manifest entry references a missing file, the
//! merge step silently skips it, or a Word TOC field is computed before
//! the body insertion finishes. The page-boundary gate measures *length*
//! but not *presence*; the bookkit gate audits prose style; this gate
//! closes the gap by comparing the manifest's declared chapter list
//! against the TOC entries Word actually wrote into the rendered docx.
//!
//! ## Inputs
//!
//! 1. `out/book_manifest.json` in the project worktree. Schema:
//!    `{"books":[{"key","title","subtitle","chapters":[<DB path>]}]}`.
//! 2. A snapshot directory on disk (e.g. `snapshots/<ts>-books-cascade/`)
//!    containing one `<book_key>.docx` per rendered book.
//!
//! Snapshots are intentionally on-disk only (ADR-0035): they are
//! regenerable from the DB and must not be ingested back into the
//! content store. The gate therefore takes the snapshot path as an
//! argument and reads the docx straight from the filesystem.
//!
//! ## Algorithm
//!
//! For every book in the manifest:
//!   1. Resolve the chapter list (markdown paths).
//!   2. For each chapter, read the markdown blob from the worktree and
//!      extract H1/H2/H3 heading texts in document order (ATX `#`/`##`/`###`).
//!   3. Locate `<snapshot_dir>/<book_key>.docx`. If absent, emit
//!      `TOC_NO_SNAPSHOT` (INFO) and skip — the gate never FAILs on
//!      missing renders.
//!   4. Open the docx, read `word/document.xml`, walk paragraphs and
//!      collect every `(level, text)` whose `<w:pStyle w:val="TOC1|TOC2|TOC3"/>`
//!      matches. The TOC the gate sees is the *rendered* TOC, i.e. the
//!      paragraphs Word emitted after expanding the `TOC \o "1-3"` field
//!      on the last save.
//!   5. Compare. A heading is matched by **normalised** text equality
//!      (trim, collapse internal whitespace, drop trailing page-number
//!      tabs that Word inserts into TOC entries). The match is greedy:
//!      every source heading consumes at most one TOC entry, in order.
//!
//! ## Findings
//!
//! | category | severity | meaning |
//! |---|---|---|
//! | `TOC_MISSING_ENTRY` | WARN | source heading has no matching TOC entry |
//! | `TOC_EXTRA_ENTRY` | WARN | TOC entry not traceable to any source heading |
//! | `TOC_LEVEL_MISMATCH` | INFO | matched by text but H-level ≠ TOC-level |
//! | `TOC_NO_SNAPSHOT` | INFO | no rendered docx for this book — skipped |
//! | `TOC_NO_MANIFEST` | INFO | no `out/book_manifest.json` in worktree |
//! | `TOC_COVERAGE_SUMMARY` | INFO | per-book coverage % |
//!
//! ## What this gate does NOT do
//!
//! * It does not read `word/styles.xml`. The source markdown's heading
//!   level is the ground truth; TOC level comes from the `<w:pStyle>`
//!   value the renderer wrote.
//! * It does not detect TOC re-ordering vs. source order — many FHNW
//!   thesis sections appear in a fixed order driven by the manifest,
//!   so an order mismatch would be louder than the gate's intended
//!   coverage signal. (A future TOC_ORDER finding could add it.)
//! * It does not consume `--rendered-docx`-style opt-in. If the
//!   snapshot dir is missing entirely, every book gets `TOC_NO_SNAPSHOT`
//!   and the gate PASSes with INFO — same pattern as render-fidelity.

use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::Connection;

use agentic_core::worktree;

use crate::{CheckReport, Finding, Severity};

/// Default manifest path inside the project worktree.
pub const MANIFEST_PATH: &str = "out/book_manifest.json";

/// One heading extracted from a chapter markdown.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceHeading {
    level: u8,
    text: String,
    /// `<chapter_path>:<line>` for diagnostics.
    location: String,
}

/// One TOC entry parsed from `word/document.xml`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TocEntry {
    level: u8,
    text: String,
}

/// Run the gate using the default manifest path.
pub fn run(conn: &Connection, project: &str, snapshot_dir: &Path) -> Result<CheckReport> {
    run_with_manifest(conn, project, MANIFEST_PATH, snapshot_dir)
}

/// Run the gate with an explicit manifest path (ad-hoc / testing).
pub fn run_with_manifest(
    conn: &Connection,
    project: &str,
    manifest_path: &str,
    snapshot_dir: &Path,
) -> Result<CheckReport> {
    let manifest_blob = match worktree::read_at(conn, project, manifest_path) {
        Ok(b) => b,
        Err(_) => {
            return Ok(CheckReport::new(
                "toc_coverage",
                vec![Finding {
                    category: "TOC_NO_MANIFEST".into(),
                    severity: Severity::Info,
                    message: format!(
                        "no {manifest_path} in worktree — gate runs no checks; \
                         render at least one book to populate it"
                    ),
                    location: Some(manifest_path.into()),
                }],
            ));
        }
    };
    let text = std::str::from_utf8(&manifest_blob.content)
        .map_err(|e| anyhow::anyhow!("{manifest_path} is not UTF-8: {e}"))?;
    let books = parse_manifest_books(text)
        .with_context(|| format!("parsing book manifest at {manifest_path}"))?;

    let mut findings = Vec::new();
    let mut books_checked = 0usize;
    let mut books_skipped = 0usize;

    for book in &books {
        let docx_path = snapshot_dir.join(format!("{}.docx", book.key));
        if !docx_path.exists() {
            findings.push(Finding {
                category: "TOC_NO_SNAPSHOT".into(),
                severity: Severity::Info,
                message: format!(
                    "no rendered docx for book '{}' at {} — skipped",
                    book.key,
                    docx_path.display()
                ),
                location: Some(docx_path.display().to_string()),
            });
            books_skipped += 1;
            continue;
        }
        books_checked += 1;

        // Collect every H1/H2/H3 across this book's chapters, in order.
        let mut source: Vec<SourceHeading> = Vec::new();
        for chapter_path in &book.chapters {
            let blob = match worktree::read_at(conn, project, chapter_path) {
                Ok(b) => b,
                Err(_) => continue, // a missing chapter is the tree-integrity gate's concern
            };
            let Ok(md) = std::str::from_utf8(&blob.content) else {
                continue;
            };
            source.extend(extract_markdown_headings(chapter_path, md));
        }

        // Extract the TOC entries from the rendered docx.
        let toc = match extract_toc_entries(&docx_path) {
            Ok(t) => t,
            Err(e) => {
                findings.push(Finding {
                    category: "TOC_NO_SNAPSHOT".into(),
                    severity: Severity::Info,
                    message: format!(
                        "could not read TOC from {} for book '{}': {e}",
                        docx_path.display(),
                        book.key
                    ),
                    location: Some(docx_path.display().to_string()),
                });
                continue;
            }
        };

        compare_book(&book.key, &source, &toc, &mut findings);
    }

    findings.push(Finding {
        category: "TOC_COVERAGE_SUMMARY".into(),
        severity: Severity::Info,
        message: format!(
            "books scanned: {} checked, {} skipped (no docx); manifest declared {} book(s)",
            books_checked,
            books_skipped,
            books.len(),
        ),
        location: Some(snapshot_dir.display().to_string()),
    });

    Ok(CheckReport::new("toc_coverage", findings))
}

/// Compare one book's source headings against its TOC entries, appending
/// findings (`TOC_MISSING_ENTRY`, `TOC_EXTRA_ENTRY`, `TOC_LEVEL_MISMATCH`,
/// `TOC_COVERAGE_SUMMARY`).
fn compare_book(
    book_key: &str,
    source: &[SourceHeading],
    toc: &[TocEntry],
    findings: &mut Vec<Finding>,
) {
    // Build a normalised-text index over TOC entries. Greedy in-order
    // consumption: each TOC entry is consumed at most once, by the first
    // source heading whose normalised text matches.
    let mut toc_consumed = vec![false; toc.len()];
    let mut matched = 0usize;

    for h in source {
        let target = normalise(&h.text);
        let mut hit: Option<usize> = None;
        for (i, t) in toc.iter().enumerate() {
            if !toc_consumed[i] && normalise(&t.text) == target {
                hit = Some(i);
                break;
            }
        }
        if let Some(i) = hit {
            toc_consumed[i] = true;
            matched += 1;
            if toc[i].level != h.level {
                findings.push(Finding {
                    category: "TOC_LEVEL_MISMATCH".into(),
                    severity: Severity::Info,
                    message: format!(
                        "book '{book_key}': heading '{}' is H{} in source but TOC{} in docx",
                        h.text, h.level, toc[i].level
                    ),
                    location: Some(h.location.clone()),
                });
            }
        } else {
            findings.push(Finding {
                category: "TOC_MISSING_ENTRY".into(),
                severity: Severity::Warn,
                message: format!(
                    "book '{book_key}': source heading '{}' (H{}) has no matching TOC entry",
                    h.text, h.level
                ),
                location: Some(h.location.clone()),
            });
        }
    }
    for (i, t) in toc.iter().enumerate() {
        if !toc_consumed[i] {
            findings.push(Finding {
                category: "TOC_EXTRA_ENTRY".into(),
                severity: Severity::Warn,
                message: format!(
                    "book '{book_key}': TOC{} entry '{}' is not traceable to any source heading",
                    t.level, t.text
                ),
                location: None,
            });
        }
    }
    let total = source.len();
    let pct = if total == 0 {
        100.0
    } else {
        (matched as f64) * 100.0 / (total as f64)
    };
    findings.push(Finding {
        category: "TOC_COVERAGE_SUMMARY".into(),
        severity: Severity::Info,
        message: format!(
            "book '{book_key}': {matched}/{total} source headings matched ({pct:.1}%); \
             {} TOC entries unmatched",
            toc_consumed.iter().filter(|c| !**c).count(),
        ),
        location: None,
    });
}

/// Parse the `books` array from the manifest JSON.
fn parse_manifest_books(json_text: &str) -> Result<Vec<ManifestBook>> {
    let v: serde_json::Value = serde_json::from_str(json_text)?;
    let books = v
        .get("books")
        .and_then(|b| b.as_array())
        .ok_or_else(|| anyhow::anyhow!("manifest has no `books` array"))?;
    let mut out = Vec::with_capacity(books.len());
    for b in books {
        let key = b
            .get("key")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("book entry missing string `key`"))?
            .to_owned();
        let chapters = b
            .get("chapters")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|c| c.as_str().map(str::to_owned))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        out.push(ManifestBook { key, chapters });
    }
    Ok(out)
}

#[derive(Debug, Clone)]
struct ManifestBook {
    key: String,
    chapters: Vec<String>,
}

/// Extract H1/H2/H3 headings from markdown. ATX-only (`#`, `##`, `###`)
/// — every chapter in this thesis uses ATX; setext (`===`/`---`) is not
/// used and would only inflate the gate's surface.
///
/// Skips fenced code blocks (``` and ~~~) so a `# comment` line inside
/// a Python sample doesn't get harvested as a heading.
fn extract_markdown_headings(chapter_path: &str, md: &str) -> Vec<SourceHeading> {
    let mut out = Vec::new();
    let mut in_fence = false;
    let mut fence_char = '`';
    for (i, line) in md.lines().enumerate() {
        let trimmed = line.trim_start();
        // Track fenced code blocks.
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            let c = trimmed.chars().next().unwrap_or('`');
            if !in_fence {
                in_fence = true;
                fence_char = c;
            } else if c == fence_char {
                in_fence = false;
            }
            continue;
        }
        if in_fence {
            continue;
        }
        if let Some((level, text)) = parse_atx_heading(trimmed) {
            out.push(SourceHeading {
                level,
                text,
                location: format!("{chapter_path}:{}", i + 1),
            });
        }
    }
    out
}

/// Parse an ATX heading `#…### text`. Returns `(level, text)` for
/// H1/H2/H3, else None. Trailing closing `#`s (Markdown spec allows
/// `## text ##`) are stripped.
fn parse_atx_heading(line: &str) -> Option<(u8, String)> {
    let mut hashes = 0u8;
    for c in line.chars() {
        if c == '#' {
            hashes += 1;
        } else {
            break;
        }
    }
    if !(1..=3).contains(&hashes) {
        return None;
    }
    let rest = &line[hashes as usize..];
    // The heading-text must be separated by at least one space (CommonMark).
    if !rest.starts_with(' ') && !rest.is_empty() {
        return None;
    }
    let mut text = rest.trim().to_owned();
    // Strip optional trailing `###…`.
    while text.ends_with('#') {
        text.pop();
    }
    let text = text.trim().to_owned();
    if text.is_empty() {
        return None;
    }
    Some((hashes, text))
}

/// Read the docx ZIP, parse `word/document.xml`, return TOC entries.
///
/// We walk the XML byte-stream looking for paragraph open tags
/// (`<w:p …>` … `</w:p>`) and for each paragraph, scan for
/// `<w:pStyle w:val="TOC1|TOC2|TOC3"/>`. When a TOC paragraph is found,
/// concatenate every `<w:t>` body inside that paragraph as the entry
/// text.
fn extract_toc_entries(docx: &Path) -> Result<Vec<TocEntry>> {
    let file =
        std::fs::File::open(docx).with_context(|| format!("opening docx {}", docx.display()))?;
    let mut zip = zip::ZipArchive::new(file).context("reading docx as zip")?;
    let mut xml = String::new();
    {
        let mut entry = zip
            .by_name("word/document.xml")
            .context("docx is missing word/document.xml")?;
        entry
            .read_to_string(&mut xml)
            .context("reading word/document.xml")?;
    }
    Ok(parse_toc_from_xml(&xml))
}

/// Split the document XML into paragraphs and emit one TocEntry per
/// paragraph whose pStyle is TOC1/TOC2/TOC3. Pure-string scan — keeps
/// the dependency surface to `zip` only.
fn parse_toc_from_xml(xml: &str) -> Vec<TocEntry> {
    let mut out = Vec::new();
    let bytes = xml.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        // Find the next `<w:p ` or `<w:p>` open tag.
        let Some(p_open) = find_subslice(bytes, b"<w:p", i) else {
            break;
        };
        // Reject `<w:pPr`, `<w:pStyle`, `<w:pageBreak`, etc.
        let after = p_open + 4;
        if after < bytes.len() && (bytes[after] == b' ' || bytes[after] == b'>') {
            // Found a real <w:p ...>. Locate the matching </w:p>.
            let Some(p_close) = find_subslice(bytes, b"</w:p>", after) else {
                break;
            };
            let para = &xml[p_open..p_close];
            if let Some(level) = detect_toc_level(para) {
                let text = collect_paragraph_text(para);
                let trimmed = trim_toc_text(&text);
                if !trimmed.is_empty() {
                    out.push(TocEntry {
                        level,
                        text: trimmed,
                    });
                }
            }
            i = p_close + b"</w:p>".len();
        } else {
            i = after;
        }
    }
    out
}

/// Return `Some(level)` if the paragraph carries `<w:pStyle w:val="TOC1|TOC2|TOC3"/>`.
fn detect_toc_level(paragraph_xml: &str) -> Option<u8> {
    for n in 1..=3u8 {
        let needle = format!("w:pStyle w:val=\"TOC{n}\"");
        if paragraph_xml.contains(&needle) {
            return Some(n);
        }
    }
    None
}

/// Concatenate every `<w:t>…</w:t>` body inside a paragraph XML fragment.
fn collect_paragraph_text(paragraph_xml: &str) -> String {
    let mut out = String::new();
    let bytes = paragraph_xml.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        let Some(t_open) = find_subslice(bytes, b"<w:t", i) else {
            break;
        };
        // Skip `<w:tbl`, `<w:tr`, `<w:tc`.
        let after = t_open + 4;
        if after >= bytes.len() {
            break;
        }
        if bytes[after] != b' ' && bytes[after] != b'>' {
            i = after;
            continue;
        }
        let Some(gt) = memchr_byte(bytes, b'>', after) else {
            break;
        };
        let body_start = gt + 1;
        let Some(close) = find_subslice(bytes, b"</w:t>", body_start) else {
            break;
        };
        out.push_str(&paragraph_xml[body_start..close]);
        i = close + b"</w:t>".len();
    }
    decode_xml_entities(&out)
}

/// Strip the page-number tab+digits Word appends to every TOC entry.
/// Real entries look like `Introduction\t3` or `Theory\t12` after
/// `<w:t>` concatenation. The tab is the canonical separator; if it is
/// missing (older Word, or the field never expanded), the whole text
/// survives.
fn trim_toc_text(raw: &str) -> String {
    let stripped = raw.split('\t').next().unwrap_or(raw);
    normalise_whitespace(stripped)
}

/// Normalise heading text for comparison: trim, collapse internal
/// whitespace (newlines, tabs, multi-space) to single spaces.
fn normalise(s: &str) -> String {
    normalise_whitespace(s).to_ascii_lowercase()
}

fn normalise_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_ws = false;
    for c in s.trim().chars() {
        if c.is_whitespace() {
            if !prev_ws {
                out.push(' ');
                prev_ws = true;
            }
        } else {
            out.push(c);
            prev_ws = false;
        }
    }
    out
}

/// Find `needle` in `haystack` starting at `from`. Naive scan — paragraph
/// XML fragments are tiny.
fn find_subslice(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || from > haystack.len() {
        return None;
    }
    let max = haystack.len().saturating_sub(needle.len());
    let mut i = from;
    while i <= max {
        if &haystack[i..i + needle.len()] == needle {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn memchr_byte(haystack: &[u8], byte: u8, from: usize) -> Option<usize> {
    haystack[from..]
        .iter()
        .position(|&b| b == byte)
        .map(|p| p + from)
}

/// Decode the five XML predefined entities. Mirrors the helper in
/// `commands/acronyms.rs`; copied (rather than re-exported) to keep
/// `agentic-checks` independent of the binary crate.
fn decode_xml_entities(src: &str) -> String {
    let mut dst = String::with_capacity(src.len());
    let mut rest = src;
    while let Some(amp) = rest.find('&') {
        dst.push_str(&rest[..amp]);
        let tail = &rest[amp..];
        if let Some(semi) = tail.find(';') {
            let ent = &tail[..=semi];
            match ent {
                "&amp;" => dst.push('&'),
                "&lt;" => dst.push('<'),
                "&gt;" => dst.push('>'),
                "&apos;" => dst.push('\''),
                "&quot;" => dst.push('"'),
                _ => {
                    if ent.len() > 3 && &ent[1..2] == "#" {
                        let digits = &ent[2..ent.len() - 1];
                        if let Ok(n) = digits.parse::<u32>() {
                            if let Some(ch) = char::from_u32(n) {
                                dst.push(ch);
                            } else {
                                dst.push_str(ent);
                            }
                        } else {
                            dst.push_str(ent);
                        }
                    } else {
                        dst.push_str(ent);
                    }
                }
            }
            rest = &tail[semi + 1..];
        } else {
            dst.push('&');
            rest = &tail[1..];
        }
    }
    dst.push_str(rest);
    dst
}

/// Path-exclusion helper kept for callers that want to skip chapters
/// from historical-preservation prefixes (mirrors `term_rename_gate`).
/// The TOC-coverage gate itself relies on the manifest's chapter list,
/// so it never scans the whole worktree; this helper is here for
/// future scoped variants and is unit-tested.
#[must_use]
pub fn is_excluded(path: &str) -> bool {
    path.starts_with("inbox/")
        || path.starts_with("thesis-draft-v")
        || path.starts_with("proposal/citations/")
        || path.starts_with("out/AUDIT_")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn parse_atx_h1_h2_h3() {
        assert_eq!(
            parse_atx_heading("# Introduction"),
            Some((1, "Introduction".into()))
        );
        assert_eq!(
            parse_atx_heading("## Theory of change"),
            Some((2, "Theory of change".into()))
        );
        assert_eq!(parse_atx_heading("### Detail"), Some((3, "Detail".into())));
        assert_eq!(parse_atx_heading("#### too deep"), None);
        assert_eq!(parse_atx_heading("plain text"), None);
        assert_eq!(parse_atx_heading("#noSpace"), None);
        // Trailing closing #.
        assert_eq!(parse_atx_heading("# Intro ##"), Some((1, "Intro".into())));
    }

    #[test]
    fn fenced_code_skips_heading_lookalike() {
        let md = "# Real\n```python\n# fake heading\n```\n## Also real\n";
        let h = extract_markdown_headings("x.md", md);
        assert_eq!(h.len(), 2);
        assert_eq!(h[0].text, "Real");
        assert_eq!(h[1].text, "Also real");
    }

    #[test]
    fn toc_paragraph_extraction_finds_three_levels() {
        let xml = r#"<w:p><w:pPr><w:pStyle w:val="TOC1"/></w:pPr><w:r><w:t>Introduction</w:t></w:r><w:r><w:t>	3</w:t></w:r></w:p>
<w:p><w:pPr><w:pStyle w:val="TOC2"/></w:pPr><w:r><w:t>Background</w:t></w:r></w:p>
<w:p><w:pPr><w:pStyle w:val="TOC3"/></w:pPr><w:r><w:t>Detail</w:t></w:r></w:p>
<w:p><w:pPr><w:pStyle w:val="Normal"/></w:pPr><w:r><w:t>Body text not in TOC</w:t></w:r></w:p>"#;
        let toc = parse_toc_from_xml(xml);
        assert_eq!(toc.len(), 3);
        assert_eq!(toc[0].level, 1);
        assert_eq!(toc[0].text, "Introduction");
        assert_eq!(toc[1].level, 2);
        assert_eq!(toc[1].text, "Background");
        assert_eq!(toc[2].level, 3);
        assert_eq!(toc[2].text, "Detail");
    }

    #[test]
    fn xml_entities_decoded_in_toc_text() {
        let xml = r#"<w:p><w:pPr><w:pStyle w:val="TOC1"/></w:pPr><w:r><w:t>Mitre ATT&amp;CK</w:t></w:r></w:p>"#;
        let toc = parse_toc_from_xml(xml);
        assert_eq!(toc.len(), 1);
        assert_eq!(toc[0].text, "Mitre ATT&CK");
    }

    #[test]
    fn compare_emits_missing_extra_and_summary() {
        let source = vec![
            SourceHeading {
                level: 1,
                text: "Introduction".into(),
                location: "ch1.md:1".into(),
            },
            SourceHeading {
                level: 1,
                text: "Theory".into(),
                location: "ch2.md:1".into(),
            },
            SourceHeading {
                level: 1,
                text: "Only in source".into(),
                location: "ch3.md:1".into(),
            },
        ];
        let toc = vec![
            TocEntry {
                level: 1,
                text: "Introduction".into(),
            },
            TocEntry {
                level: 2,
                text: "Theory".into(),
            }, // level mismatch
            TocEntry {
                level: 1,
                text: "Only in TOC".into(),
            },
        ];
        let mut findings = Vec::new();
        compare_book("test_book", &source, &toc, &mut findings);
        let cats: BTreeMap<&str, usize> = findings.iter().fold(BTreeMap::new(), |mut m, f| {
            *m.entry(f.category.as_str()).or_insert(0) += 1;
            m
        });
        assert_eq!(cats.get("TOC_MISSING_ENTRY").copied(), Some(1));
        assert_eq!(cats.get("TOC_EXTRA_ENTRY").copied(), Some(1));
        assert_eq!(cats.get("TOC_LEVEL_MISMATCH").copied(), Some(1));
        assert_eq!(cats.get("TOC_COVERAGE_SUMMARY").copied(), Some(1));
    }

    #[test]
    fn normalise_collapses_whitespace_and_lowercases() {
        assert_eq!(normalise("  Hello   World\n\t"), "hello world");
        assert_eq!(normalise("Same"), "same");
    }

    #[test]
    fn trim_toc_text_strips_tab_page_number() {
        assert_eq!(trim_toc_text("Introduction\t3"), "Introduction");
        assert_eq!(trim_toc_text("No tab"), "No tab");
        assert_eq!(trim_toc_text("Multi\twords\ttab"), "Multi");
    }

    #[test]
    fn parse_manifest_books_picks_up_keys_and_chapters() {
        let json = r#"{
            "books": [
                {"key":"master_thesis","title":"T","subtitle":"S",
                 "chapters":["thesis/fhnw_1_introduction.md","thesis/fhnw_2_theory.md"]},
                {"key":"dim_01","title":"Agile","chapters":[]}
            ]
        }"#;
        let books = parse_manifest_books(json).unwrap();
        assert_eq!(books.len(), 2);
        assert_eq!(books[0].key, "master_thesis");
        assert_eq!(books[0].chapters.len(), 2);
        assert_eq!(books[1].key, "dim_01");
        assert!(books[1].chapters.is_empty());
    }

    #[test]
    fn is_excluded_matches_historical_prefixes() {
        assert!(is_excluded("inbox/foo.md"));
        assert!(is_excluded("thesis-draft-v5/x.md"));
        assert!(is_excluded("proposal/citations/x"));
        assert!(is_excluded("out/AUDIT_latest.md"));
        assert!(!is_excluded("thesis/fhnw_1_introduction.md"));
    }
}
