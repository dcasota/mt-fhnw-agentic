//! Book renderer — the Rust port of the bookkit engine.
//!
//! Builds a professional A4 DOCX (title page, TOC, styled headings, tables with
//! shaded headers, embedded figures with captions) from chapter markdown via
//! `docx-rs`. Figures are expected already rendered (paths under `figdir`); the
//! caller resolves `figspec` blocks with `agentic-figures` first.

use std::io::Cursor;
use std::path::Path;

use anyhow::{Context, Result};
use docx_rs::{
    AlignmentType, BorderType, BreakType, Docx, FieldCharType, Footer, HeightRule, Hyperlink,
    HyperlinkType, InstrText, LineSpacing, LineSpacingType, PageMargin, PageNum,
    PageOrientationType, PageSize, Paragraph, Pic, Run, RunFonts, SectionProperty, Shading, Style,
    StyleType, Table, TableCell, TableCellBorder, TableCellBorderPosition, TableCellMargins,
    TableLayoutType, TableOfContents, TableRow, TextDirectionType, VAlignType, WidthType,
};

use agentic_core::i18n::t;

use crate::markdown::{DocxBlock, DocxRun, to_docx_blocks};

const NAVY: &str = "1F497D";
const HEAD2: &str = "2E4A7A";
const GREY: &str = "666666";
const ACCENT: &str = "0B5C9E"; // hyperlink blue
const HEADBG: &str = "1F3864";
const ALTBG: &str = "F4F6FA";
const RULE: &str = "C9D2E0";
const BODY: &str = "Georgia";
const HEADF: &str = "Calibri";
const MONO: &str = "Consolas";

const CONTENT_TWIPS: usize = 9298; // A4 (11906) − 2×1304 margins

// Relaxed readability (ADR-0030): 1.5 line spacing + breathing room after each
// block so text and tables are not packed edge-to-edge.
const LINE_15: i32 = 360; // 1.5× single (240 = single)
const SPACE_AFTER: u32 = 160; // ≈8 pt after body paragraphs
const SPACE_AFTER_HEAD: u32 = 120; // after headings
const SPACE_BEFORE_HEAD: u32 = 280; // before headings (separate from prose above)
const SPACE_AROUND_TABLE: u32 = 140; // spacer paragraphs hugging a table
const SPACE_AROUND_FIG: u32 = 220; // breathing room above a figure + below its caption (audit sentinel)
// Below this column width (≈2.2 cm) header labels are rotated to read bottom-up.
const ROTATE_COLW: usize = 1250;

// A table with at least this many columns is too cramped for portrait A4 even
// with fixed layout + rotated headers, so it is placed on its own A4 *landscape*
// page (ADR-0030). Mirrors the old thesis's landscape wide-table pages.
const LANDSCAPE_COLS: usize = 7;
// A4 landscape content width: 16838 − 2×1304 margins.
const LAND_CONTENT_TWIPS: usize = 14230;

/// 1.5-spaced body paragraph spacing with a little room after the block.
fn body_spacing() -> LineSpacing {
    LineSpacing::new()
        .line_rule(LineSpacingType::Auto)
        .line(LINE_15)
        .after(SPACE_AFTER)
}

/// The standard page margins, shared by the body and every mid-document section.
fn std_margin() -> PageMargin {
    PageMargin::new()
        .top(1417)
        .bottom(1417)
        .left(1304)
        .right(1304)
}

/// `sectPr` for a portrait A4 section (default next-page break).
fn portrait_sectpr() -> SectionProperty {
    SectionProperty::new()
        .page_size(PageSize::new().size(11906, 16838))
        .page_margin(std_margin())
}

/// `sectPr` for a landscape A4 section (default next-page break).
fn landscape_sectpr() -> SectionProperty {
    SectionProperty::new()
        .page_size(
            PageSize::new()
                .size(16838, 11906)
                .orient(PageOrientationType::Landscape),
        )
        .page_margin(std_margin())
}

/// Effective column count of a markdown table (header or widest row).
fn col_count(header: &[String], rows: &[Vec<String>]) -> usize {
    header
        .len()
        .max(rows.iter().map(Vec::len).max().unwrap_or(1))
        .max(1)
}

#[derive(Debug, Clone, Default)]
pub struct BookMeta {
    pub title: String,
    pub subtitle: String,
    pub author: String,
    pub context: String,
    /// Descriptive line under the title rule (bookkit DESCRIPTION). Optional.
    pub description: String,
    /// Centred dedication on the inscription page (bookkit inscription). Optional.
    pub dedication: Option<String>,
    /// Epigraph quote on the inscription page. Optional.
    pub epigraph: Option<String>,
    /// Epigraph attribution ("— Name"). Optional.
    pub epigraph_by: Option<String>,
    /// Edition & disclaimer paragraphs (one per line). Optional.
    pub disclaimer: Option<String>,
    /// Imprint lines on the title page under the affiliation (e.g. "Version 1.0",
    /// place + date). One centred line per text line. Optional.
    pub imprint: Option<String>,
    /// Master-thesis numbering profile (ADR-0045, bookkit C). When true, body
    /// chapters (Introduction, Theory, Conclusion, …) are NUMBERED and only true
    /// front/back-matter (Management Summary, Acronyms, Appendix, Bibliography,
    /// …) stays unnumbered — the opposite of the book profile, where
    /// "Introduction" is unnumbered front-matter.
    pub thesis_profile: bool,
    /// Companion-paper profile (ADR-0045, bookkit B). When true the document is
    /// NOT transformed into a book: the elaborate title page, edition/disclaimer
    /// and inscription pages are skipped in favour of a plain title + contents.
    pub companion: bool,
    /// Extra index terms beyond the built-in set (e.g. dimension-specific).
    pub index_terms: Vec<String>,
    /// Chrome language tag (en|de|fr|it|rm|hi). Empty or unknown → English.
    /// Localises only engine-generated chrome (labels, caption prefixes,
    /// list/section headings); chapter content is untouched.
    pub lang: String,
}

/// Per-book render state threaded through `render_block`: running figure / table
/// / chapter counters, the set of index terms already marked, and the current
/// chapter's collected links (for the Sources & QR-codes box).
struct Ctx<'a> {
    figdir: &'a Path,
    /// Chrome language tag (en|de|fr|it|rm|hi); drives `i18n::t` lookups.
    lang: &'a str,
    figno: u32,
    tblno: u32,
    chapno: u32,
    idx_seen: std::collections::HashSet<String>,
    /// Curated terms (built-in + per-book) marked into the index.
    index_terms: Vec<String>,
    /// (label, url) links seen in the current chapter, de-duped by URL.
    links: Vec<(String, String)>,
}

impl Ctx<'_> {
    /// Register a link for the chapter Sources box; returns its 1-based number
    /// (de-duped by URL, matching bookkit `_register_link`).
    fn register_link(&mut self, label: &str, url: &str) -> usize {
        if let Some(i) = self.links.iter().position(|(_, u)| u == url) {
            return i + 1;
        }
        self.links.push((label.to_string(), url.to_string()));
        self.links.len()
    }
}

/// Front/back-matter chapter titles that do NOT receive a chapter number
/// (bookkit UNNUMBERED set), matched case-insensitively on the first H1.
const UNNUMBERED_TITLES: &[&str] = &[
    "foreword",
    "preface",
    "acknowledgements",
    "acknowledgments",
    "introduction",
    "acronyms and abbreviations",
    "acronyms",
    "abbreviations",
    "bibliography",
    "references",
    "index",
    "contents",
    "table of figures",
    "list of figures",
    "table of tables",
    "list of tables",
    "about this book",
    "about the book",
    "glossary",
    "appendix",
    "dedication",
    "colophon",
    "disclaimer",
];

/// First H1 text of a chapter's markdown (used to decide numbering + title).
fn first_h1(md: &str) -> Option<String> {
    to_docx_blocks(md).into_iter().find_map(|b| match b {
        DocxBlock::Heading { level: 1, text } => Some(text),
        _ => None,
    })
}

/// Front/back-matter titles for the THESIS profile (ADR-0045). Unlike the book
/// profile, "Introduction"/"Conclusion"/etc. are NOT here — they are numbered
/// chapters; only true front/back-matter stays unnumbered.
const THESIS_UNNUMBERED: &[&str] = &[
    "management summary",
    "executive summary",
    "acronyms and abbreviations",
    "acronyms",
    "abbreviations",
    "table of contents",
    "contents",
    "appendix",
    "table of figures",
    "list of figures",
    "table of tables",
    "list of tables",
    "bibliography",
    "references",
    "index",
];

/// Whether a chapter is numbered: numbered unless its first H1 is a known
/// front/back-matter title. The unnumbered set depends on the profile
/// (`thesis_profile` ⇒ body chapters like Introduction are numbered).
fn chapter_is_numbered(md: &str, thesis_profile: bool) -> bool {
    let set: &[&str] = if thesis_profile {
        THESIS_UNNUMBERED
    } else {
        UNNUMBERED_TITLES
    };
    match first_h1(md) {
        Some(t) => {
            let tl = t.trim().to_lowercase();
            !set.iter().any(|u| tl == *u || tl.starts_with(u))
        }
        None => false,
    }
}

fn body_fonts() -> RunFonts {
    RunFonts::new().ascii(BODY).hi_ansi(BODY)
}
fn head_fonts() -> RunFonts {
    RunFonts::new().ascii(HEADF).hi_ansi(HEADF)
}

fn page_break() -> Paragraph {
    Paragraph::new().add_run(Run::new().add_break(BreakType::Page))
}

