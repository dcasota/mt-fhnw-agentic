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
    OrderedItem(Vec<DocxRun>),
    CodeBlock {
        lang: String,
        body: String,
    },
    HorizontalRule,
    Table {
        header: Vec<String>,
        rows: Vec<Vec<String>>,
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
}

impl DocxRun {
    fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            bold: false,
            italic: false,
            code: false,
        }
    }
}

/// Parse markdown into a flat `Vec<DocxBlock>`.
#[must_use]
pub fn to_docx_blocks(md: &str) -> Vec<DocxBlock> {
    let parser = Parser::new_ext(md, options());
    let mut blocks: Vec<DocxBlock> = Vec::new();
    let mut current_runs: Vec<DocxRun> = Vec::new();
    let mut style = RunStyle::default();
    let mut state = ParseState::Top;
    let mut list_stack: Vec<bool> = Vec::new(); // true = ordered
    let mut code_lang = String::new();
    let mut code_body = String::new();
    let mut tbl_header: Vec<String> = Vec::new();
    let mut tbl_rows: Vec<Vec<String>> = Vec::new();
    let mut cur_row: Vec<String> = Vec::new();
    let mut cur_cell = String::new();
    let mut in_table = false;
    let mut in_head = false;
    let mut img: Option<(String, String)> = None; // (url, alt)

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
                        blocks.push(DocxBlock::OrderedItem(runs));
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
            }
            Event::End(TagEnd::List(_)) => {
                list_stack.pop();
            }
            Event::Start(Tag::Item) => {
                state = ParseState::Item;
            }
            Event::End(TagEnd::Item) => {
                // Paragraph End will handle flushing; if the item had no
                // explicit paragraph (a bare item), flush here.
                if !current_runs.is_empty() {
                    let runs = std::mem::take(&mut current_runs);
                    if list_stack.last().copied() == Some(true) {
                        blocks.push(DocxBlock::OrderedItem(runs));
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
                } else {
                    current_runs.push(DocxRun {
                        text: s.to_string(),
                        bold: style.bold,
                        italic: style.italic,
                        code: false,
                    });
                }
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
}
