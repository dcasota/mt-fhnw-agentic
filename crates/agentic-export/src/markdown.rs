//! Markdown adapters.
//!
//! Two output dialects share one pulldown-cmark parser:
//!   * [`to_typst`] — render to Typst markup (headings, emphasis, lists, code).
//!   * [`MdDocxFlow`] — iterate normalised paragraphs for docx-rs to consume.
//!
//! Both are intentionally minimal: image embedding, tables and footnotes are
//! left for a later pass.

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

/// Render markdown to Typst markup.
///
/// Heading `#` → Typst's `=` prefix; `**bold**` → `*text*`; `*em*` → `_text_`;
/// bullet lists, ordered lists, code blocks and inline code map straight over.
#[must_use]
pub fn to_typst(md: &str) -> String {
    let reflowed = structural_reflow(md);
    let md: &str = reflowed.as_ref();
    let parser = Parser::new_ext(md, options());
    let mut out = String::with_capacity(md.len());
    let mut list_stack: Vec<Option<u64>> = Vec::new(); // None = bullet, Some(n) = ordered next
    let mut in_code: Option<String> = None;

    for ev in parser {
        match ev {
            Event::Start(Tag::Heading { level, .. }) => {
                out.push('\n');
                out.push_str(&"=".repeat(heading_depth(level)));
                out.push(' ');
            }
            Event::End(TagEnd::Heading(_)) => out.push('\n'),
            Event::Start(Tag::Paragraph) => out.push('\n'),
            Event::End(TagEnd::Paragraph) => out.push('\n'),
            Event::Start(Tag::Emphasis) => out.push('_'),
            Event::End(TagEnd::Emphasis) => out.push('_'),
            Event::Start(Tag::Strong) => out.push('*'),
            Event::End(TagEnd::Strong) => out.push('*'),
            Event::Start(Tag::List(start)) => {
                list_stack.push(start);
            }
            Event::End(TagEnd::List(_)) => {
                list_stack.pop();
                out.push('\n');
            }
            Event::Start(Tag::Item) => {
                out.push('\n');
                match list_stack.last_mut() {
                    Some(None) | None => out.push_str("- "),
                    Some(Some(n)) => {
                        out.push_str(&format!("+ "));
                        *n += 1;
                    }
                }
            }
            Event::End(TagEnd::Item) => {}
            Event::Start(Tag::CodeBlock(kind)) => {
                let lang = match kind {
                    CodeBlockKind::Indented => String::new(),
                    CodeBlockKind::Fenced(s) => s.into_string(),
                };
                out.push_str("\n```");
                out.push_str(&lang);
                out.push('\n');
                in_code = Some(lang);
            }
            Event::End(TagEnd::CodeBlock) => {
                out.push_str("```\n");
                in_code = None;
            }
            Event::Code(s) => {
                out.push('`');
                out.push_str(&s);
                out.push('`');
            }
            Event::Text(s) => {
                if in_code.is_some() {
                    out.push_str(&s);
                } else {
                    out.push_str(&escape_typst(&s));
                }
            }
            Event::SoftBreak | Event::HardBreak => out.push('\n'),
            Event::Rule => out.push_str("\n#line(length: 100%)\n"),
            // Links, images, tables, footnotes: emit text only for the MVP.
            Event::Start(Tag::Link { .. }) | Event::End(TagEnd::Link) => {}
            Event::Start(Tag::Image { .. }) | Event::End(TagEnd::Image) => {}
            _ => {}
        }
    }
    out
}