fn title_page(mut doc: Docx, m: &BookMeta) -> Docx {
    for _ in 0..3 {
        doc = doc.add_paragraph(Paragraph::new());
    }
    doc = doc.add_paragraph(
        Paragraph::new().align(AlignmentType::Center).add_run(
            Run::new()
                .add_text(&m.title)
                .bold()
                .size(72)
                .color(NAVY)
                .fonts(head_fonts()),
        ),
    );
    if !m.subtitle.is_empty() {
        doc = doc.add_paragraph(
            Paragraph::new().align(AlignmentType::Center).add_run(
                Run::new()
                    .add_text(&m.subtitle)
                    .size(30)
                    .color(GREY)
                    .fonts(head_fonts()),
            ),
        );
    }
    // Blue rule + descriptive line under the title (bookkit DESCRIPTION).
    if !m.description.is_empty() {
        doc = doc.add_paragraph(
            Paragraph::new()
                .align(AlignmentType::Center)
                .line_spacing(LineSpacing::new().before(160).after(120))
                .add_run(
                    Run::new()
                        .add_text("\u{2014}\u{2014}\u{2014}")
                        .color(ACCENT),
                ),
        );
        doc = doc.add_paragraph(
            Paragraph::new().align(AlignmentType::Center).add_run(
                Run::new()
                    .add_text(&m.description)
                    .italic()
                    .size(26)
                    .color(GREY)
                    .fonts(head_fonts()),
            ),
        );
    }
    for _ in 0..6 {
        doc = doc.add_paragraph(Paragraph::new());
    }
    doc = doc.add_paragraph(
        Paragraph::new().align(AlignmentType::Center).add_run(
            Run::new()
                .add_text(&m.author)
                .size(28)
                .color("1A1A1A")
                .fonts(head_fonts()),
        ),
    );
    doc = doc.add_paragraph(
        Paragraph::new().align(AlignmentType::Center).add_run(
            Run::new()
                .add_text(&m.context)
                .size(22)
                .color(GREY)
                .fonts(head_fonts()),
        ),
    );
    // Imprint lines (version, place + date) — one centred line each.
    if let Some(imp) = &m.imprint {
        doc = doc.add_paragraph(Paragraph::new());
        for line in imp.lines().map(str::trim).filter(|l| !l.is_empty()) {
            doc = doc.add_paragraph(
                Paragraph::new().align(AlignmentType::Center).add_run(
                    Run::new()
                        .add_text(line)
                        .size(20)
                        .color(GREY)
                        .fonts(head_fonts()),
                ),
            );
        }
    }
    doc.add_paragraph(page_break())
}

/// bookkit inscription page: centred dedication + epigraph (italic) + "— by".
/// No outline heading, so it stays out of the TOC.
fn inscription_page(mut doc: Docx, m: &BookMeta) -> Docx {
    if m.dedication.is_none() && m.epigraph.is_none() {
        return doc;
    }
    for _ in 0..6 {
        doc = doc.add_paragraph(Paragraph::new());
    }
    if let Some(d) = &m.dedication {
        doc = doc.add_paragraph(
            Paragraph::new().align(AlignmentType::Center).add_run(
                Run::new()
                    .add_text(d)
                    .italic()
                    .size(24)
                    .color("1A1A1A")
                    .fonts(body_fonts()),
            ),
        );
    }
    if let Some(e) = &m.epigraph {
        for _ in 0..3 {
            doc = doc.add_paragraph(Paragraph::new());
        }
        doc = doc.add_paragraph(
            Paragraph::new()
                .align(AlignmentType::Center)
                .line_spacing(body_spacing())
                .add_run(
                    Run::new()
                        .add_text(format!("\u{201c}{e}\u{201d}"))
                        .italic()
                        .size(22)
                        .color(GREY)
                        .fonts(body_fonts()),
                ),
        );
        if let Some(by) = &m.epigraph_by {
            doc = doc.add_paragraph(
                Paragraph::new().align(AlignmentType::Center).add_run(
                    Run::new()
                        .add_text(format!("\u{2014} {by}"))
                        .size(20)
                        .color(GREY)
                        .fonts(body_fonts()),
                ),
            );
        }
    }
    doc.add_paragraph(page_break())
}

/// bookkit "Edition & Disclaimer" page: a heading + one grey paragraph per line.
fn disclaimer_page(mut doc: Docx, m: &BookMeta) -> Docx {
    let Some(text) = &m.disclaimer else {
        return doc;
    };
    doc = doc.add_paragraph(
        Paragraph::new().add_run(
            Run::new()
                .add_text(t(&m.lang, "edition_disclaimer"))
                .bold()
                .size(32)
                .color(NAVY)
                .fonts(head_fonts()),
        ),
    );
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        doc = doc.add_paragraph(
            Paragraph::new().line_spacing(body_spacing()).add_run(
                Run::new()
                    .add_text(line)
                    .size(19)
                    .color(GREY)
                    .fonts(body_fonts()),
            ),
        );
    }
    doc.add_paragraph(page_break())
}

fn heading_para(level: u8, text: &str, page_break_before: bool) -> Paragraph {
    let (size, color) = match level {
        1 => (44, NAVY),
        2 => (32, NAVY),
        3 => (26, HEAD2),
        _ => (23, HEAD2),
    };
    let mut p = Paragraph::new()
        .style(&format!("Heading{}", level.min(4)))
        .line_spacing(
            LineSpacing::new()
                .before(SPACE_BEFORE_HEAD)
                .after(SPACE_AFTER_HEAD),
        );
    if page_break_before {
        p = p.add_run(Run::new().add_break(BreakType::Page));
    }
    p.add_run(
        Run::new()
            .add_text(text)
            .bold()
            .size(size)
            .color(color)
            .fonts(head_fonts()),
    )
}

fn run_of(r: &DocxRun) -> Run {
    let mut run = Run::new().add_text(&r.text).size(22);
    run = if r.code {
        run.fonts(RunFonts::new().ascii(MONO).hi_ansi(MONO))
    } else {
        run.fonts(body_fonts())
    };
    if r.bold {
        run = run.bold();
    }
    if r.italic {
        run = run.italic();
    }
    run
}

/// A small bracketed reference-number run (bookkit `_superscript`), pointing
/// into the chapter Sources box. docx-rs 0.4 has no public run vertical-align,
/// so this is a small raised-looking `[n]` marker rather than a true superscript.
fn superscript(n: usize) -> Run {
    Run::new()
        .add_text(format!("\u{200a}[{n}]"))
        .size(15)
        .color(ACCENT)
        .fonts(body_fonts())
}

/// Add a run sequence to a paragraph. Markdown links (`[label](url)`) render as
/// the label plus a superscript reference number and are registered in the
/// chapter's link registry (bookkit `add_inline` + `_register_link`); the URLs
/// then appear in the end-of-chapter Sources & QR-codes box.
fn add_runs(mut p: Paragraph, runs: &[DocxRun], links: &mut Vec<(String, String)>) -> Paragraph {
    for r in runs {
        if let Some(url) = &r.link {
            // Register (de-dupe by URL) and emit label + superscript number.
            let n = match links.iter().position(|(_, u)| u == url) {
                Some(i) => i + 1,
                None => {
                    links.push((r.text.clone(), url.clone()));
                    links.len()
                }
            };
            let mut label = Run::new()
                .add_text(&r.text)
                .size(22)
                .color(ACCENT)
                .fonts(body_fonts());
            if r.bold {
                label = label.bold();
            }
            p = p.add_run(label).add_run(superscript(n));
        } else {
            p = p.add_run(run_of(r));
        }
    }
    p
}

fn para_of(runs: &[DocxRun], links: &mut Vec<(String, String)>) -> Paragraph {
    add_runs(Paragraph::new().line_spacing(body_spacing()), runs, links)
}

/// Parse PNG width/height from the IHDR chunk (bytes 16..24, big-endian).
fn png_dims(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 24 || &bytes[1..4] != b"PNG" {
        return None;
    }
    let w = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
    let h = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
    Some((w, h))
}

fn table_block(header: &[String], rows: &[Vec<String>], content_twips: usize) -> Table {
    let ncols = col_count(header, rows);
    let colw = content_twips / ncols;
    // Narrow many-column tables: rotate non-trivial header labels to read
    // bottom-up so they stay legible instead of wrapping into a sliver.
    let rotate_headers = colw < ROTATE_COLW && header.iter().any(|h| h.trim().chars().count() > 4);
    let mut trows = Vec::new();
    if !header.is_empty() {
        let cells = header
            .iter()
            .map(|h| {
                let para = Paragraph::new()
                    .align(if rotate_headers {
                        AlignmentType::Center
                    } else {
                        AlignmentType::Left
                    })
                    .add_run(
                        Run::new()
                            .add_text(h)
                            .bold()
                            .size(19)
                            .color("FFFFFF")
                            .fonts(body_fonts()),
                    );
                let mut cell = TableCell::new()
                    .shading(Shading::new().fill(HEADBG))
                    .width(colw, WidthType::Dxa)
                    .vertical_align(VAlignType::Center);
                if rotate_headers {
                    cell = cell.text_direction(TextDirectionType::BtLr);
                }
                cell.add_paragraph(para)
            })
            .collect();
        let mut hrow = TableRow::new(cells);
        if rotate_headers {
            // Give the rotated labels vertical room.
            hrow = hrow.row_height(1600.0).height_rule(HeightRule::AtLeast);
        }
        trows.push(hrow);
    }
    for (ri, row) in rows.iter().enumerate() {
        let fill = if ri % 2 == 0 { ALTBG } else { "FFFFFF" };
        let mut cells = Vec::with_capacity(ncols);
        for c in 0..ncols {
            let val = row.get(c).map(String::as_str).unwrap_or("");
            cells.push(
                TableCell::new()
                    .shading(Shading::new().fill(fill))
                    .width(colw, WidthType::Dxa)
                    .vertical_align(VAlignType::Center)
                    .add_paragraph(
                        Paragraph::new()
                            .add_run(Run::new().add_text(val).size(19).fonts(body_fonts())),
                    ),
            );
        }
        trows.push(TableRow::new(cells));
    }
    Table::new(trows)
        .set_grid(vec![colw; ncols])
        .width(content_twips, WidthType::Dxa)
        // Fixed layout makes Word honour the grid and wrap text, so a wide table
        // can never expand past the page margins (ADR-0030).
        .layout(TableLayoutType::Fixed)
        // Cell padding so text never touches the borders.
        .margins(TableCellMargins::new().margin(60, 100, 60, 100))
}

