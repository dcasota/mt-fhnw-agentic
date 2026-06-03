//! `agentic acronyms` — refresh the Pages column of the front-matter
//! acronyms table (`out/sources/frontmatter/acronyms.md`) from a rendered
//! `master_thesis.docx`.
//!
//! ## Algorithm (v4, token-aware)
//!
//! 1. Open the DOCX as a ZIP, read `word/document.xml`.
//! 2. Stream-scan the XML for `<w:t>` (visible text) and
//!    `<w:lastRenderedPageBreak/>` (page-break hints Word writes on
//!    every save). Build a concatenated text buffer plus a sorted
//!    `(byte_offset, page_number)` index.
//! 3. Parse the acronyms table from the markdown source (left column).
//! 4. For each acronym:
//!    - Walk the text with case-sensitive `find()` (Rust's
//!      `str::find`, not regex — fast, deterministic).
//!    - For every match, apply the v4 surrounding-char post-filter:
//!      reject when the byte immediately before or after the match
//!      is alphanumeric (this filters substring matches like "AI"
//!      inside "AIBOM" while admitting "AI" inside "AI-First").
//!    - Map each accepted offset to its page via binary search on
//!      the page-break index.
//! 5. Rewrite the Pages column in the markdown source and write the
//!    updated blob back to the project's working tree.
//!
//! The DOCX-side page numbers depend on Word having opened and saved
//! the file at least once (otherwise `lastRenderedPageBreak` hints are
//! missing). The Word-COM `agentic book finalize` step satisfies this.
//!
//! ## Known limitations
//!
//! - Plurals: `AIs` is rejected (the trailing `s` is alphanumeric).
//!   This matches the current expansion-column convention (acronyms are
//!   listed in their canonical capitalised form, not their plural).
//! - camelCase compounds (`AIBom` instead of `AIBOM`) are rejected —
//!   these are anti-patterns in this thesis and none exist in the corpus.
//! - Acronyms whose canonical form contains a digit (`MAPE-K` has no
//!   digit; `AI100-2` would) are matched as substrings; the post-filter
//!   still applies.

use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};

use agentic_core::worktree;

use crate::cli::AcronymsAction;

/// Front-matter acronyms-table path inside the project worktree.
const ACRONYMS_MD: &str = "out/sources/frontmatter/acronyms.md";

/// Dispatch the top-level `acronyms` verb.
pub fn run(db_path: &Path, action: AcronymsAction, json: bool) -> Result<()> {
    match action {
        AcronymsAction::Refresh {
            project,
            docx,
            dry_run,
            add_missing,
            drop_zero_page,
        } => refresh(
            db_path,
            &project,
            &docx,
            dry_run,
            add_missing,
            drop_zero_page,
            json,
        ),
    }
}

/// Three-letter ALL-CAPS English-word stop list (REQ-2 candidate filter).
/// Some collide with legitimate acronyms (`THE`, `WHO`) — listing them
/// errs on the side of false negatives, since the author can always
/// add the row by hand. The list is exact-3-letter only; 2-letter
/// candidates (`AI`, `EU`) and 4+-letter candidates (`HITL`, `AIBOM`)
/// are always considered acronym-shaped.
const STOPWORDS_3: &[&str] = &[
    "AND", "BUT", "FOR", "THE", "WHO", "YOU", "ARE", "HER", "ITS", "OUR", "ONE", "TWO", "ANY",
    "ALL", "DAY", "GET", "HAD", "HAS", "HIS", "HOW", "MAY", "NEW", "NOW", "OLD", "SEE", "WAY",
    "BOY", "DID", "MAN", "OUT", "PUT", "SAY", "SHE", "TOO", "USE",
];

#[derive(Debug, Default, Clone, serde::Serialize)]
struct PageAwareText {
    /// Concatenated text from every `<w:t>` element, in document order.
    text: String,
    /// Sorted `(byte_offset, page_number)`. The page valid AT `offset`
    /// is the largest `page_number` whose `offset` is `<= query`.
    /// Always starts with `(0, 1)`.
    breaks: Vec<(usize, u32)>,
}