fn heading_depth(level: HeadingLevel) -> usize {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// Escape characters Typst treats as markup: `*`, `_`, `` ` ``, `=`, `#`,
/// `<`, `>`, `\`.
fn escape_typst(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '*' | '_' | '`' | '=' | '#' | '<' | '>' | '\\' | '$' | '@' | '~' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

/// Flat docx-rs-friendly representation of a markdown document.
///
/// Each variant maps cleanly to a single docx paragraph or run. The renderer
/// in [`crate::docx`] consumes this stream in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocxBlock {
    Heading {
        level: u8,
        text: String,
    },
    Paragraph(Vec<DocxRun>),
    BulletItem(Vec<DocxRun>),
    OrderedItem {
        number: u64,
        runs: Vec<DocxRun>,
    },
    CodeBlock {
        lang: String,
        body: String,
    },
    HorizontalRule,
    Table {
        header: Vec<String>,
        rows: Vec<Vec<String>>,
        /// Optional caption (bookkit "Table N." caption-above), populated by the
        /// renderer from a preceding `Table:`-prefixed paragraph.
        caption: Option<String>,
    },
    Image {
        path: String,
        caption: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocxRun {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    /// External URL when this run is part of a markdown link.
    pub link: Option<String>,
}

impl DocxRun {
    fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            bold: false,
            italic: false,
            code: false,
            link: None,
        }
    }
}

/// Split a text run on bare/plain-text URLs (Round-D AI-Norms parity,
/// 2026-06-03). Returns a sequence of `(text, link)` segments: ordinary text
/// gets `None`, each detected URL gets `Some(url)` so the run carrying it can
/// be registered as a clickable hyperlink by `add_runs` (and therefore reach
/// the per-chapter Sources & QR-codes box).
///
/// A bare URL is a contiguous run starting with `http://` or `https://`, ending
/// at the first ASCII whitespace OR at one of the common trailing punctuation
/// characters `,;:.!?` when followed by whitespace / end-of-string.
///
/// Trailing `)` is handled by the GFM-autolink balanced-paren rule: it is
/// preserved when the URL contains balanced parens (e.g. Wikipedia titles like
/// `…/Foo_(disambiguation)`) and stripped when unbalanced (more `)` than `(`),
/// where the closing paren belongs to the surrounding prose — e.g. URLs sitting
/// inside a parenthetical clause `(see https://example.org/x)`. Without this,
/// the rendered hyperlink and its end-of-chapter QR code both encode the stray
/// `)` and fail to resolve.
pub fn split_bare_urls(s: &str) -> Vec<(String, Option<String>)> {
    let mut out: Vec<(String, Option<String>)> = Vec::new();
    let mut rest = s;
    while !rest.is_empty() {
        let Some(pos) = find_url_start(rest) else {
            out.push((rest.to_string(), None));
            break;
        };
        if pos > 0 {
            out.push((rest[..pos].to_string(), None));
        }
        let after = &rest[pos..];
        let end = url_end(after);
        let url = after[..end].to_string();
        out.push((url.clone(), Some(url)));
        rest = &after[end..];
    }
    out
}

/// Find the byte offset of the next `http://` or `https://` in `s`, or None.
/// The match must start at a word boundary (string start or non-URL-char
/// preceding) so we don't split mid-token like `foo:https://...`.
fn find_url_start(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if (bytes[i] == b'h') && (s[i..].starts_with("https://") || s[i..].starts_with("http://")) {
            let prev_ok = i == 0
                || matches!(
                    bytes[i - 1],
                    b' ' | b'\t' | b'\n' | b'(' | b'[' | b'<' | b'"' | b'\''
                );
            if prev_ok {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

/// Find where a URL ends inside `s` (which is guaranteed to begin with the
/// scheme). Stops at the first whitespace or sentinel character, then strips
/// any trailing sentence-ending punctuation `,;:.!?` so prose punctuation
/// adjacent to a URL is not absorbed into it.
fn url_end(s: &str) -> usize {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if matches!(b, b' ' | b'\t' | b'\n' | b'\r' | b'<' | b'>' | b'"' | b'\'') {
            break;
        }
        i += 1;
    }
    // Repeatedly strip trailing characters that belong to the surrounding
    // prose, not the URL:
    //   1. sentence-end punctuation `,;:.!?` — never legal at URL end.
    //   2. a closing `)` IFF the URL substring [0..i) holds more `)` than `(`
    //      (GFM autolink balanced-paren rule: preserve Wikipedia-style
    //      `…/Foo_(disambiguation)` but strip the prose `)` that closes a
    //      parenthetical like `(available at https://example.org/x)`).
    // Loop because the two rules interact — a URL can end with `).`, where
    // the `.` must be stripped first before the balanced-paren check sees `)`.
    loop {
        let before = i;
        while i > 0 && matches!(bytes[i - 1], b',' | b';' | b':' | b'.' | b'!' | b'?') {
            i -= 1;
        }
        if i > 0 && bytes[i - 1] == b')' {
            let opens = bytes[..i].iter().filter(|&&c| c == b'(').count();
            let closes = bytes[..i].iter().filter(|&&c| c == b')').count();
            if closes > opens {
                i -= 1;
            }
        }
        if i == before {
            break;
        }
    }
    i
}

/// Parse markdown into a flat `Vec<DocxBlock>`.
#[must_use]
pub fn to_docx_blocks(md: &str) -> Vec<DocxBlock> {
    let reflowed = structural_reflow(md);
    let md: &str = reflowed.as_ref();
    let parser = Parser::new_ext(md, options());
    let mut blocks: Vec<DocxBlock> = Vec::new();
    let mut current_runs: Vec<DocxRun> = Vec::new();
    let mut style = RunStyle::default();
    let mut state = ParseState::Top;
    let mut list_stack: Vec<bool> = Vec::new(); // true = ordered
    let mut ordered_counters: Vec<u64> = Vec::new(); // next ordinal per ordered list
    let mut cur_ordinal: u64 = 0; // ordinal of the item currently open
    let mut code_lang = String::new();
    let mut code_body = String::new();
    let mut tbl_header: Vec<String> = Vec::new();
    let mut tbl_rows: Vec<Vec<String>> = Vec::new();
    let mut cur_row: Vec<String> = Vec::new();
    let mut cur_cell = String::new();
    let mut in_table = false;
    let mut in_head = false;
    let mut img: Option<(String, String)> = None; // (url, alt)
    let mut cur_link: Option<String> = None; // active hyperlink URL

    for ev in parser {
        match ev {
            Event::Start(Tag::Heading { level, .. }) => {
                flush(&mut blocks, &mut current_runs, &state, &mut list_stack);
                state = ParseState::Heading(heading_depth(level) as u8);
            }
            Event::End(TagEnd::Heading(_)) => {
                let text = current_runs
                    .iter()
                    .map(|r| r.text.as_str())
                    .collect::<String>();
                let level = match state {
                    ParseState::Heading(l) => l,
                    _ => 1,
                };
                blocks.push(DocxBlock::Heading { level, text });
                current_runs.clear();
                state = ParseState::Top;
            }
            Event::Start(Tag::Paragraph) => {
                state = ParseState::Paragraph;
            }
            Event::End(TagEnd::Paragraph) => {
                let runs = std::mem::take(&mut current_runs);
                if !runs.is_empty() {
                    if list_stack.last().copied() == Some(true) {
                        blocks.push(DocxBlock::OrderedItem {
                            number: cur_ordinal,
                            runs,
                        });
                    } else if list_stack.last().copied() == Some(false) {
                        blocks.push(DocxBlock::BulletItem(runs));
                    } else {
                        blocks.push(DocxBlock::Paragraph(runs));
                    }
                }
                state = ParseState::Top;
            }
            Event::Start(Tag::List(start)) => {
                list_stack.push(start.is_some());
                // `start` is the first ordinal (usually 1); track the next value.
                ordered_counters.push(start.unwrap_or(1));
            }
            Event::End(TagEnd::List(_)) => {
                list_stack.pop();
                ordered_counters.pop();
            }
            Event::Start(Tag::Item) => {
                // Assign + advance this item's ordinal if it sits in an ordered list.
                if list_stack.last().copied() == Some(true) {
                    if let Some(n) = ordered_counters.last_mut() {
                        cur_ordinal = *n;
                        *n += 1;
                    }
                }
                state = ParseState::Item;
            }
            Event::End(TagEnd::Item) => {
                // Paragraph End will handle flushing; if the item had no
                // explicit paragraph (a bare item), flush here.
                if !current_runs.is_empty() {
                    let runs = std::mem::take(&mut current_runs);
                    if list_stack.last().copied() == Some(true) {
                        blocks.push(DocxBlock::OrderedItem {
                            number: cur_ordinal,
                            runs,
                        });
                    } else {
                        blocks.push(DocxBlock::BulletItem(runs));
                    }
                }
                state = ParseState::Top;
            }
            Event::Start(Tag::Emphasis) => style.italic = true,
            Event::End(TagEnd::Emphasis) => style.italic = false,
            Event::Start(Tag::Strong) => style.bold = true,
            Event::End(TagEnd::Strong) => style.bold = false,
            Event::Start(Tag::CodeBlock(kind)) => {
                code_lang = match kind {
                    CodeBlockKind::Indented => String::new(),
                    CodeBlockKind::Fenced(s) => s.into_string(),
                };
                code_body.clear();
                state = ParseState::Code;
            }
            Event::End(TagEnd::CodeBlock) => {
                blocks.push(DocxBlock::CodeBlock {
                    lang: std::mem::take(&mut code_lang),
                    body: std::mem::take(&mut code_body),
                });
                state = ParseState::Top;
            }
            Event::Code(s) => {
                if in_table {
                    cur_cell.push_str(&s);
                } else if img.is_some() {
                    if let Some((_, alt)) = img.as_mut() {
                        alt.push_str(&s);
                    }
                } else {
                    current_runs.push(DocxRun {
                        text: s.to_string(),
                        code: true,
                        ..DocxRun::plain("")
                    });
                }
            }
            Event::Text(s) => {
                if in_table {
                    cur_cell.push_str(&s);
                } else if let Some((_, alt)) = img.as_mut() {
                    alt.push_str(&s);
                } else if matches!(state, ParseState::Code) {
                    code_body.push_str(&s);
                } else if cur_link.is_some() {
                    // Inside an explicit markdown link — keep as-is.
                    current_runs.push(DocxRun {
                        text: s.to_string(),
                        bold: style.bold,
                        italic: style.italic,
                        code: false,
                        link: cur_link.clone(),
                    });
                } else {
                    // Round-D AI-Norms parity (2026-06-03): split bare/plain-text
                    // URLs out of the run so they reach `add_runs` as `link =
                    // Some(url)` and get registered in the chapter Sources & QR
                    // box. Without this the reference book's autodetected bare
                    // URLs (~20 per AI-Norms volume) miss the per-chapter QR
                    // emission and we under-shoot the FIGURE_COUNT_PARITY gate.
                    for (text, url) in split_bare_urls(&s) {
                        current_runs.push(DocxRun {
                            text,
                            bold: style.bold,
                            italic: style.italic,
                            code: false,
                            link: url,
                        });
                    }
                }
            }
            Event::Start(Tag::Link { dest_url, .. }) => {
                cur_link = Some(dest_url.into_string());
            }
            Event::End(TagEnd::Link) => {
                cur_link = None;
            }
            Event::SoftBreak | Event::HardBreak => {
                if in_table {
                    cur_cell.push(' ');
                } else if !img.is_some() {
                    current_runs.push(DocxRun::plain(" "));
                }
            }
            Event::Rule => blocks.push(DocxBlock::HorizontalRule),
            // Tables
            Event::Start(Tag::Table(_)) => {
                in_table = true;
                tbl_header.clear();
                tbl_rows.clear();
            }
            Event::End(TagEnd::Table) => {
                in_table = false;
                blocks.push(DocxBlock::Table {
                    header: std::mem::take(&mut tbl_header),
                    rows: std::mem::take(&mut tbl_rows),
                    caption: None,
                });
            }
            Event::Start(Tag::TableHead) => {
                in_head = true;
                cur_row.clear();
            }
            Event::End(TagEnd::TableHead) => {
                in_head = false;
                tbl_header = std::mem::take(&mut cur_row);
            }
            Event::Start(Tag::TableRow) => cur_row.clear(),
            Event::End(TagEnd::TableRow) => {
                if !in_head {
                    tbl_rows.push(std::mem::take(&mut cur_row));
                }
            }
            Event::Start(Tag::TableCell) => cur_cell.clear(),
            Event::End(TagEnd::TableCell) => {
                cur_row.push(std::mem::take(&mut cur_cell).trim().to_string())
            }
            // Images
            Event::Start(Tag::Image { dest_url, .. }) => {
                img = Some((dest_url.into_string(), String::new()));
            }
            Event::End(TagEnd::Image) => {
                if let Some((url, alt)) = img.take() {
                    blocks.push(DocxBlock::Image {
                        path: url,
                        caption: alt,
                    });
                }
            }
            _ => {}
        }
    }
    flush(&mut blocks, &mut current_runs, &state, &mut list_stack);
    blocks
}

#[derive(Clone, Copy, Default)]
struct RunStyle {
    bold: bool,
    italic: bool,
}

#[derive(Clone, Copy)]
enum ParseState {
    Top,
    Paragraph,
    Heading(u8),
    Item,
    Code,
}

fn flush(
    blocks: &mut Vec<DocxBlock>,
    runs: &mut Vec<DocxRun>,
    state: &ParseState,
    _list_stack: &mut Vec<bool>,
) {
    if runs.is_empty() {
        return;
    }
    let taken = std::mem::take(runs);
    match state {
        ParseState::Heading(level) => {
            let text = taken.iter().map(|r| r.text.as_str()).collect::<String>();
            blocks.push(DocxBlock::Heading {
                level: *level,
                text,
            });
        }
        _ => blocks.push(DocxBlock::Paragraph(taken)),
    }
}

/// Restore paragraph / heading / list / table breaks on markdown sources
/// that arrived as flattened single-line dumps (newlines collapsed into
/// 3+ space runs by an upstream pipeline step). The Jun-6 cycle-close
/// regression (see ADR-0063) recovered campaign + student-notes sources
/// from an older anchor that had this shape; without this pre-pass,
/// pulldown-cmark treats the dump as one continuous paragraph and the
/// rendered docx has no headings, no tables, and ~7 % of the original
/// paragraph count.
///
/// The reflow is heuristic and code-fence aware:
///   * runs only when LF density < 10 LF/kB (clean multi-line content
///     is left untouched — borne out by the corpus baseline of 22 LF/kB
///     mean and 6.14 LF/kB minimum for list-heavy inbox files)
///   * inside fenced ``` code blocks, the original bytes pass through
///     unchanged (multi-space content is legitimate in code)
///   * outside code, the following patterns get a `\n\n` prefix injected:
///       - `<≥3 spaces><# >` to `<≥3 spaces><###### >`  → heading
///       - `<≥3 spaces><\*[^*\n]{1,160}\*>` followed by `<≥3 spaces>`
///         → italic-anchor section marker (used by FRD / campaign docs
///         for sub-section headings styled in italic)
///       - `<≥3 spaces><- >` / `<≥3 spaces><\* >` → bullet list item
///       - `<≥3 spaces><\d+\. >` → ordered list item
///       - `<≥3 spaces><\| >` only when followed by another `|` later in
///         the same span → pipe-table row
///
/// The output borrows `md` when no reflow is needed (the common case).
pub(crate) fn structural_reflow(md: &str) -> std::borrow::Cow<'_, str> {
    let bytes = md.len() as f64;
    if bytes < 200.0 {
        return std::borrow::Cow::Borrowed(md);
    }
    let lf = md.bytes().filter(|&b| b == b'\n').count() as f64;
    let lf_per_kb = 1000.0 * lf / bytes;
    if lf_per_kb >= 10.0 {
        return std::borrow::Cow::Borrowed(md);
    }
    // Single-line dump confirmed; reflow.
    let mut out = String::with_capacity(md.len() + md.len() / 8);
    let mut in_fence = false;
    let mut fence_marker: &str = "";
    for line in md.split_inclusive('\n') {
        // Code-fence detection on a per-line basis: a fence opener / closer
        // is the trimmed line starting with ``` or ~~~. In a single-line
        // dump there are very few lines, so fences in the actual content
        // are rare — but we still respect them defensively.
        let trimmed = line.trim_start();
        if !in_fence && (trimmed.starts_with("```") || trimmed.starts_with("~~~")) {
            in_fence = true;
            fence_marker = if trimmed.starts_with("```") {
                "```"
            } else {
                "~~~"
            };
            out.push_str(line);
            continue;
        }
        if in_fence {
            out.push_str(line);
            if trimmed.starts_with(fence_marker) {
                in_fence = false;
                fence_marker = "";
            }
            continue;
        }
        reflow_line_into(&mut out, line);
    }
    std::borrow::Cow::Owned(out)
}

/// Walk one source line, looking for `<≥3 spaces><structural-marker>`
/// runs and replacing the leading space run with `\n\n`. All other
/// whitespace runs (1, 2 spaces) and non-space content pass through
/// unchanged. The first `<structural-marker>` *at line start* is
/// preserved (already a paragraph boundary).
fn reflow_line_into(out: &mut String, line: &str) {
    let bytes = line.as_bytes();
    let n = bytes.len();
    let mut i = 0;
    while i < n {
        // Consume the next space run (may be 0 long).
        let space_start = i;
        while i < n && bytes[i] == b' ' {
            i += 1;
        }
        let sp = i - space_start;
        if sp >= 3 && i < n && is_structural_marker(&line[i..]) {
            // Replace ≥3-space run preceding a structural marker with
            // `\n\n` — but only mid-line; if we're still at the very
            // start of the line, the line itself already provides a
            // paragraph boundary so just keep the original spaces.
            if space_start > 0 {
                out.push_str("\n\n");
            } else {
                out.push_str(&line[space_start..i]);
            }
        } else if sp > 0 {
            // Plain space run (or ≥3 spaces not followed by a marker) —
            // emit verbatim.
            out.push_str(&line[space_start..i]);
        }
        // Now consume up to the next space run; emit non-space content
        // (and any trailing CR/LF) as one slice.
        let nonspace_start = i;
        while i < n && bytes[i] != b' ' {
            i += 1;
        }
        if i > nonspace_start {
            out.push_str(&line[nonspace_start..i]);
        }
    }
}

/// True if `s` starts with a markdown structural marker that should
/// open a new block when preceded by a `<≥3 spaces>` run inside a
/// flattened single-line dump.
fn is_structural_marker(s: &str) -> bool {
    let b = s.as_bytes();
    if b.is_empty() {
        return false;
    }
    // ATX heading: # … ###### followed by a space.
    if b[0] == b'#' {
        let mut k = 0;
        while k < b.len() && k < 6 && b[k] == b'#' {
            k += 1;
        }
        if k >= 1 && k <= 6 && b.get(k) == Some(&b' ') {
            return true;
        }
    }
    // Bullet list: - or * followed by space (asterisk handled cautiously —
    // bare `*` opens italic, but `* ` at a paragraph boundary is a list).
    if (b[0] == b'-' || b[0] == b'+') && b.get(1) == Some(&b' ') {
        return true;
    }
    if b[0] == b'*' && b.get(1) == Some(&b' ') {
        return true;
    }
    // Ordered list: <digits>. or <digits>) followed by a space.
    let mut k = 0;
    while k < b.len() && b[k].is_ascii_digit() {
        k += 1;
    }
    if k >= 1 && k < b.len() && (b[k] == b'.' || b[k] == b')') && b.get(k + 1) == Some(&b' ') {
        return true;
    }
    // Pipe-table row: leading `|` AND another `|` somewhere later in the
    // same line span. Restrict to the first 200 chars to keep the scan
    // cheap.
    // ADR-0064 iter43 (2026-07-05) UTF-8 safety fix — same class of bug
    // as line 707 below: walk back to the nearest char boundary before
    // slicing when the raw byte cap lands inside a multi-byte char.
    let mut pipe_end = s.len().min(200);
    while pipe_end > 1 && !s.is_char_boundary(pipe_end) {
        pipe_end -= 1;
    }
    if b[0] == b'|' && s[1..pipe_end].contains('|') {
        return true;
    }
    // Italic-anchor section marker: `*<word>...<word>*` followed by a
    // run of ≥3 spaces (the FRD / campaign style for italic
    // sub-headings). Tolerant of CRLF.
    if b[0] == b'*' && b.len() >= 3 && b[1] != b' ' && b[1] != b'*' {
        // Look for closing `*` within 160 chars.
        // ADR-0064 iter43 (2026-07-05) UTF-8 safety fix: the previous
        // `s[1..s.len().min(161)]` panics when byte 161 falls inside a
        // multi-byte char like `§` (bytes 160..162 in the failing
        // PT-C08-9 sample). Walk back to the nearest char boundary
        // before slicing.
        let mut end = s.len().min(161);
        while end > 1 && !s.is_char_boundary(end) {
            end -= 1;
        }
        let close = s[1..end].find('*');
        if let Some(c) = close {
            let after = c + 2;
            if after < s.len() && b.get(after) == Some(&b' ') {
                return true;
            }
        }
    }
    false
}

fn options() -> Options {
    let mut o = Options::empty();
    o.insert(Options::ENABLE_TABLES);
    o.insert(Options::ENABLE_STRIKETHROUGH);
    o.insert(Options::ENABLE_TASKLISTS);
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    // ====================================================================
    // ADR-0063 structural-reflow tests (#410, 2026-06-08).
    // Lock the behaviour of the pre-parse pass that restores paragraph
    // breaks on flattened single-line markdown dumps (the Jun-6 cycle-
    // close regression).
    // ====================================================================

    #[test]
    fn reflow_borrows_clean_multi_line_input() {
        // Above the 10 LF/kB threshold → no reflow → Cow::Borrowed.
        let clean =
            "# Title\n\nBody paragraph one.\n\nBody paragraph two.\n\n## Sub\n\nMore body.\n";
        let out = structural_reflow(clean);
        assert!(matches!(out, std::borrow::Cow::Borrowed(_)));
        assert_eq!(out.as_ref(), clean);
    }

    #[test]
    fn reflow_short_input_is_borrowed() {
        // Inputs under 200 B are always borrowed (not worth the work).
        let short = "# Tiny doc with a heading and a flat single line of body text content xx";
        let out = structural_reflow(short);
        assert!(matches!(out, std::borrow::Cow::Borrowed(_)));
    }

    #[test]
    fn reflow_restores_heading_after_three_spaces() {
        // Build a flattened dump > 200 B with LF density << 10 LF/kB.
        let prose = "Body paragraph one with a lot of content to make sure we are well above the 200 byte minimum threshold for reflow to even attempt to fire on this input. ".repeat(3);
        let flat = format!("# Title    {}   ## Sub    {}", prose, prose);
        let out = structural_reflow(&flat);
        assert!(matches!(out, std::borrow::Cow::Owned(_)), "should reflow");
        let s: &str = out.as_ref();
        // ## Sub must now be preceded by a blank line so pulldown-cmark
        // treats it as a heading, not as a `## Sub` literal mid-paragraph.
        assert!(
            s.contains("\n\n## Sub"),
            "expected `\\n\\n## Sub` in reflowed output; got: {s}"
        );
    }

    #[test]
    fn reflow_restores_bullet_list_after_three_spaces() {
        let prose = "Body content to push us past the size threshold for reflow. ".repeat(8);
        let flat = format!(
            "# Title    {}   - first item   - second item   - third item",
            prose
        );
        let out = structural_reflow(&flat);
        let s: &str = out.as_ref();
        assert!(s.contains("\n\n- first item"));
        assert!(s.contains("\n\n- second item"));
        assert!(s.contains("\n\n- third item"));
    }

    #[test]
    fn reflow_restores_pipe_table_after_three_spaces() {
        let prose = "Body content to push us past the threshold. ".repeat(8);
        let flat = format!(
            "# Title    {}   | Col A | Col B |   | --- | --- |   | 1 | 2 |",
            prose
        );
        let out = structural_reflow(&flat);
        let s: &str = out.as_ref();
        assert!(s.contains("\n\n| Col A | Col B |"));
        assert!(s.contains("\n\n| --- | --- |"));
    }

    #[test]
    fn reflow_restores_ordered_list_after_three_spaces() {
        let prose = "Body content padding past the byte threshold. ".repeat(8);
        let flat = format!("# Title    {}   1. first   2. second   3. third", prose);
        let out = structural_reflow(&flat);
        let s: &str = out.as_ref();
        assert!(s.contains("\n\n1. first"));
        assert!(s.contains("\n\n3. third"));
    }

    #[test]
    fn reflow_preserves_content_inside_code_fence() {
        // A fenced block with 4-space-indented content INSIDE it must
        // survive unchanged. We need the fence on its own line for the
        // detector — that's the realistic case for clean intermediate
        // documents.
        let prose = "Body content padding for size threshold. ".repeat(8);
        let flat = format!(
            "# Title    {}\n```\n    indented code    # not a heading\n```\nAfter the fence.",
            prose
        );
        let out = structural_reflow(&flat);
        let s: &str = out.as_ref();
        // Heading inside code fence must NOT have been promoted.
        assert!(
            s.contains("    indented code    # not a heading"),
            "code-block content must pass through unchanged; got: {s}"
        );
    }

    #[test]
    fn reflow_does_not_break_blob_with_existing_headings_at_line_starts() {
        // Mixed shape: real newlines AND some inline-space-prefixed
        // markers. The real headings (at line start) must NOT get an
        // extra `\n\n`; only the in-line ones do.
        let body = "Paragraph A. ".repeat(20);
        let flat = format!("# Top heading\n{}   ## Inline sub heading", body);
        let lf_per_kb =
            1000.0 * (flat.bytes().filter(|&b| b == b'\n').count() as f64) / (flat.len() as f64);
        assert!(
            lf_per_kb < 10.0,
            "constructed input should still be below threshold to trigger reflow; got {lf_per_kb} LF/kB"
        );
        let out = structural_reflow(&flat);
        let s: &str = out.as_ref();
        assert!(s.starts_with("# Top heading\n"));
        assert!(s.contains("\n\n## Inline sub heading"));
    }

    #[test]
    fn to_docx_blocks_recovers_headings_from_flat_dump() {
        // End-to-end: feed a flattened campaign-style dump through the
        // public docx-blocks entry point and confirm headings are
        // recognised. Pre-reflow, this dump would have produced a
        // single big paragraph and ZERO headings.
        let prose = "Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. ".repeat(4);
        let flat = format!(
            "# Campaign 01: Autonomous CVE Self-Patch   {}   ## Goal   {}   ## Plan   {}",
            prose, prose, prose
        );
        let blocks = to_docx_blocks(&flat);
        let heading_count = blocks
            .iter()
            .filter(|b| matches!(b, DocxBlock::Heading { .. }))
            .count();
        assert!(
            heading_count >= 3,
            "expected ≥3 headings recovered from flat dump; got {heading_count} (blocks: {})",
            blocks.len()
        );
    }

    #[test]
    fn heading_maps_to_typst_equal_signs() {
        let t = to_typst("# Title\n\n## Sub\n");
        assert!(t.contains("\n= Title"));
        assert!(t.contains("\n== Sub"));
    }

    #[test]
    fn bold_and_italic_round_trip_to_typst() {
        let t = to_typst("a **strong** and *em* word\n");
        assert!(t.contains("*strong*"));
        assert!(t.contains("_em_"));
    }

    #[test]
    fn typst_escapes_special_chars() {
        let t = to_typst("price is $5 and # is hash\n");
        assert!(t.contains("\\$5"));
        assert!(t.contains("\\#"));
    }

    #[test]
    fn docx_blocks_extract_heading_and_paragraph() {
        let blocks = to_docx_blocks("# Title\n\nBody text.\n");
        assert!(
            blocks
                .iter()
                .any(|b| matches!(b, DocxBlock::Heading { level: 1, text } if text == "Title"))
        );
        assert!(
            blocks
                .iter()
                .any(|b| matches!(b, DocxBlock::Paragraph(runs) if !runs.is_empty()))
        );
    }

    #[test]
    fn docx_blocks_capture_bold_and_italic() {
        let blocks = to_docx_blocks("a **strong** and *em* word\n");
        let runs: Vec<&DocxRun> = blocks
            .iter()
            .filter_map(|b| {
                if let DocxBlock::Paragraph(r) = b {
                    Some(r)
                } else {
                    None
                }
            })
            .flatten()
            .collect();
        assert!(runs.iter().any(|r| r.bold && r.text == "strong"));
        assert!(runs.iter().any(|r| r.italic && r.text == "em"));
    }

    #[test]
    fn docx_blocks_capture_code_block() {
        let blocks = to_docx_blocks("```rust\nfn main() {}\n```\n");
        assert!(
            blocks
                .iter()
                .any(|b| matches!(b, DocxBlock::CodeBlock { lang, body } if lang == "rust" && body.contains("fn main")))
        );
    }

    /// Round-D AI-Norms parity (2026-06-03): a bare-text URL in a paragraph
    /// must surface as a run with `link = Some(url)` so `add_runs` registers
    /// it for the chapter Sources & QR-codes box. Without this the
    /// FIGURE_COUNT_PARITY gate undershoots the reference by ~20 QR codes
    /// (the AI-Norms reference book autodetected bare URLs from the legacy
    /// bookkit `chapters_src/*.txt` sources).
    #[test]
    fn bare_url_in_paragraph_becomes_linked_run() {
        let blocks = to_docx_blocks("See https://example.org/page for details.\n");
        let runs: Vec<&DocxRun> = blocks
            .iter()
            .filter_map(|b| {
                if let DocxBlock::Paragraph(r) = b {
                    Some(r)
                } else {
                    None
                }
            })
            .flatten()
            .collect();
        assert!(
            runs.iter()
                .any(|r| r.link.as_deref() == Some("https://example.org/page")
                    && r.text == "https://example.org/page"),
            "no linked-URL run found in: {runs:?}",
        );
    }

    #[test]
    fn split_bare_urls_keeps_prose_and_strips_trailing_punctuation() {
        let segs = split_bare_urls("see https://a.example/x, and https://b.example.");
        let urls: Vec<_> = segs.iter().filter_map(|(_, u)| u.clone()).collect();
        assert_eq!(
            urls,
            vec![
                "https://a.example/x".to_string(),
                "https://b.example".to_string()
            ],
            "unexpected URL split: {segs:?}",
        );
    }

    #[test]
    fn split_bare_urls_strips_unbalanced_trailing_paren() {
        // Photon OS compliance declaration shape: URL sitting inside a
        // parenthetical clause. The trailing `)` belongs to the prose, not
        // the URL — leaving it in breaks both the clickable hyperlink and
        // the end-of-chapter QR code.
        let segs = split_bare_urls(
            "available at https://docs.broadcom.com/doc/end-user-agreement-english), including",
        );
        let urls: Vec<_> = segs.iter().filter_map(|(_, u)| u.clone()).collect();
        assert_eq!(
            urls,
            vec!["https://docs.broadcom.com/doc/end-user-agreement-english".to_string()],
            "expected trailing `)` stripped: {segs:?}",
        );
    }

    #[test]
    fn split_bare_urls_strips_paren_after_semicolon() {
        // Appendix-B shape: URL followed by `);` — the `;` strip must fire
        // first so the balanced-paren check then sees the unbalanced `)`.
        let segs = split_bare_urls(
            "(https://github.com/in-toto/attestation/blob/main/spec/v1/statement.md); rest",
        );
        let urls: Vec<_> = segs.iter().filter_map(|(_, u)| u.clone()).collect();
        assert_eq!(
            urls,
            vec![
                "https://github.com/in-toto/attestation/blob/main/spec/v1/statement.md".to_string()
            ],
            "expected `);` fully stripped: {segs:?}",
        );
    }

    #[test]
    fn split_bare_urls_keeps_balanced_internal_parens() {
        // Wikipedia-style URL with balanced parens — the closing `)` is
        // legitimately part of the URL and must NOT be stripped. Counter-
        // example to the unbalanced-paren strip rule above.
        let segs =
            split_bare_urls("see https://en.wikipedia.org/wiki/Foo_(disambiguation) for details");
        let urls: Vec<_> = segs.iter().filter_map(|(_, u)| u.clone()).collect();
        assert_eq!(
            urls,
            vec!["https://en.wikipedia.org/wiki/Foo_(disambiguation)".to_string()],
            "expected balanced `(...)` preserved: {segs:?}",
        );
    }

    #[test]
    fn split_bare_urls_ignores_non_urls() {
        let segs = split_bare_urls("plain text only — no URL here.");
        assert_eq!(segs.len(), 1);
        assert!(segs[0].1.is_none());
    }

    #[test]
    fn explicit_markdown_link_is_not_double_processed() {
        // `[label](url)` should still be a single linked run, NOT also
        // re-split by the bare-URL heuristic.
        let blocks = to_docx_blocks("See [the spec](https://example.org) here.\n");
        let runs: Vec<&DocxRun> = blocks
            .iter()
            .filter_map(|b| {
                if let DocxBlock::Paragraph(r) = b {
                    Some(r)
                } else {
                    None
                }
            })
            .flatten()
            .collect();
        let linked: Vec<&&DocxRun> = runs
            .iter()
            .filter(|r| r.link.as_deref() == Some("https://example.org"))
            .collect();
        assert_eq!(
            linked.len(),
            1,
            "expected exactly one linked run, got: {runs:?}"
        );
        assert_eq!(linked[0].text, "the spec");
    }
}