/// chapter_extras.py "Key topics at a glance" box: a shaded single-column table
/// (navy header + zebra key-point rows), with breathing room around it.
fn keypoints_box(mut doc: Docx, body: &str) -> Docx {
    let spacer = || Paragraph::new().line_spacing(LineSpacing::new().after(SPACE_AROUND_TABLE));
    let mut rows = vec![TableRow::new(vec![
        TableCell::new()
            .shading(Shading::new().fill(HEADBG))
            .width(CONTENT_TWIPS, WidthType::Dxa)
            .add_paragraph(
                Paragraph::new().add_run(
                    Run::new()
                        .add_text("Key topics at a glance")
                        .bold()
                        .size(21)
                        .color("FFFFFF")
                        .fonts(head_fonts()),
                ),
            ),
    ])];
    for (i, line) in body
        .lines()
        .map(|l| l.trim().trim_start_matches(['-', '•', '*', ' ']).trim())
        .filter(|l| !l.is_empty())
        .enumerate()
    {
        let fill = if i % 2 == 0 { ALTBG } else { "FFFFFF" };
        rows.push(TableRow::new(vec![
            TableCell::new()
                .shading(Shading::new().fill(fill))
                .width(CONTENT_TWIPS, WidthType::Dxa)
                .add_paragraph(
                    Paragraph::new()
                        .line_spacing(LineSpacing::new().after(40))
                        .add_run(
                            Run::new()
                                .add_text("•  ")
                                .bold()
                                .size(21)
                                .color(NAVY)
                                .fonts(body_fonts()),
                        )
                        .add_run(Run::new().add_text(line).size(21).fonts(body_fonts())),
                ),
        ]));
    }
    doc = doc.add_paragraph(spacer());
    doc = doc.add_table(
        Table::new(rows)
            .set_grid(vec![CONTENT_TWIPS])
            .width(CONTENT_TWIPS, WidthType::Dxa)
            .layout(TableLayoutType::Fixed)
            .margins(TableCellMargins::new().margin(70, 120, 70, 120)),
    );
    doc.add_paragraph(spacer())
}

/// bookkit.py admonition: a colour-coded shaded box with a left accent border
/// and a bold label, for note / tip / warning asides. Rendered as a single-cell
/// table so the fill + left border survive in Word.
fn admonition_box(mut doc: Docx, kind: &str, body: &str, figdir: &Path, lang: &str) -> Docx {
    // Label is localised chrome; the SEQ-free admonition has no field name to
    // keep stable, so the visible word is translated directly.
    let (word, glyph, fill, edge) = match kind {
        "tip" => (t(lang, "tip"), "\u{2714}", "EAF6EC", "2E7D32"),
        "warning" => (t(lang, "warning"), "\u{26A0}", "FBF1E2", "C77F18"),
        _ => (t(lang, "note"), "\u{2139}", "EAF1FB", "1F3864"),
    };
    let text: String = body
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    // gen_icons PNG (icon_{kind}.png) if the book command rendered it into
    // figdir; otherwise fall back to a unicode glyph.
    let icon = std::fs::read(figdir.join(format!("icon_{kind}.png"))).ok();
    let mut label_para = Paragraph::new().line_spacing(body_spacing());
    if let Some(bytes) = &icon {
        let pic = Pic::new(bytes).size(150_000, 150_000); // ≈0.4 cm square
        label_para = label_para.add_run(Run::new().add_image(pic)).add_run(
            Run::new()
                .add_text(format!(" {word}  "))
                .bold()
                .size(21)
                .color(edge)
                .fonts(head_fonts()),
        );
    } else {
        label_para = label_para.add_run(
            Run::new()
                .add_text(format!("{glyph} {word}  "))
                .bold()
                .size(21)
                .color(edge)
                .fonts(head_fonts()),
        );
    }
    label_para = label_para.add_run(Run::new().add_text(text).size(22).fonts(body_fonts()));
    let spacer = || Paragraph::new().line_spacing(LineSpacing::new().after(SPACE_AROUND_TABLE));
    let cell = TableCell::new()
        .shading(Shading::new().fill(fill))
        .width(CONTENT_TWIPS, WidthType::Dxa)
        .set_border(
            TableCellBorder::new(TableCellBorderPosition::Left)
                .color(edge)
                .size(24)
                .border_type(BorderType::Single),
        )
        .add_paragraph(label_para);
    doc = doc.add_paragraph(spacer());
    doc = doc.add_table(
        Table::new(vec![TableRow::new(vec![cell])])
            .set_grid(vec![CONTENT_TWIPS])
            .width(CONTENT_TWIPS, WidthType::Dxa)
            .layout(TableLayoutType::Fixed)
            .margins(TableCellMargins::new().margin(70, 120, 70, 120)),
    );
    doc.add_paragraph(spacer())
}

/// bookkit.py generic callout: a navy left-bordered shaded box; the first line
/// (if it ends with a colon) is a bold navy title, the rest is the body.
fn callout_box(mut doc: Docx, body: &str) -> Docx {
    let lines: Vec<&str> = body
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let (title, rest) = match lines.split_first() {
        Some((first, tail)) if first.ends_with(':') => (Some(*first), tail.join(" ")),
        _ => (None, lines.join(" ")),
    };
    let spacer = || Paragraph::new().line_spacing(LineSpacing::new().after(SPACE_AROUND_TABLE));
    let mut cell = TableCell::new()
        .shading(Shading::new().fill("EEF2F8"))
        .width(CONTENT_TWIPS, WidthType::Dxa)
        .set_border(
            TableCellBorder::new(TableCellBorderPosition::Left)
                .color(NAVY)
                .size(24)
                .border_type(BorderType::Single),
        );
    if let Some(t) = title {
        cell = cell.add_paragraph(
            Paragraph::new().add_run(
                Run::new()
                    .add_text(t.trim_end_matches(':'))
                    .bold()
                    .size(21)
                    .color(NAVY)
                    .fonts(head_fonts()),
            ),
        );
    }
    cell = cell.add_paragraph(
        Paragraph::new()
            .line_spacing(body_spacing())
            .add_run(Run::new().add_text(rest).size(22).fonts(body_fonts())),
    );
    doc = doc.add_paragraph(spacer());
    doc = doc.add_table(
        Table::new(vec![TableRow::new(vec![cell])])
            .set_grid(vec![CONTENT_TWIPS])
            .width(CONTENT_TWIPS, WidthType::Dxa)
            .layout(TableLayoutType::Fixed)
            .margins(TableCellMargins::new().margin(70, 120, 70, 120)),
    );
    doc.add_paragraph(spacer())
}

/// chapter_extras.py per-chapter "Review questions": `Q:`/`A:` line pairs become
/// a numbered bold question + a grey italic answer.
fn quiz_block(mut doc: Docx, body: &str) -> Docx {
    doc = doc.add_paragraph(
        Paragraph::new()
            .line_spacing(
                LineSpacing::new()
                    .before(SPACE_BEFORE_HEAD)
                    .after(SPACE_AFTER_HEAD),
            )
            .add_run(
                Run::new()
                    .add_text("Review questions")
                    .bold()
                    .size(26)
                    .color(HEAD2)
                    .fonts(head_fonts()),
            ),
    );
    let mut qn = 0u32;
    let mut cur_q: Option<String> = None;
    for line in body.lines() {
        let l = line.trim();
        if let Some(q) = l.strip_prefix("Q:") {
            cur_q = Some(q.trim().to_string());
        } else if let Some(a) = l.strip_prefix("A:") {
            qn += 1;
            let q = cur_q.take().unwrap_or_default();
            doc = doc.add_paragraph(
                Paragraph::new()
                    .line_spacing(LineSpacing::new().before(80).after(30))
                    .add_run(
                        Run::new()
                            .add_text(format!("{qn}. {q}"))
                            .bold()
                            .size(22)
                            .color("1A1A1A")
                            .fonts(body_fonts()),
                    ),
            );
            doc = doc.add_paragraph(
                Paragraph::new().line_spacing(body_spacing()).add_run(
                    Run::new()
                        .add_text(a.trim())
                        .italic()
                        .size(21)
                        .color(GREY)
                        .fonts(body_fonts()),
                ),
            );
        }
    }
    doc
}

/// bookkit quote block: an indented italic block with a left blue border and an
/// optional "— attribution" line (a body line starting with `—` or `by:`).
fn quote_block(mut doc: Docx, body: &str) -> Docx {
    let mut lines: Vec<&str> = body
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let mut by: Option<String> = None;
    if let Some(last) = lines.last() {
        if let Some(rest) = last
            .strip_prefix('\u{2014}')
            .or_else(|| last.strip_prefix("by:"))
        {
            by = Some(rest.trim().to_string());
            lines.pop();
        }
    }
    let quote = lines.join(" ");
    let spacer = || Paragraph::new().line_spacing(LineSpacing::new().after(SPACE_AROUND_TABLE));
    let mut cell = TableCell::new()
        .width(CONTENT_TWIPS, WidthType::Dxa)
        .set_border(
            TableCellBorder::new(TableCellBorderPosition::Left)
                .color(ACCENT)
                .size(18)
                .border_type(BorderType::Single),
        )
        .add_paragraph(
            Paragraph::new().line_spacing(body_spacing()).add_run(
                Run::new()
                    .add_text(quote)
                    .italic()
                    .size(23)
                    .color("1A1A1A")
                    .fonts(body_fonts()),
            ),
        );
    if let Some(b) = by {
        cell = cell.add_paragraph(
            Paragraph::new().add_run(
                Run::new()
                    .add_text(format!("\u{2014} {b}"))
                    .size(20)
                    .color(GREY)
                    .fonts(body_fonts()),
            ),
        );
    }
    doc = doc.add_paragraph(spacer());
    doc = doc.add_table(
        Table::new(vec![TableRow::new(vec![cell])])
            .set_grid(vec![CONTENT_TWIPS])
            .width(CONTENT_TWIPS, WidthType::Dxa)
            .layout(TableLayoutType::Fixed)
            .margins(TableCellMargins::new().margin(70, 200, 70, 120)),
    );
    doc.add_paragraph(spacer())
}