impl PageAwareText {
    fn page_at(&self, offset: usize) -> u32 {
        match self.breaks.binary_search_by_key(&offset, |&(o, _)| o) {
            Ok(i) => self.breaks[i].1,
            Err(0) => 1,
            Err(i) => self.breaks[i - 1].1,
        }
    }
}

/// Extract page-aware text from the DOCX.
///
/// Word writes `<w:lastRenderedPageBreak/>` inside the run that contains
/// the visible text appearing immediately AFTER a page boundary on the
/// last render. By scanning these markers in document order we know
/// exactly which page every text byte belongs to.
fn extract_page_aware_text(docx: &Path) -> Result<PageAwareText> {
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

    let mut out = PageAwareText {
        text: String::with_capacity(xml.len() / 4),
        breaks: vec![(0, 1)],
    };
    let mut page: u32 = 1;
    let bytes = xml.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        // Find the end of the tag
        let tag_end = match memchr(b'>', bytes, i + 1) {
            Some(p) => p,
            None => break,
        };
        let tag = &bytes[i..=tag_end];
        if starts_with(tag, b"<w:lastRenderedPageBreak") {
            // Word emits this hint at the start of the FIRST visible run on
            // each new page after layout. Manual `<w:br w:type="page"/>`
            // boundaries also induce a `lastRenderedPageBreak` at the same
            // location after re-render, so counting both would double-count.
            // We count only `lastRenderedPageBreak`.
            page += 1;
            out.breaks.push((out.text.len(), page));
        } else if starts_with(tag, b"<w:t") && !starts_with(tag, b"<w:tbl") {
            // <w:t> (no slash, no children). Body text is between this
            // open tag and the next "</w:t>".
            let body_start = tag_end + 1;
            let body_end = find_closing(bytes, body_start, b"</w:t>");
            if let Some(end) = body_end {
                let raw = std::str::from_utf8(&bytes[body_start..end]).unwrap_or("");
                // Decode the five XML predefined entities Word may write
                // inside `<w:t>`. Critical for acronyms that contain `&`,
                // `<`, `>`, `'`, `"` (e.g. `ATT&CK` is stored as
                // `ATT&amp;CK` in `document.xml`).
                decode_xml_entities_into(raw, &mut out.text);
                i = end;
            } else {
                i = tag_end + 1;
            }
            continue;
        } else if starts_with(tag, b"</w:p>") {
            // Paragraph end — insert a newline so substring searches don't
            // bridge across paragraph boundaries.
            out.text.push('\n');
        }
        i = tag_end + 1;
    }
    Ok(out)
}

/// Decode the five XML predefined entities (`&amp;`, `&lt;`, `&gt;`,
/// `&apos;`, `&quot;`) and the rare numeric-entity form `&#NNN;`.
/// Everything else is appended verbatim.
fn decode_xml_entities_into(src: &str, dst: &mut String) {
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
                    // &#NNN; numeric form
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
            // Lone `&` — copy and continue.
            dst.push('&');
            rest = &tail[1..];
        }
    }
    dst.push_str(rest);
}

fn starts_with(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.len() >= needle.len() && &haystack[..needle.len()] == needle
}

fn memchr(byte: u8, haystack: &[u8], from: usize) -> Option<usize> {
    haystack[from..]
        .iter()
        .position(|&b| b == byte)
        .map(|p| p + from)
}