/// bookkit "Conventions Used in This Book": a live demo of the typographic
/// conventions (italic / monospace variants) plus the three admonition styles.
fn conventions_block(mut doc: Docx, figdir: &Path, lang: &str) -> Docx {
    // Localised section title (engine chrome). Resolved before the local `mono`
    // /`plain` closures shadow the imported `t`.
    let conv_title = t(lang, "conventions_title");
    doc = doc.add_paragraph(
        Paragraph::new()
            .line_spacing(
                LineSpacing::new()
                    .before(SPACE_BEFORE_HEAD)
                    .after(SPACE_AFTER_HEAD),
            )
            .add_run(
                Run::new()
                    .add_text(conv_title)
                    .bold()
                    .size(26)
                    .color(HEAD2)
                    .fonts(head_fonts()),
            ),
    );
    let bullet = |doc: Docx, runs: Vec<Run>| -> Docx {
        let mut p = Paragraph::new().line_spacing(body_spacing()).add_run(
            Run::new()
                .add_text("\u{2022}  ")
                .bold()
                .size(22)
                .color(NAVY)
                .fonts(body_fonts()),
        );
        for r in runs {
            p = p.add_run(r);
        }
        doc.add_paragraph(p)
    };
    let mono = |t: &str| {
        Run::new()
            .add_text(t)
            .size(21)
            .fonts(RunFonts::new().ascii(MONO).hi_ansi(MONO))
    };
    let plain = |t: &str| Run::new().add_text(t).size(22).fonts(body_fonts());
    doc = bullet(
        doc,
        vec![
            plain("Italic"),
            Run::new()
                .add_text(" \u{2014} emphasis, terms, and titles.")
                .italic()
                .size(22)
                .fonts(body_fonts()),
        ],
    );
    doc = bullet(
        doc,
        vec![
            mono("Constant width"),
            plain(" \u{2014} commands, code, file names and identifiers."),
        ],
    );
    doc = bullet(
        doc,
        vec![
            mono("Constant width bold").bold(),
            plain(" \u{2014} literal user input."),
        ],
    );
    doc = bullet(
        doc,
        vec![
            mono("Constant width italic").italic(),
            plain(" \u{2014} values you supply."),
        ],
    );
    doc = admonition_box(
        doc,
        "tip",
        "A tip points out a useful shortcut or best practice.",
        figdir,
        lang,
    );
    doc = admonition_box(
        doc,
        "note",
        "A note adds context worth keeping in mind.",
        figdir,
        lang,
    );
    doc = admonition_box(
        doc,
        "warning",
        "A warning flags a pitfall or irreversible action.",
        figdir,
        lang,
    );
    doc
}

/// A real horizontal rule: an empty paragraph carrying a bottom border.
fn rule_para() -> Paragraph {
    Paragraph::new()
        .line_spacing(LineSpacing::new().before(60).after(120))
        .add_run(Run::new().add_text("\u{2500}".repeat(60)).color(RULE))
}

/// A Word field `{ instr }` with a cached display value — lets us emit arbitrary
/// fields (SEQ, TOC \c, XE, INDEX) that docx-rs has no builder for.
fn field_run(instr: &str, cached: &str) -> Run {
    // `InstrText::Unsupported` is written verbatim (no escaping), so a term such
    // as "MITRE ATT&CK" would emit a raw `&` and break the XML. Escape it.
    let instr = instr
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let mut r = Run::new()
        .add_field_char(FieldCharType::Begin, false)
        .add_instr_text(InstrText::Unsupported(instr))
        .add_field_char(FieldCharType::Separate, false);
    if !cached.is_empty() {
        r = r.add_text(cached.to_string());
    }
    r.add_field_char(FieldCharType::End, false)
}

/// Curated index terms (bookkit.py port). Matched case-insensitively in body
/// text; the first hit per term per book gets an XE field so the INDEX field can
/// compile a real, page-referenced index.
const INDEX_TERMS: &[&str] = &[
    "ISO/IEC 42001",
    "ISO/IEC 27001",
    "ISO/IEC 23894",
    "ISO/IEC 5338",
    "NIST AI RMF",
    "NIST Cybersecurity Framework",
    "EU AI Act",
    "CVSS",
    "Process Autonomy Matrix",
    "reproducible build",
    "SBOM",
    "CBOM",
    "post-quantum cryptography",
    "ML-DSA",
    "MITRE ATLAS",
    "MITRE ATT&CK",
    "AI Master",
    "Team 4.0",
    "Habitat",
    "human-in-the-loop",
    "FINMA",
    "BACS",
    "FDPIC",
    "Photon OS",
    "Broadcom",
    "verification gate",
    "three-agent consensus",
    "crypto-agility",
    "ISO 42001",
    // Dimension-corpus terms (Agentic-AI thesis).
    "ISO/IEC 23053",
    "ISO/IEC TR 24028",
    "ISO/IEC 25059",
    "GDPR",
    "DORA",
    "NIS2",
    "Cyber Resilience Act",
    "ML-KEM",
    "SLH-DSA",
    "harvest-now-decrypt-later",
    "QUBO",
    "COCOMO",
    "agentic AI",
    "non-human identity",
    "RBAC",
    "trust score",
    "material passport",
    "claim audit",
    "SLSA",
    "AIBOM",
    "QBOM",
    "HBOM",
    "self-adaptive",
    "MAPE-K",
    "digital sovereignty",
    "operating mode",
    "escalation",
    "tdnf",
];

/// XE index-entry field runs for any curated term first seen in `text`.
fn index_marks(
    text: &str,
    terms: &[String],
    seen: &mut std::collections::HashSet<String>,
) -> Vec<Run> {
    let lower = text.to_lowercase();
    let mut out = Vec::new();
    for term in terms {
        if !seen.contains(term) && lower.contains(&term.to_lowercase()) {
            seen.insert(term.clone());
            out.push(field_run(&format!("XE \"{term}\""), ""));
        }
    }
    out
}

/// A "List of Figures"/"List of Tables" section: a heading + a `TOC \c` field
/// that Word fills from the caption SEQ fields.
fn list_of(seq: &str, heading: &str) -> [Paragraph; 2] {
    [
        Paragraph::new()
            .style("Heading1")
            .line_spacing(
                LineSpacing::new()
                    .before(SPACE_BEFORE_HEAD)
                    .after(SPACE_AFTER_HEAD),
            )
            .add_run(
                Run::new()
                    .add_text(heading)
                    .bold()
                    .size(32)
                    .color(NAVY)
                    .fonts(head_fonts()),
            ),
        Paragraph::new().add_run(field_run(&format!("TOC \\h \\z \\c \"{seq}\""), "")),
    ]
}

/// Generate a PNG QR code for a URL (bookkit `_qr_png`). None on encode failure.
/// Built from the module matrix directly so it does not couple to qrcode's own
/// image-crate version.
fn qr_png(url: &str) -> Option<Vec<u8>> {
    use qrcode::types::Color;
    let code = qrcode::QrCode::new(url.as_bytes()).ok()?;
    let modules = code.width() as u32; // modules per side
    let colors = code.to_colors();
    let q = 4u32; // quiet-zone modules
    let px = 6u32; // pixels per module
    let dim = (modules + 2 * q) * px;
    let mut img = image::GrayImage::from_pixel(dim, dim, image::Luma([255u8]));
    for my in 0..modules {
        for mx in 0..modules {
            if matches!(colors[(my * modules + mx) as usize], Color::Dark) {
                let (ox, oy) = ((mx + q) * px, (my + q) * px);
                for dy in 0..px {
                    for dx in 0..px {
                        img.put_pixel(ox + dx, oy + dy, image::Luma([0u8]));
                    }
                }
            }
        }
    }
    let mut buf = Cursor::new(Vec::<u8>::new());
    image::DynamicImage::ImageLuma8(img)
        .write_to(&mut buf, image::ImageFormat::Png)
        .ok()?;
    Some(buf.into_inner())
}

/// bookkit `flush_sources`: the end-of-chapter "Sources & QR codes" box — a
/// two-column table (numbered link | scannable QR) of every link registered in
/// the chapter. Clears the registry. The heading is a plain bold run (not an
/// outline Heading) so it stays out of the TOC.
fn flush_sources(mut doc: Docx, links: &mut Vec<(String, String)>, lang: &str) -> Docx {
    if links.is_empty() {
        return doc;
    }
    doc = doc.add_paragraph(
        Paragraph::new()
            .line_spacing(
                LineSpacing::new()
                    .before(SPACE_BEFORE_HEAD)
                    .after(SPACE_AFTER_HEAD),
            )
            .add_run(
                Run::new()
                    .add_text(t(lang, "sources_box"))
                    .bold()
                    .size(26)
                    .color(HEAD2)
                    .fonts(head_fonts()),
            ),
    );
    doc = doc.add_paragraph(
        Paragraph::new()
            .line_spacing(LineSpacing::new().after(80))
            .add_run(
                Run::new()
                    .add_text("Scan a code, or follow the link, to reach the cited source.")
                    .italic()
                    .size(18)
                    .color(GREY)
                    .fonts(body_fonts()),
            ),
    );
    const QR_COL: usize = 1700; // ≈3.0 cm
    let text_col = CONTENT_TWIPS - QR_COL;
    let mut rows = Vec::new();
    for (i, (label, url)) in links.iter().enumerate() {
        let n = i + 1;
        let left = TableCell::new()
            .width(text_col, WidthType::Dxa)
            .vertical_align(VAlignType::Center)
            .add_paragraph(
                Paragraph::new()
                    .line_spacing(LineSpacing::new().after(40))
                    .add_run(
                        Run::new()
                            .add_text(format!("{n}.  {label}"))
                            .bold()
                            .size(19)
                            .color("1A1A1A")
                            .fonts(body_fonts()),
                    ),
            )
            .add_paragraph(
                Paragraph::new().add_hyperlink(
                    Hyperlink::new(url, HyperlinkType::External).add_run(
                        Run::new()
                            .add_text(url)
                            .size(16)
                            .color(ACCENT)
                            .underline("single")
                            .fonts(body_fonts()),
                    ),
                ),
            );
        let qr_para = match qr_png(url) {
            Some(png) => Paragraph::new()
                .align(AlignmentType::Center)
                .add_run(Run::new().add_image(Pic::new(&png).size(900_000, 900_000))),
            None => Paragraph::new()
                .align(AlignmentType::Center)
                .add_run(Run::new().add_text("[QR]").size(16).color(GREY)),
        };
        let right = TableCell::new()
            .width(QR_COL, WidthType::Dxa)
            .vertical_align(VAlignType::Center)
            .add_paragraph(qr_para);
        rows.push(TableRow::new(vec![left, right]));
    }
    doc = doc.add_table(
        Table::new(rows)
            .set_grid(vec![text_col, QR_COL])
            .width(CONTENT_TWIPS, WidthType::Dxa)
            .layout(TableLayoutType::Fixed)
            .margins(TableCellMargins::new().margin(60, 100, 60, 100)),
    );
    links.clear();
    doc.add_paragraph(Paragraph::new().line_spacing(LineSpacing::new().after(SPACE_AROUND_TABLE)))
}

fn render_block(
    mut doc: Docx,
    b: &DocxBlock,
    ctx: &mut Ctx,
    chapter_start: bool,
    numbered: bool,
) -> Docx {
    match b {
        DocxBlock::Heading { level, text } => {
            // Chapter number prefix on the first H1 of a numbered chapter.
            let shown = if *level == 1 && chapter_start && numbered {
                ctx.chapno += 1;
                format!("{}  {text}", ctx.chapno)
            } else {
                text.clone()
            };
            doc.add_paragraph(heading_para(*level, &shown, chapter_start && *level <= 2))
        }
        DocxBlock::Paragraph(runs) => {
            let mut p = para_of(runs, &mut ctx.links);
            let text: String = runs.iter().map(|r| r.text.as_str()).collect();
            for xe in index_marks(&text, &ctx.index_terms, &mut ctx.idx_seen) {
                p = p.add_run(xe);
            }
            doc.add_paragraph(p)
        }
        DocxBlock::BulletItem(runs) => {
            let mut p = Paragraph::new().line_spacing(body_spacing()).add_run(
                Run::new()
                    .add_text("•  ")
                    .size(22)
                    .color(NAVY)
                    .bold()
                    .fonts(body_fonts()),
            );
            p = add_runs(p, runs, &mut ctx.links);
            doc.add_paragraph(p)
        }
        DocxBlock::OrderedItem { number, runs } => {
            let mut p = Paragraph::new().line_spacing(body_spacing()).add_run(
                Run::new()
                    .add_text(format!("{number}.  "))
                    .size(22)
                    .color(NAVY)
                    .bold()
                    .fonts(body_fonts()),
            );
            p = add_runs(p, runs, &mut ctx.links);
            doc.add_paragraph(p)
        }
        DocxBlock::CodeBlock { lang, body } => match lang.as_str() {
            // chapter_extras.py port: the "Key topics at a glance" box.
            "keypoints" => keypoints_box(doc, body),
            // chapter_extras.py port: the per-chapter "Review questions".
            "quiz" => quiz_block(doc, body),
            // bookkit.py port: note / tip / warning admonition callouts.
            "note" | "tip" | "warning" => admonition_box(doc, lang, body, ctx.figdir, ctx.lang),
            // bookkit.py port: a generic titled key-point callout box.
            "callout" => callout_box(doc, body),
            // bookkit.py port: pull-quote with optional "— attribution".
            "quote" => quote_block(doc, body),
            // bookkit.py port: lead-in paragraph (slightly larger).
            "lead" => {
                let text: String = body.lines().map(str::trim).collect::<Vec<_>>().join(" ");
                doc.add_paragraph(
                    Paragraph::new().line_spacing(body_spacing()).add_run(
                        Run::new()
                            .add_text(text.trim())
                            .size(23)
                            .fonts(body_fonts()),
                    ),
                )
            }
            // bookkit.py port: "Conventions Used in This Book" live demo.
            "conventions" => conventions_block(doc, ctx.figdir, ctx.lang),
            _ => {
                let mut p = Paragraph::new();
                for (i, line) in body.split('\n').enumerate() {
                    if i > 0 {
                        p = p.add_run(Run::new().add_break(BreakType::TextWrapping));
                    }
                    p = p.add_run(
                        Run::new()
                            .add_text(line)
                            .size(19)
                            .fonts(RunFonts::new().ascii(MONO).hi_ansi(MONO)),
                    );
                }
                doc.add_paragraph(p)
            }
        },
        DocxBlock::HorizontalRule => doc.add_paragraph(rule_para()),
        DocxBlock::Table {
            header,
            rows,
            caption,
        } => {
            // bookkit caption-above with "Table N." SEQ numbering.
            if let Some(cap) = caption {
                ctx.tblno += 1;
                let cap_style = |t: &str| {
                    Run::new()
                        .add_text(t.to_string())
                        .italic()
                        .size(18)
                        .color(GREY)
                        .fonts(body_fonts())
                };
                doc = doc.add_paragraph(
                    Paragraph::new()
                        .line_spacing(LineSpacing::new().before(SPACE_AROUND_TABLE).after(40))
                        .add_run(cap_style(t(ctx.lang, "table_prefix")))
                        .add_run(field_run("SEQ Table \\* ARABIC", &format!("{}", ctx.tblno)))
                        .add_run(cap_style(&format!(". {cap}"))),
                );
            }
            if col_count(header, rows) >= LANDSCAPE_COLS {
                // Wide table → its own A4 landscape page (ADR-0030). The empty
                // paragraph carrying the portrait sectPr ends the portrait
                // section; the table then lives in the landscape section, which
                // the trailing landscape-sectPr paragraph closes before portrait
                // content resumes.
                doc = doc.add_paragraph(Paragraph::new().section_property(portrait_sectpr()));
                doc = doc.add_table(table_block(header, rows, LAND_CONTENT_TWIPS));
                doc.add_paragraph(Paragraph::new().section_property(landscape_sectpr()))
            } else {
                // Breathing room around the table (ADR-0030 relaxed placement).
                let spacer =
                    || Paragraph::new().line_spacing(LineSpacing::new().after(SPACE_AROUND_TABLE));
                doc = doc.add_paragraph(spacer());
                doc = doc.add_table(table_block(header, rows, CONTENT_TWIPS));
                doc.add_paragraph(spacer())
            }
        }
        DocxBlock::Image { path, caption } => {
            let full = ctx
                .figdir
                .join(path.replace('/', std::path::MAIN_SEPARATOR_STR));
            if let Ok(bytes) = std::fs::read(&full) {
                ctx.figno += 1;
                let target_w: u32 = 5_400_000; // 15 cm in EMU
                let h_emu = png_dims(&bytes)
                    .map(|(w, h)| {
                        ((u64::from(h) * u64::from(target_w)) / u64::from(w.max(1))) as u32
                    })
                    .unwrap_or(3_400_000);
                let pic = Pic::new(&bytes).size(target_w, h_emu);
                // Breathing room above the figure (ADR-0030 relaxed placement).
                doc = doc.add_paragraph(
                    Paragraph::new().line_spacing(LineSpacing::new().after(SPACE_AROUND_FIG)),
                );
                doc = doc.add_paragraph(
                    Paragraph::new()
                        .align(AlignmentType::Center)
                        .line_spacing(LineSpacing::new().after(80))
                        .add_run(Run::new().add_image(pic)),
                );
                // Caption with generous room after, so the next text isn't crammed.
                // Caption with a SEQ field so a List of Figures can collect it.
                let cap_style = |t: &str| {
                    Run::new()
                        .add_text(t.to_string())
                        .italic()
                        .size(18)
                        .color(GREY)
                        .fonts(body_fonts())
                };
                doc.add_paragraph(
                    Paragraph::new()
                        .align(AlignmentType::Center)
                        .line_spacing(LineSpacing::new().after(SPACE_AROUND_FIG))
                        .add_run(cap_style(t(ctx.lang, "fig_prefix")))
                        .add_run(field_run(
                            "SEQ Figure \\* ARABIC",
                            &format!("{}", ctx.figno),
                        ))
                        .add_run(cap_style(&format!(". {caption}"))),
                )
            } else {
                doc.add_paragraph(
                    Paragraph::new().add_run(
                        Run::new()
                            .add_text(format!("[figure missing: {path}]"))
                            .italic()
                            .color(GREY),
                    ),
                )
            }
        }
    }
}

/// Render a complete book to DOCX bytes. `chapters` are `(label, markdown)`
/// with figures already rendered under `figdir`.
/// Define the Heading1–4 paragraph styles (with outline levels so the Word TOC
/// field populates) + the caption style. docx-rs does not ship Heading styles,
/// so referencing them without defining them yields an empty TOC.
fn with_styles(mut doc: Docx) -> Docx {
    let specs = [
        (1u8, 44usize, NAVY),
        (2, 32, NAVY),
        (3, 26, HEAD2),
        (4, 23, HEAD2),
    ];
    for (lvl, size, color) in specs {
        doc = doc.add_style(
            Style::new(format!("Heading{lvl}"), StyleType::Paragraph)
                .name(format!("heading {lvl}"))
                .based_on("Normal")
                .size(size)
                .bold()
                .color(color)
                .fonts(head_fonts())
                .outline_lvl(usize::from(lvl) - 1),
        );
    }
    doc
}

/// Fold a `Table:`-prefixed paragraph into the caption of the table that
/// immediately follows it (bookkit caption-above convention for markdown).
fn fold_table_captions(blocks: Vec<DocxBlock>) -> Vec<DocxBlock> {
    let mut out: Vec<DocxBlock> = Vec::with_capacity(blocks.len());
    let mut pending: Option<String> = None;
    for b in blocks {
        match b {
            DocxBlock::Paragraph(ref runs) => {
                let text: String = runs.iter().map(|r| r.text.as_str()).collect();
                let t = text.trim();
                if let Some(rest) = t
                    .strip_prefix("Table:")
                    .or_else(|| t.strip_prefix("table:"))
                {
                    pending = Some(rest.trim().to_string());
                } else {
                    pending = None;
                    out.push(b);
                }
            }
            DocxBlock::Table {
                header,
                rows,
                caption,
            } => {
                let cap = pending.take().or(caption);
                out.push(DocxBlock::Table {
                    header,
                    rows,
                    caption: cap,
                });
            }
            other => {
                pending = None;
                out.push(other);
            }
        }
    }
    out
}