fn find_closing(haystack: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    let mut i = from;
    while i + needle.len() <= haystack.len() {
        if &haystack[i..i + needle.len()] == needle {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Parse the acronyms table (markdown). Returns the left-column acronyms in
/// document order, plus the raw lines (so we can rewrite the file
/// in-place preserving everything else).
fn parse_acronyms_table(md: &str) -> (Vec<String>, Vec<String>) {
    let lines: Vec<String> = md.lines().map(str::to_owned).collect();
    let mut acros = Vec::new();
    let mut in_body = false;
    let mut header_seen = false;
    for line in &lines {
        let t = line.trim();
        if !t.starts_with('|') {
            in_body = false;
            header_seen = false;
            continue;
        }
        let cells: Vec<&str> = t
            .trim_start_matches('|')
            .trim_end_matches('|')
            .split('|')
            .map(str::trim)
            .collect();
        let is_sep = cells.len() >= 2
            && cells.iter().all(|c| {
                c.chars()
                    .all(|ch| ch == '-' || ch == ':' || ch.is_whitespace())
            });
        if is_sep {
            if header_seen {
                in_body = true;
            }
            continue;
        }
        if !header_seen {
            header_seen = true;
            continue;
        }
        if in_body && !cells.is_empty() {
            let key: String = cells[0]
                .trim_matches(|c: char| c == '*' || c == '`' || c == '_')
                .trim()
                .into();
            if !key.is_empty() {
                acros.push(key);
            }
        }
    }
    (acros, lines)
}

/// v4 page-list extraction for one acronym.
fn pages_for_acronym(text: &PageAwareText, acronym: &str) -> Vec<u32> {
    let mut pages = BTreeSet::new();
    if acronym.is_empty() {
        return Vec::new();
    }
    let needle = acronym.as_bytes();
    let bytes = text.text.as_bytes();
    let mut pos = 0;
    while pos + needle.len() <= bytes.len() {
        if &bytes[pos..pos + needle.len()] == needle {
            // v4 surrounding-char post-filter: reject when neighbour
            // byte is alphanumeric (substring inside a larger word).
            let before = if pos == 0 { 0 } else { bytes[pos - 1] };
            let after = if pos + needle.len() == bytes.len() {
                0
            } else {
                bytes[pos + needle.len()]
            };
            if !is_alnum(before) && !is_alnum(after) {
                pages.insert(text.page_at(pos));
            }
            pos += 1;
        } else {
            pos += 1;
        }
    }
    pages.into_iter().collect()
}

fn is_alnum(b: u8) -> bool {
    b.is_ascii_alphanumeric()
}

/// Rewrite the Pages cell of every body row. Returns the new markdown.
fn rewrite_pages_column(
    lines: &[String],
    pages: &std::collections::BTreeMap<String, Vec<u32>>,
) -> (String, usize) {
    let mut updated = 0usize;
    let mut out = String::with_capacity(lines.iter().map(|l| l.len() + 1).sum());
    let mut in_body = false;
    let mut header_seen = false;
    for line in lines {
        let t = line.trim();
        let mut emitted = false;
        if t.starts_with('|') {
            let cells: Vec<&str> = t
                .trim_start_matches('|')
                .trim_end_matches('|')
                .split('|')
                .map(str::trim)
                .collect();
            let is_sep = cells.len() >= 2
                && cells.iter().all(|c| {
                    c.chars()
                        .all(|ch| ch == '-' || ch == ':' || ch.is_whitespace())
                });
            if is_sep {
                if header_seen {
                    in_body = true;
                }
                out.push_str(line);
                out.push('\n');
                continue;
            }
            if !header_seen {
                header_seen = true;
                out.push_str(line);
                out.push('\n');
                continue;
            }
            if in_body && cells.len() >= 3 {
                let acro: String = cells[0]
                    .trim_matches(|c: char| c == '*' || c == '`' || c == '_')
                    .trim()
                    .into();
                if let Some(ps) = pages.get(&acro) {
                    let pages_cell = if ps.is_empty() {
                        cells[2].to_owned()
                    } else {
                        ps.iter()
                            .map(|p| p.to_string())
                            .collect::<Vec<_>>()
                            .join(",")
                    };
                    let new_line = format!("| {} | {} | {} |", cells[0], cells[1], pages_cell);
                    if new_line != *line {
                        updated += 1;
                    }
                    out.push_str(&new_line);
                    out.push('\n');
                    emitted = true;
                }
            }
        } else {
            in_body = false;
            header_seen = false;
        }
        if !emitted {
            out.push_str(line);
            out.push('\n');
        }
    }
    (out, updated)
}

/// Scan the page-aware text for ALL-CAPS acronym candidates matching
/// `[A-Z]{2,8}(?:-[A-Z0-9]+)?` with the same v4 surrounding-char filter
/// used for known acronyms. Returns each candidate's pages.
///
/// Filters:
///   * 2..=8 leading caps (then optional `-[A-Z0-9]+` tail).
///   * Surrounding bytes must not be alphanumeric (so "AI" in "AIBOM"
///     is still rejected, matching the canonical lookup behaviour).
///   * 3-letter tokens matching the English stop list are skipped.
fn detect_candidates(text: &PageAwareText) -> std::collections::BTreeMap<String, Vec<u32>> {
    let bytes = text.text.as_bytes();
    let mut out: std::collections::BTreeMap<String, BTreeSet<u32>> =
        std::collections::BTreeMap::new();
    let n = bytes.len();
    let mut i = 0usize;
    while i < n {
        let b = bytes[i];
        // Anchor only at a non-alnum / start boundary.
        if !is_ascii_upper(b) {
            i += 1;
            continue;
        }
        let before = if i == 0 { 0 } else { bytes[i - 1] };
        if is_alnum(before) {
            i += 1;
            continue;
        }
        // Walk the leading caps run.
        let start = i;
        let mut j = i;
        while j < n && is_ascii_upper(bytes[j]) {
            j += 1;
        }
        let head_len = j - start;
        if !(2..=8).contains(&head_len) {
            i = j.max(i + 1);
            continue;
        }
        // Optional hyphen-tail of caps or digits, repeatable, e.g.
        // `MAPE-K`, `MITRE-ATT-CK` (rare; only when contiguous CAPS+digits).
        let mut k = j;
        while k < n && bytes[k] == b'-' {
            let tail_start = k + 1;
            let mut t = tail_start;
            while t < n && (is_ascii_upper(bytes[t]) || bytes[t].is_ascii_digit()) {
                t += 1;
            }
            if t == tail_start {
                break;
            }
            k = t;
        }
        // Surrounding-char post-filter on the right side.
        let after = if k == n { 0 } else { bytes[k] };
        if is_alnum(after) {
            i = j.max(i + 1);
            continue;
        }
        // Safe: only ASCII upper/digit/hyphen between start..k.
        let token = std::str::from_utf8(&bytes[start..k])
            .unwrap_or("")
            .to_owned();
        let admit = match token.len() {
            3 => !STOPWORDS_3.contains(&token.as_str()),
            _ => true,
        };
        if admit && !token.is_empty() {
            out.entry(token).or_default().insert(text.page_at(start));
        }
        i = k.max(i + 1);
    }
    out.into_iter()
        .map(|(k, v)| (k, v.into_iter().collect()))
        .collect()
}

fn is_ascii_upper(b: u8) -> bool {
    (b'A'..=b'Z').contains(&b)
}

/// Drop body rows whose acronym maps to an empty page list.
///
/// Preserves every line outside the acronyms table (preamble, footer,
/// blank lines). Header + separator + non-matching rows are emitted
/// verbatim. Returns the new markdown and the number of dropped rows.
fn drop_zero_page_rows(
    lines: &[String],
    pages: &std::collections::BTreeMap<String, Vec<u32>>,
) -> (Vec<String>, usize) {
    let mut out = Vec::with_capacity(lines.len());
    let mut dropped = 0usize;
    let mut in_body = false;
    let mut header_seen = false;
    for line in lines {
        let t = line.trim();
        if !t.starts_with('|') {
            in_body = false;
            header_seen = false;
            out.push(line.clone());
            continue;
        }
        let cells: Vec<&str> = t
            .trim_start_matches('|')
            .trim_end_matches('|')
            .split('|')
            .map(str::trim)
            .collect();
        let is_sep = cells.len() >= 2
            && cells.iter().all(|c| {
                c.chars()
                    .all(|ch| ch == '-' || ch == ':' || ch.is_whitespace())
            });
        if is_sep {
            if header_seen {
                in_body = true;
            }
            out.push(line.clone());
            continue;
        }
        if !header_seen {
            header_seen = true;
            out.push(line.clone());
            continue;
        }
        if in_body && !cells.is_empty() {
            let key: String = cells[0]
                .trim_matches(|c: char| c == '*' || c == '`' || c == '_')
                .trim()
                .into();
            if let Some(ps) = pages.get(&key) {
                if ps.is_empty() {
                    dropped += 1;
                    continue;
                }
            }
        }
        out.push(line.clone());
    }
    (out, dropped)
}

/// Append placeholder rows for newly-detected acronyms to the table.
///
/// Appends at the END of the acronyms table (in alphabetical order),
/// preserving every line after the table verbatim. Returns the new
/// markdown lines and the count of rows added.
fn append_missing_rows(
    lines: &[String],
    new_rows: &std::collections::BTreeMap<String, Vec<u32>>,
) -> (Vec<String>, usize) {
    if new_rows.is_empty() {
        return (lines.to_vec(), 0);
    }
    // Find the last body-row index of the acronyms table.
    let mut table_last_idx: Option<usize> = None;
    let mut in_body = false;
    let mut header_seen = false;
    for (idx, line) in lines.iter().enumerate() {
        let t = line.trim();
        if !t.starts_with('|') {
            in_body = false;
            header_seen = false;
            continue;
        }
        let cells: Vec<&str> = t
            .trim_start_matches('|')
            .trim_end_matches('|')
            .split('|')
            .map(str::trim)
            .collect();
        let is_sep = cells.len() >= 2
            && cells.iter().all(|c| {
                c.chars()
                    .all(|ch| ch == '-' || ch == ':' || ch.is_whitespace())
            });
        if is_sep {
            if header_seen {
                in_body = true;
            }
            continue;
        }
        if !header_seen {
            header_seen = true;
            continue;
        }
        if in_body {
            table_last_idx = Some(idx);
        }
    }
    let mut out: Vec<String> = Vec::with_capacity(lines.len() + new_rows.len());
    let insert_after = match table_last_idx {
        Some(i) => i,
        None => {
            // No body rows — append everything at the tail.
            out.extend(lines.iter().cloned());
            for (acro, ps) in new_rows {
                let cell = ps.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
                out.push(format!("| {acro} | TODO: define | {cell} |"));
            }
            return (out, new_rows.len());
        }
    };
    for (idx, line) in lines.iter().enumerate() {
        out.push(line.clone());
        if idx == insert_after {
            for (acro, ps) in new_rows {
                let cell = if ps.is_empty() {
                    String::new()
                } else {
                    ps.iter().map(u32::to_string).collect::<Vec<_>>().join(",")
                };
                out.push(format!("| {acro} | TODO: define | {cell} |"));
            }
        }
    }
    (out, new_rows.len())
}

fn refresh(
    db_path: &Path,
    project: &str,
    docx: &PathBuf,
    dry_run: bool,
    add_missing: bool,
    drop_zero_page: bool,
    json: bool,
) -> Result<()> {
    if !docx.exists() {
        return Err(anyhow!("docx not found: {}", docx.display()));
    }

    let conn = agentic_core::db::open(db_path)?;

    // Load current acronyms.md from project worktree.
    let blob = worktree::read_at(&conn, project, ACRONYMS_MD)
        .with_context(|| format!("worktree::read_at {ACRONYMS_MD} in project {project}"))?;
    let md = String::from_utf8(blob.content).context("acronyms.md is not UTF-8")?;
    let (acronyms, lines) = parse_acronyms_table(&md);
    if acronyms.is_empty() {
        return Err(anyhow!(
            "no acronyms parsed from {ACRONYMS_MD} — table format unexpected"
        ));
    }

    // Extract page-aware text from rendered docx.
    let page_text = extract_page_aware_text(docx)?;
    let total_pages = page_text.breaks.last().map(|&(_, p)| p).unwrap_or(1);

    // Compute pages for each acronym in the existing table.
    let mut pages_map: std::collections::BTreeMap<String, Vec<u32>> =
        std::collections::BTreeMap::new();
    for a in &acronyms {
        pages_map.insert(a.clone(), pages_for_acronym(&page_text, a));
    }

    // Order: load → drop-zero (if flag) → add-missing (if flag) → write back.
    // Dropping first ensures a re-introduced stale row is kept as a TODO.
    let mut working_lines = lines.clone();
    let mut dropped = 0usize;
    if drop_zero_page {
        let (after, n) = drop_zero_page_rows(&working_lines, &pages_map);
        working_lines = after;
        dropped = n;
    }

    let mut added = 0usize;
    let mut additions: std::collections::BTreeMap<String, Vec<u32>> =
        std::collections::BTreeMap::new();
    if add_missing {
        let candidates = detect_candidates(&page_text);
        let known: std::collections::BTreeSet<String> = pages_map.keys().cloned().collect();
        for (acro, pages) in candidates {
            if known.contains(&acro) {
                continue;
            }
            additions.insert(acro, pages);
        }
        let (after, n) = append_missing_rows(&working_lines, &additions);
        working_lines = after;
        added = n;
        // Reflect placeholder rows in pages_map so the rewrite emits real
        // page numbers (not blanks) for them.
        for (acro, pages) in &additions {
            pages_map.insert(acro.clone(), pages.clone());
        }
    }

    // Rewrite acronyms.md (refreshes pages column for all rows including
    // newly-added placeholder rows).
    let (new_md, updated) = rewrite_pages_column(&working_lines, &pages_map);

    if !dry_run && new_md != md {
        let mut msg =
            String::from("acronyms refresh: Pages column rebuilt from rendered docx (v4)");
        if drop_zero_page {
            msg.push_str(&format!("; dropped={dropped}"));
        }
        if add_missing {
            msg.push_str(&format!("; added={added}"));
        }
        worktree::put_at(
            &conn,
            project,
            ACRONYMS_MD,
            new_md.as_bytes(),
            "text/markdown",
            Some("en"),
            "agentic-acronyms",
            &msg,
        )?;
    }

    if json {
        let summary = serde_json::json!({
            "project": project,
            "docx": docx.display().to_string(),
            "total_pages": total_pages,
            "acronyms_scanned": acronyms.len(),
            "rows_updated": updated,
            "rows_dropped": dropped,
            "rows_added": added,
            "added": additions.iter()
                .map(|(k, v)| (k, v.iter().map(u32::to_string).collect::<Vec<_>>().join(",")))
                .collect::<std::collections::BTreeMap<_, _>>(),
            "dry_run": dry_run,
            "pages": pages_map.iter()
                .map(|(k, v)| (k, v.iter().map(u32::to_string).collect::<Vec<_>>().join(",")))
                .collect::<std::collections::BTreeMap<_, _>>(),
        });
        println!("{summary}");
    } else {
        println!(
            "acronyms refresh: docx={} total_pages={} scanned={} updated_rows={} dropped={} added={} dry_run={}",
            docx.display(),
            total_pages,
            acronyms.len(),
            updated,
            dropped,
            added,
            dry_run
        );
        for (k, v) in &pages_map {
            let cell = if v.is_empty() {
                "(no matches)".to_string()
            } else {
                v.iter().map(u32::to_string).collect::<Vec<_>>().join(",")
            };
            println!("  {k}\t{cell}");
        }
        if !additions.is_empty() {
            println!("added (placeholder definitions):");
            for (k, v) in &additions {
                let cell = v.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
                println!("  {k}\tTODO: define\t{cell}");
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_at_lookups_are_correct() {
        let p = PageAwareText {
            text: "abcdef".into(),
            breaks: vec![(0, 1), (3, 2), (5, 7)],
        };
        assert_eq!(p.page_at(0), 1);
        assert_eq!(p.page_at(1), 1);
        assert_eq!(p.page_at(2), 1);
        assert_eq!(p.page_at(3), 2);
        assert_eq!(p.page_at(4), 2);
        assert_eq!(p.page_at(5), 7);
        assert_eq!(p.page_at(99), 7);
    }

    #[test]
    fn v4_post_filter_admits_hyphenated_compounds() {
        let t = PageAwareText {
            // p1: "AI-First", p2: "AIBOM is the ledger"
            text: "AI-First\nAIBOM is the ledger".into(),
            breaks: vec![(0, 1), (9, 2)],
        };
        let pages = pages_for_acronym(&t, "AI");
        // Should match the "AI-" prefix (page 1) but reject "AI" inside "AIBOM" (page 2).
        assert_eq!(pages, vec![1]);
    }

    #[test]
    fn v4_admits_standalone_with_punctuation_neighbours() {
        let t = PageAwareText {
            text: "We use AI.\nWith \"AI\" quoted.\n(AI) in parens.".into(),
            breaks: vec![(0, 1), (11, 2), (28, 3)],
        };
        let pages = pages_for_acronym(&t, "AI");
        assert_eq!(pages, vec![1, 2, 3]);
    }

    #[test]
    fn v4_rejects_substring_inside_larger_word() {
        let t = PageAwareText {
            text: "Sinaitic but not Sinai".into(),
            breaks: vec![(0, 1)],
        };
        let pages = pages_for_acronym(&t, "AI");
        // "AI" appears inside "Sinaitic" / "Sinai" but case-sensitive
        // match is "AI" — only when the source uses CAPITAL AI. In the
        // test text, "AI" never appears (only "ai"), so no match.
        assert!(pages.is_empty());
    }

    #[test]
    fn parse_acronyms_extracts_left_column() {
        let md = "# title\n\nIntro.\n\n| Acronym | Expansion | Pages |\n|---|---|---|\n| AI | Artificial Intelligence | 1,2 |\n| HITL | Human in the Loop | 3 |\n";
        let (acros, _) = parse_acronyms_table(md);
        assert_eq!(acros, vec!["AI".to_string(), "HITL".to_string()]);
    }

    #[test]
    fn rewrite_replaces_pages_cell() {
        let md = "| Acronym | Expansion | Pages |\n|---|---|---|\n| AI | x | old |\n";
        let lines: Vec<String> = md.lines().map(str::to_owned).collect();
        let mut map = std::collections::BTreeMap::new();
        map.insert("AI".into(), vec![1, 5, 7]);
        let (out, updated) = rewrite_pages_column(&lines, &map);
        assert!(out.contains("| AI | x | 1,5,7 |"));
        assert_eq!(updated, 1);
    }

    #[test]
    fn detect_candidates_finds_allcaps_and_skips_stopwords() {
        // p1: "AIBOM" + a stop-word "THE" + already-known "AI".
        // p2: "HITL" alone; p3: "MAPE-K" hyphenated; p4: lowercase noise.
        let t = PageAwareText {
            text: "AIBOM THE AI is good.\nHITL alone.\nMAPE-K cycle.\nlowercase only".into(),
            breaks: vec![(0, 1), (22, 2), (34, 3), (48, 4)],
        };
        let cands = detect_candidates(&t);
        // Expected: AIBOM, AI, HITL, MAPE-K. Not THE (stopword), not lowercase.
        assert!(
            cands.contains_key("AIBOM"),
            "missing AIBOM: {:?}",
            cands.keys().collect::<Vec<_>>()
        );
        assert!(cands.contains_key("AI"));
        assert!(cands.contains_key("HITL"));
        assert!(cands.contains_key("MAPE-K"));
        assert!(!cands.contains_key("THE"));
    }

    #[test]
    fn detect_candidates_rejects_substring_inside_word() {
        let t = PageAwareText {
            text: "HITLer is not an acronym; HITL is".into(),
            breaks: vec![(0, 1)],
        };
        let cands = detect_candidates(&t);
        // "HITL" in "HITLer" — followed by 'e' (alnum) — rejected.
        // The standalone "HITL" at the end is admitted.
        let pages = cands.get("HITL").expect("HITL standalone");
        assert_eq!(pages, &vec![1]);
    }

    #[test]
    fn drop_zero_page_removes_only_empty_rows() {
        let md = "| Acronym | Expansion | Pages |\n|---|---|---|\n| AI | x | 1,2 |\n| OLD | gone | |\n| HITL | y | 3 |\n";
        let lines: Vec<String> = md.lines().map(str::to_owned).collect();
        let mut map = std::collections::BTreeMap::new();
        map.insert("AI".into(), vec![1, 2]);
        map.insert("OLD".into(), vec![]); // zero pages → drop
        map.insert("HITL".into(), vec![3]);
        let (out, n) = drop_zero_page_rows(&lines, &map);
        assert_eq!(n, 1);
        let joined = out.join("\n");
        assert!(joined.contains("| AI |"));
        assert!(joined.contains("| HITL |"));
        assert!(!joined.contains("| OLD |"));
    }

    #[test]
    fn append_missing_rows_inserts_after_last_body_row() {
        let md = "intro line\n\n| Acronym | Expansion | Pages |\n|---|---|---|\n| AI | x | 1 |\n\nfooter line\n";
        let lines: Vec<String> = md.lines().map(str::to_owned).collect();
        let mut additions = std::collections::BTreeMap::new();
        additions.insert("HITL".to_string(), vec![3, 4]);
        let (out, n) = append_missing_rows(&lines, &additions);
        assert_eq!(n, 1);
        let joined = out.join("\n");
        // The new row sits AFTER the existing AI row and BEFORE the footer.
        let ai = joined.find("| AI |").expect("AI row");
        let hitl = joined.find("| HITL |").expect("HITL row");
        let footer = joined.find("footer line").expect("footer");
        assert!(ai < hitl, "HITL should come after AI");
        assert!(hitl < footer, "HITL should come before the footer");
        assert!(joined.contains("| HITL | TODO: define | 3,4 |"));
    }

    #[test]
    fn combined_drop_then_add_via_helpers() {
        // Simulates the order-of-operations: drop OLD first, then append NEW.
        let md =
            "| Acronym | Expansion | Pages |\n|---|---|---|\n| AI | x | 1 |\n| OLD | gone | |\n";
        let lines: Vec<String> = md.lines().map(str::to_owned).collect();

        let mut pages = std::collections::BTreeMap::new();
        pages.insert("AI".to_string(), vec![1]);
        pages.insert("OLD".to_string(), vec![]);

        let (after_drop, dropped) = drop_zero_page_rows(&lines, &pages);
        assert_eq!(dropped, 1);

        let mut additions = std::collections::BTreeMap::new();
        additions.insert("HITL".to_string(), vec![2]);
        let (after_add, added) = append_missing_rows(&after_drop, &additions);
        assert_eq!(added, 1);

        let joined = after_add.join("\n");
        assert!(joined.contains("| AI |"));
        assert!(joined.contains("| HITL | TODO: define | 2 |"));
        assert!(!joined.contains("| OLD |"));
    }
}