pub fn render_book(
    meta: &BookMeta,
    chapters: &[(String, String)],
    figdir: &Path,
) -> Result<Vec<u8>> {
    // The FHNW master-thesis profile (bookkit C) follows a mandated front/back-
    // matter reading order that differs from the book layout; render it on its
    // own path. The book (A) and companion (B) profiles below are unchanged.
    if meta.thesis_profile {
        return render_thesis_book(meta, chapters, figdir);
    }
    let mut doc = with_styles(
        Docx::new()
            .default_fonts(body_fonts())
            .default_size(22)
            .page_size(11906, 16838)
            .page_margin(std_margin()),
    )
    .footer(
        Footer::new().add_paragraph(
            Paragraph::new()
                .align(AlignmentType::Center)
                .add_page_num(PageNum::new()),
        ),
    );

    if meta.companion {
        // Companion paper (bookkit B): a plain title, no book chrome.
        doc = doc.add_paragraph(
            Paragraph::new().align(AlignmentType::Center).add_run(
                Run::new()
                    .add_text(&meta.title)
                    .bold()
                    .size(48)
                    .color(NAVY)
                    .fonts(head_fonts()),
            ),
        );
        if !meta.subtitle.is_empty() {
            doc = doc.add_paragraph(
                Paragraph::new().align(AlignmentType::Center).add_run(
                    Run::new()
                        .add_text(&meta.subtitle)
                        .size(26)
                        .color(GREY)
                        .fonts(head_fonts()),
                ),
            );
        }
        doc = doc.add_paragraph(page_break());
    } else {
        doc = title_page(doc, meta);
        doc = disclaimer_page(doc, meta);
        doc = inscription_page(doc, meta);
    }
    doc = doc.add_paragraph(
        Paragraph::new().add_run(
            Run::new()
                .add_text("Contents")
                .bold()
                .size(44)
                .color(NAVY)
                .fonts(head_fonts()),
        ),
    );
    doc = doc.add_table_of_contents(TableOfContents::new().heading_styles_range(1, 3).auto());
    doc = doc.add_paragraph(page_break());

    // Build the per-book context: built-in index terms + any book-specific ones.
    let mut index_terms: Vec<String> = INDEX_TERMS.iter().map(|s| (*s).to_string()).collect();
    index_terms.extend(meta.index_terms.iter().cloned());
    let mut ctx = Ctx {
        figdir,
        lang: &meta.lang,
        figno: 0,
        tblno: 0,
        chapno: 0,
        idx_seen: std::collections::HashSet::new(),
        index_terms,
        links: Vec::new(),
    };

    for (ci, (_label, md)) in chapters.iter().enumerate() {
        let blocks = fold_table_captions(to_docx_blocks(md));
        let numbered = chapter_is_numbered(md, meta.thesis_profile);
        let mut first = true;
        for b in &blocks {
            doc = render_block(doc, b, &mut ctx, first && ci > 0, numbered);
            first = false;
        }
        // End-of-chapter Sources & QR-codes box (bookkit flush_sources).
        doc = flush_sources(doc, &mut ctx.links, &meta.lang);
    }

    // Appendix lists (filled from the caption SEQ fields on field update).
    doc = doc.add_paragraph(page_break());
    // `seq` (SEQ field name) stays English/stable for numbering; only the
    // visible heading is localised.
    for p in list_of("Figure", t(&meta.lang, "list_of_figures")) {
        doc = doc.add_paragraph(p);
    }
    for p in list_of("Table", t(&meta.lang, "list_of_tables")) {
        doc = doc.add_paragraph(p);
    }

    // Back-of-book index: the INDEX field, filled from XE entries on update.
    doc = doc.add_paragraph(page_break());
    doc = doc.add_paragraph(
        Paragraph::new().style("Heading1").add_run(
            Run::new()
                .add_text("Index")
                .bold()
                .size(32)
                .color(NAVY)
                .fonts(head_fonts()),
        ),
    );
    doc = doc.add_paragraph(
        Paragraph::new().add_run(
            Run::new()
                .add_text("Right-click and choose \u{201c}Update Field\u{201d} to build the index.")
                .italic()
                .size(18)
                .color(GREY)
                .fonts(body_fonts()),
        ),
    );
    doc = doc.add_paragraph(Paragraph::new().add_run(field_run("INDEX \\c 2", "")));

    let mut cur = Cursor::new(Vec::<u8>::new());
    doc.build().pack(&mut cur).context("pack book docx")?;
    Ok(cur.into_inner())
}

/// FHNW thesis front/back-matter slot a chapter belongs to, decided by its first
/// H1 (`specs/overrides/fhnw-mas/thesis-structure.md`). `Body` = numbered ch. 1-7.
#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
enum ThesisSlot {
    MgmtSummary,
    Declaration,
    Acronyms,
    Bibliography,
    AiTools,
    Appendix,
    Body,
}

/// One emitted item in the FHNW thesis layout: either a chapter (by index into
/// the input `chapters`) or a generated structural section.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum ThesisItem {
    Chapter(usize),
    Toc,
    ListFigures,
    ListTables,
}

/// Classify a chapter by its first H1 into an FHNW front/back-matter slot.
fn thesis_slot(md: &str) -> ThesisSlot {
    let h1 = first_h1(md).unwrap_or_default().to_lowercase();
    let h = h1.trim();
    if h.contains("management summary") || h.contains("executive summary") {
        ThesisSlot::MgmtSummary
    } else if h.contains("ehrenwörtliche erklärung")
        || h.contains("eidesstattliche")
        || h.contains("declaration of authorship")
        || h == "declaration"
    {
        ThesisSlot::Declaration
    } else if h.contains("acronym") || h.contains("abbreviation") || h.contains("abkürzungsverz") {
        ThesisSlot::Acronyms
    } else if h.contains("bibliography") || h.contains("references") || h.contains("literaturverz")
    {
        ThesisSlot::Bibliography
    } else if h.contains("hilfsmittel")
        || h.contains("ai tools")
        || h.contains("ai-tools")
        || h.contains("tools and databases")
        || h.contains("declaration of tools")
    {
        ThesisSlot::AiTools
    } else if h.contains("appendix") || h.contains("anhang") {
        ThesisSlot::Appendix
    } else {
        ThesisSlot::Body
    }
}

/// Compute the FHNW-mandated emission order (pure; unit-tested separately):
///   Management Summary → Declaration → Table of Contents → List of Figures →
///   List of Tables → Acronyms → numbered body (1-7) → Bibliography →
///   Tools/AI disclosure → Appendix.
/// Order within each slot follows the input (manifest) order. Slots with no
/// chapters simply contribute nothing; the TOC and the figure/table lists are
/// always emitted (matching the book engine's "always present" chrome).
fn thesis_layout(chapters: &[(String, String)]) -> Vec<ThesisItem> {
    let mut by_slot: std::collections::HashMap<ThesisSlot, Vec<usize>> =
        std::collections::HashMap::new();
    for (i, (_label, md)) in chapters.iter().enumerate() {
        by_slot.entry(thesis_slot(md)).or_default().push(i);
    }
    let take = |slot: ThesisSlot| -> Vec<ThesisItem> {
        by_slot
            .get(&slot)
            .into_iter()
            .flatten()
            .map(|&i| ThesisItem::Chapter(i))
            .collect()
    };
    let mut out = Vec::new();
    out.extend(take(ThesisSlot::MgmtSummary));
    out.extend(take(ThesisSlot::Declaration));
    out.push(ThesisItem::Toc);
    out.push(ThesisItem::ListFigures);
    out.push(ThesisItem::ListTables);
    out.extend(take(ThesisSlot::Acronyms));
    out.extend(take(ThesisSlot::Body));
    out.extend(take(ThesisSlot::Bibliography));
    out.extend(take(ThesisSlot::AiTools));
    out.extend(take(ThesisSlot::Appendix));
    out
}

/// Render one thesis chapter: optional leading page-break + blocks + the
/// end-of-chapter Sources box. Numbering is decided per-chapter (numbered body
/// chapters only fire their chapter number when `page_break_before` is true).
fn render_thesis_chapter(
    mut doc: Docx,
    md: &str,
    meta: &BookMeta,
    ctx: &mut Ctx,
    page_break_before: bool,
) -> Docx {
    let blocks = fold_table_captions(to_docx_blocks(md));
    let numbered = chapter_is_numbered(md, meta.thesis_profile);
    let mut first = true;
    for b in &blocks {
        doc = render_block(doc, b, ctx, first && page_break_before, numbered);
        first = false;
    }
    flush_sources(doc, &mut ctx.links, &meta.lang)
}

/// Render the FHNW master-thesis profile (bookkit C, ADR-0045) in the mandated
/// reading order (`thesis-structure.md`): Title page → Management Summary →
/// Declaration → Table of Contents → List of Figures → List of Tables →
/// Acronyms → numbered body → Bibliography → Tools/AI disclosure → Appendix.
/// No book-style disclaimer/inscription pages and no back-of-book index — those
/// are not part of the FHNW thesis structure.
fn render_thesis_book(
    meta: &BookMeta,
    chapters: &[(String, String)],
    figdir: &Path,
) -> Result<Vec<u8>> {
    let mut doc = with_styles(
        Docx::new()
            .default_fonts(body_fonts())
            .default_size(22)
            .page_size(11906, 16838)
            .page_margin(std_margin()),
    )
    .footer(
        Footer::new().add_paragraph(
            Paragraph::new()
                .align(AlignmentType::Center)
                .add_page_num(PageNum::new()),
        ),
    );

    doc = title_page(doc, meta);

    let mut index_terms: Vec<String> = INDEX_TERMS.iter().map(|s| (*s).to_string()).collect();
    index_terms.extend(meta.index_terms.iter().cloned());
    let mut ctx = Ctx {
        figdir,
        lang: &meta.lang,
        figno: 0,
        tblno: 0,
        chapno: 0,
        idx_seen: std::collections::HashSet::new(),
        index_terms,
        links: Vec::new(),
    };

    // `emitted` tracks whether any flow content precedes the next item, so the
    // first front-matter chapter does not get a redundant page break (the title
    // page already ends with one).
    let mut emitted = false;
    for item in thesis_layout(chapters) {
        match item {
            ThesisItem::Chapter(i) => {
                doc = render_thesis_chapter(doc, &chapters[i].1, meta, &mut ctx, emitted);
                emitted = true;
            }
            ThesisItem::Toc => {
                if emitted {
                    doc = doc.add_paragraph(page_break());
                }
                doc = doc.add_paragraph(
                    Paragraph::new().add_run(
                        Run::new()
                            .add_text("Contents")
                            .bold()
                            .size(44)
                            .color(NAVY)
                            .fonts(head_fonts()),
                    ),
                );
                doc = doc.add_table_of_contents(
                    TableOfContents::new().heading_styles_range(1, 3).auto(),
                );
                emitted = true;
            }
            ThesisItem::ListFigures => {
                doc = doc.add_paragraph(page_break());
                for p in list_of("Figure", t(&meta.lang, "list_of_figures")) {
                    doc = doc.add_paragraph(p);
                }
            }
            ThesisItem::ListTables => {
                for p in list_of("Table", t(&meta.lang, "list_of_tables")) {
                    doc = doc.add_paragraph(p);
                }
            }
        }
    }

    let mut cur = Cursor::new(Vec::<u8>::new());
    doc.build().pack(&mut cur).context("pack thesis docx")?;
    Ok(cur.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_book_with_table_and_heading() {
        let meta = BookMeta {
            title: "T".into(),
            subtitle: "S".into(),
            author: "A".into(),
            context: "C".into(),
            ..Default::default()
        };
        let md = "# Chapter\n\nA **bold** paragraph.\n\n| H1 | H2 |\n|----|----|\n| a | b |\n"
            .to_string();
        let bytes = render_book(&meta, &[("c1".into(), md)], Path::new(".")).unwrap();
        assert_eq!(&bytes[..4], b"PK\x03\x04");
        assert!(bytes.len() > 2000);
    }

    #[test]
    fn front_matter_unnumbered_dimension_numbered() {
        // Front/back-matter H1s render UNNUMBERED (exact or starts-with match,
        // case-insensitive); a real dimension chapter stays numbered.
        assert!(!chapter_is_numbered("# Foreword\n\nText.\n", false));
        assert!(!chapter_is_numbered(
            "# Appendix: The Research Prompts\n\nText.\n",
            false
        ));
        assert!(!chapter_is_numbered(
            "# Acronyms and Abbreviations\n",
            false
        ));
        assert!(!chapter_is_numbered("# List of Figures\n", false));
        assert!(chapter_is_numbered(
            "# Dimension 06 — Quantum Computing\n\nText.\n",
            false
        ));
    }

    #[test]
    fn companion_renders_without_book_chrome() {
        // Companion profile still produces a valid docx (plain title, no
        // title/disclaimer/inscription pages).
        let meta = BookMeta {
            title: "Student Notes".into(),
            disclaimer: Some("should be skipped in companion mode".into()),
            companion: true,
            ..Default::default()
        };
        let md = "# Overview\n\nSome synthesis text.\n".to_string();
        let bytes = render_book(&meta, &[("c1".into(), md)], Path::new(".")).unwrap();
        assert_eq!(&bytes[..4], b"PK\x03\x04");
        assert!(bytes.len() > 1500);
    }

    /// The master_thesis bookkit (thesis_profile) chapter set, mirroring the
    /// manifest order: Management Summary, Acronyms, the seven numbered body
    /// chapters, two appendices, then the bibliography.
    fn master_thesis_chapters() -> Vec<(String, String)> {
        [
            ("mgmt", "# Management Summary\n\nThe one-page summary.\n"),
            (
                "acro",
                "# Acronyms and Abbreviations\n\nAI — Artificial Intelligence.\n",
            ),
            ("c1", "# Introduction\n\nBackground.\n"),
            ("c2", "# Theory\n\nState of the art.\n"),
            ("c3", "# Current-State Analysis\n\nIST.\n"),
            ("c4", "# Empirical Study\n\nData.\n"),
            ("c5", "# Solution\n\nDesign.\n"),
            ("c6", "# Conclusion\n\nFindings.\n"),
            ("c7", "# Personal Reflection\n\nLessons learned.\n"),
            ("apx1", "# Appendix: Research Prompts\n\nPrompts.\n"),
            ("apx2", "# Appendix A — Transformation Plan\n\nPlan.\n"),
            ("bib", "# Bibliography\n\nDoe, J. (2026).\n"),
        ]
        .into_iter()
        .map(|(l, m)| (l.to_string(), m.to_string()))
        .collect()
    }

    #[test]
    fn thesis_layout_follows_fhnw_front_back_matter_order() {
        // The FHNW order (thesis-structure.md): Management Summary → Table of
        // Contents → List of Figures → List of Tables → Acronyms → numbered body
        // (1-7) → Bibliography → Appendix. (No Declaration / Tools chapters in
        // this manifest, so those slots contribute nothing.)
        let chapters = master_thesis_chapters();
        let layout = thesis_layout(&chapters);
        let expected = vec![
            ThesisItem::Chapter(0),  // Management Summary
            ThesisItem::Toc,         // Table of Contents (BEFORE body — the fix)
            ThesisItem::ListFigures, // front-matter, after the TOC
            ThesisItem::ListTables,
            ThesisItem::Chapter(1),  // Acronyms (last front-matter item)
            ThesisItem::Chapter(2),  // 1 Introduction
            ThesisItem::Chapter(3),  // 2 Theory
            ThesisItem::Chapter(4),  // 3 Current-State Analysis
            ThesisItem::Chapter(5),  // 4 Empirical Study
            ThesisItem::Chapter(6),  // 5 Solution
            ThesisItem::Chapter(7),  // 6 Conclusion
            ThesisItem::Chapter(8),  // 7 Personal Reflection
            ThesisItem::Chapter(11), // Bibliography (back matter, before Appendix)
            ThesisItem::Chapter(9),  // Appendix: Research Prompts
            ThesisItem::Chapter(10), // Appendix A
        ];
        assert_eq!(layout, expected);

        // Explicit guard for the original bug: Management Summary precedes the TOC.
        let ms = layout
            .iter()
            .position(|i| *i == ThesisItem::Chapter(0))
            .unwrap();
        let toc = layout.iter().position(|i| *i == ThesisItem::Toc).unwrap();
        assert!(
            ms < toc,
            "Management Summary must come before the Table of Contents"
        );
    }

    #[test]
    fn thesis_slot_classification() {
        assert_eq!(
            thesis_slot("# Management Summary\n"),
            ThesisSlot::MgmtSummary
        );
        assert_eq!(
            thesis_slot("# Acronyms and Abbreviations\n"),
            ThesisSlot::Acronyms
        );
        assert_eq!(thesis_slot("# Bibliography\n"), ThesisSlot::Bibliography);
        assert_eq!(thesis_slot("# Appendix: Prompts\n"), ThesisSlot::Appendix);
        assert_eq!(
            thesis_slot("# Ehrenwörtliche Erklärung\n"),
            ThesisSlot::Declaration
        );
        assert_eq!(thesis_slot("# Introduction\n"), ThesisSlot::Body);
        assert_eq!(thesis_slot("# Theory\n"), ThesisSlot::Body);
    }

    #[test]
    fn thesis_profile_renders_valid_docx() {
        // End-to-end: the master_thesis bookkit renders a valid (PK-zip) DOCX
        // via the thesis path, with the full 12-chapter set.
        let meta = BookMeta {
            title: "Governance and Leadership…".into(),
            subtitle: "FHNW MAS Cybersecurity — Master's Thesis".into(),
            author: "Daniel Casota".into(),
            context: "MAS Cybersecurity, FHNW".into(),
            thesis_profile: true,
            ..Default::default()
        };
        let bytes = render_book(&meta, &master_thesis_chapters(), Path::new(".")).unwrap();
        assert_eq!(&bytes[..4], b"PK\x03\x04", "valid docx zip");
        assert!(bytes.len() > 3000, "non-trivial document");
    }

    #[test]
    fn non_thesis_profiles_keep_book_layout() {
        // Guard: with thesis_profile = false the book path is taken (TOC-first
        // book layout), proving the thesis branch is isolated to bookkit C.
        let meta = BookMeta {
            title: "Merged Dimensions".into(),
            thesis_profile: false,
            ..Default::default()
        };
        let bytes = render_book(
            &meta,
            &[(
                "c1".into(),
                "# Dimension 01 — Agile Leadership\n\nText.\n".into(),
            )],
            Path::new("."),
        )
        .unwrap();
        assert_eq!(&bytes[..4], b"PK\x03\x04");
    }

    /// The dimensions book (bookkit A) chapter set, mirroring the manifest:
    /// front matter, the merged dimensions body (H1 per dimension, with H2/H3
    /// sub-sections so the 1-3 level TOC has depth), appendix, bibliography.
    fn dimensions_book_chapters() -> Vec<(String, String)> {
        [
            ("foreword", "# Foreword\n\nText.\n"),
            ("ack", "# Acknowledgements\n\nThanks.\n"),
            ("about", "# About this Book\n\nScope.\n"),
            ("preface", "# Preface\n\nWhy.\n"),
            (
                "acro",
                "# Acronyms and Abbreviations\n\nAI — Artificial Intelligence.\n",
            ),
            ("intro", "# Introduction\n\nIntro.\n"),
            (
                "dims",
                "# Dimension 01 — Agile Leadership\n\n## 1.1 Overview\n\nText.\n\n\
                 ### 1.1.1 Detail\n\nText.\n\n# Dimension 02 — Cybersecurity and AI\n\n\
                 ## 2.1 Overview\n\nText.\n",
            ),
            ("appx", "# Appendix: Research Prompts\n\nPrompts.\n"),
            ("bib", "# Bibliography\n\nDoe, J. (2026). A work.\n"),
        ]
        .into_iter()
        .map(|(l, m)| (l.to_string(), m.to_string()))
        .collect()
    }

    #[test]
    fn dimensions_book_enforces_auto_toc_levels_1_3() {
        // Bookkit A (no thesis_profile, no companion) must render the engine's
        // dedicated auto Table of Contents over heading levels 1-3 — the spec in
        // ADR-0030 ("auto TOC over heading levels 1–3") and ADR-0045 ("Table of
        // Contents (engine)"). Verified against the emitted Word field, not by
        // proxy.
        let meta = BookMeta {
            title: "Governance and Leadership…".into(),
            subtitle: "A Cross-Dimensional Field Guide".into(),
            author: "Daniel Casota".into(),
            context: "MAS Cybersecurity, FHNW".into(),
            disclaimer: Some("First researched edition.".into()),
            ..Default::default() // thesis_profile = false, companion = false
        };
        let bytes = render_book(&meta, &dimensions_book_chapters(), Path::new(".")).unwrap();
        assert_eq!(&bytes[..4], b"PK\x03\x04", "valid docx zip");

        use std::io::Read;
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut xml = String::new();
        zip.by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut xml)
            .unwrap();

        // The dedicated dimensions-book TOC: a Word TOC field over levels 1-3.
        assert!(
            xml.contains(r#"TOC \o &quot;1-3&quot;"#),
            "auto TOC over heading levels 1-3 must be present (the dedicated dimensions-book TOC)"
        );
        // The merged body's dimension H1s populate that TOC.
        assert!(
            xml.contains("Dimension 01"),
            "dimension chapters present in body/TOC"
        );
        // Book-path-only chrome the thesis path drops — proves bookkit A took the
        // BOOK layout, not the thesis layout (this test is solely the A profile).
        assert!(
            xml.contains("INDEX"),
            "books profile emits the back-of-book Index field"
        );
    }

    #[test]
    fn thesis_profile_numbers_body_chapters() {
        // Book profile: Introduction is unnumbered front-matter.
        assert!(!chapter_is_numbered("# Introduction\n\nText.\n", false));
        // Thesis profile: Introduction/Theory/Conclusion are numbered chapters…
        assert!(chapter_is_numbered("# Introduction\n\nText.\n", true));
        assert!(chapter_is_numbered("# Theory\n\nText.\n", true));
        assert!(chapter_is_numbered("# Conclusion\n\nText.\n", true));
        assert!(chapter_is_numbered(
            "# Personal Reflection\n\nText.\n",
            true
        ));
        // …but Management Summary / Acronyms / Appendix / Bibliography stay front/back-matter.
        assert!(!chapter_is_numbered(
            "# Management Summary\n\nText.\n",
            true
        ));
        assert!(!chapter_is_numbered("# Acronyms and Abbreviations\n", true));
        assert!(!chapter_is_numbered(
            "# Appendix: The Research Prompts\n",
            true
        ));
        assert!(!chapter_is_numbered("# Bibliography\n", true));
    }

    #[test]
    fn field_instructions_are_xml_escaped() {
        use std::io::Read;
        let meta = BookMeta {
            title: "T".into(),
            subtitle: String::new(),
            author: "A".into(),
            context: "C".into(),
            ..Default::default()
        };
        // "MITRE ATT&CK" is a curated index term; its XE field must escape '&'.
        let md = "# C\n\nThe MITRE ATT&CK framework catalogues adversary tactics.\n".to_string();
        let bytes = render_book(&meta, &[("c1".into(), md)], Path::new(".")).unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut xml = String::new();
        zip.by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut xml)
            .unwrap();
        assert!(xml.contains("ATT&amp;CK"), "field instr must be escaped");
        // no raw ampersand that isn't an entity
        assert!(
            !regex_lite_has_raw_amp(&xml),
            "document.xml has a raw unescaped '&'"
        );
    }

    fn regex_lite_has_raw_amp(s: &str) -> bool {
        let b = s.as_bytes();
        for (i, &c) in b.iter().enumerate() {
            if c == b'&' {
                let tail = &s[i + 1..];
                let ok = ["amp;", "lt;", "gt;", "quot;", "apos;", "#"]
                    .iter()
                    .any(|e| tail.starts_with(e));
                if !ok {
                    return true;
                }
            }
        }
        false
    }

    #[test]
    fn admonition_renders() {
        let meta = BookMeta {
            title: "T".into(),
            subtitle: String::new(),
            author: "A".into(),
            context: "C".into(),
            ..Default::default()
        };
        let md = "# C\n\n```warning\nThe EU AI Act has extraterritorial reach.\n```\n".to_string();
        let bytes = render_book(&meta, &[("c1".into(), md)], Path::new(".")).unwrap();
        assert_eq!(&bytes[..4], b"PK\x03\x04");
        assert!(bytes.len() > 2000);
    }

    #[test]
    fn wide_table_emits_landscape_section() {
        let meta = BookMeta {
            title: "T".into(),
            subtitle: String::new(),
            author: "A".into(),
            context: "C".into(),
            ..Default::default()
        };
        // 7 columns ⇒ at/above LANDSCAPE_COLS ⇒ rendered on a landscape page.
        let header: Vec<String> = (1..=7).map(|i| format!("H{i}")).collect();
        let row: Vec<String> = (1..=7).map(|i| format!("c{i}")).collect();
        let docx = render_book_to_docx(&meta, &header, &row);
        // Unzip document.xml and assert a landscape sectPr is present.
        let mut zip = zip::ZipArchive::new(Cursor::new(docx)).unwrap();
        let mut xml = String::new();
        {
            use std::io::Read;
            zip.by_name("word/document.xml")
                .unwrap()
                .read_to_string(&mut xml)
                .unwrap();
        }
        assert!(
            xml.contains("orient=\"landscape\""),
            "wide table should produce a landscape section"
        );
    }

    fn doc_xml(bytes: Vec<u8>) -> String {
        use std::io::Read;
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut xml = String::new();
        zip.by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut xml)
            .unwrap();
        xml
    }

    #[test]
    fn ordered_list_gets_real_numerals_and_sources_box() {
        let meta = BookMeta {
            title: "T".into(),
            author: "A".into(),
            ..Default::default()
        };
        let md = "# Chapter\n\n1. First\n2. Second\n3. Third\n\nSee [the site](https://example.com/x).\n"
            .to_string();
        let bytes = render_book(&meta, &[("c1".into(), md)], Path::new(".")).unwrap();
        let xml = doc_xml(bytes.clone());
        // real running numerals, not a static en-dash
        assert!(
            xml.contains("2.  "),
            "ordered list should show real numerals"
        );
        assert!(xml.contains("3.  "));
        // per-chapter Sources & QR box (note: '&' is XML-escaped)
        assert!(
            xml.contains("Sources &amp; QR codes"),
            "links should produce a Sources box"
        );
        // a QR PNG was embedded into the package media
        let zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        assert!(
            zip.file_names().any(|n| n.starts_with("word/media/")),
            "a QR image should be embedded"
        );
    }

    #[test]
    fn table_caption_is_folded_and_numbered() {
        let meta = BookMeta {
            title: "T".into(),
            author: "A".into(),
            ..Default::default()
        };
        let md =
            "# Chapter\n\nTable: Demo caption\n\n| A | B |\n|---|---|\n| 1 | 2 |\n".to_string();
        let xml = doc_xml(render_book(&meta, &[("c1".into(), md)], Path::new(".")).unwrap());
        assert!(xml.contains("Demo caption"), "table caption text present");
        assert!(
            xml.contains("SEQ Table"),
            "table caption carries a SEQ Table field"
        );
    }

    #[test]
    fn quote_and_chapter_number_render() {
        let meta = BookMeta {
            title: "T".into(),
            author: "A".into(),
            ..Default::default()
        };
        // ci>0 so the chapter-number prefix applies to a numbered chapter.
        let intro = "# Foreword\n\nWelcome.\n".to_string(); // unnumbered
        let body = "# Real Chapter\n\n```quote\nThe machine must be governed.\n— Kranzberg\n```\n"
            .to_string();
        let xml = doc_xml(
            render_book(
                &meta,
                &[("c0".into(), intro), ("c1".into(), body)],
                Path::new("."),
            )
            .unwrap(),
        );
        assert!(xml.contains("The machine must be governed."));
        assert!(xml.contains("Kranzberg"));
        assert!(
            xml.contains("1  Real Chapter"),
            "numbered chapter gets an N prefix"
        );
    }

    #[test]
    fn german_chrome_localises_while_english_chrome_absent() {
        let meta = BookMeta {
            title: "T".into(),
            author: "A".into(),
            lang: "de".into(),
            ..Default::default()
        };
        // Figure + table + a warning admonition + a link (Sources box) exercise
        // every localised chrome site.
        let md = "# Kapitel\n\n```warning\nGefahr.\n```\n\nTable: Demo\n\n| A | B |\n|---|---|\n| 1 | 2 |\n\nSiehe [Quelle](https://example.com/x).\n"
            .to_string();
        let xml = doc_xml(render_book(&meta, &[("c1".into(), md)], Path::new(".")).unwrap());
        // Localised chrome present.
        assert!(xml.contains("Abbildungsverzeichnis"), "fig list heading de");
        assert!(xml.contains("Tabellenverzeichnis"), "table list heading de");
        assert!(xml.contains("Tabelle "), "table caption prefix de");
        assert!(xml.contains("Warnung"), "warning admonition label de");
        // '&' is XML-escaped: "Quellen & QR-Codes" → "Quellen &amp; QR-Codes".
        assert!(xml.contains("Quellen &amp; QR-Codes"), "sources box de");
        // English chrome must be gone.
        assert!(
            !xml.contains("Sources &amp; QR codes"),
            "english sources chrome leaked"
        );
        assert!(
            !xml.contains("Edition &amp; Disclaimer"),
            "english disclaimer chrome leaked"
        );
        assert!(
            !xml.contains("List of Figures") && !xml.contains("List of Tables"),
            "english list headings leaked"
        );
        // SEQ field NAMES stay English/stable for numbering.
        assert!(
            xml.contains("SEQ Table"),
            "SEQ Table field name stays english"
        );
    }

    #[test]
    fn english_chrome_unchanged_by_default() {
        let meta = BookMeta {
            title: "T".into(),
            author: "A".into(),
            // lang left empty → English.
            ..Default::default()
        };
        let md = "# Chapter\n\nTable: Demo\n\n| A | B |\n|---|---|\n| 1 | 2 |\n\nSee [src](https://example.com/x).\n"
            .to_string();
        let xml = doc_xml(render_book(&meta, &[("c1".into(), md)], Path::new(".")).unwrap());
        assert!(xml.contains("List of Figures"), "english fig list heading");
        assert!(xml.contains("List of Tables"), "english table list heading");
        assert!(
            xml.contains("Sources &amp; QR codes"),
            "english sources box"
        );
        assert!(xml.contains("Table "), "english table caption prefix");
        assert!(!xml.contains("Abbildungsverzeichnis"), "no german leak");
    }

    fn render_book_to_docx(meta: &BookMeta, header: &[String], row: &[String]) -> Vec<u8> {
        let head = header.join(" | ");
        let sep = vec!["---"; header.len()].join(" | ");
        let cells = row.join(" | ");
        let md = format!("# Chapter\n\n| {head} |\n| {sep} |\n| {cells} |\n");
        render_book(meta, &[("c1".into(), md)], Path::new(".")).unwrap()
    }
}
