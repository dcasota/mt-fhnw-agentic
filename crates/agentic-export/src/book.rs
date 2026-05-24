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

use crate::markdown::{DocxBlock, DocxRun, to_docx_blocks};

const NAVY: &str = "1F497D";
const HEAD2: &str = "2E4A7A";
const GREY: &str = "666666";
const ACCENT: &str = "0B5C9E"; // hyperlink blue
const HEADBG: &str = "1F3864";
const ALTBG: &str = "F4F6FA";
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

#[derive(Debug, Clone)]
pub struct BookMeta {
    pub title: String,
    pub subtitle: String,
    pub author: String,
    pub context: String,
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

/// A blue, underlined run for hyperlink text.
fn link_run(r: &DocxRun) -> Run {
    let mut run = Run::new()
        .add_text(&r.text)
        .size(22)
        .color(ACCENT)
        .underline("single")
        .fonts(body_fonts());
    if r.bold {
        run = run.bold();
    }
    run
}

/// Add a run sequence to a paragraph, emitting clickable hyperlinks for any run
/// that carries a URL (markdown `[label](url)`).
fn add_runs(mut p: Paragraph, runs: &[DocxRun]) -> Paragraph {
    for r in runs {
        if let Some(url) = &r.link {
            p = p.add_hyperlink(Hyperlink::new(url, HyperlinkType::External).add_run(link_run(r)));
        } else {
            p = p.add_run(run_of(r));
        }
    }
    p
}

fn para_of(runs: &[DocxRun]) -> Paragraph {
    add_runs(Paragraph::new().line_spacing(body_spacing()), runs)
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
fn admonition_box(mut doc: Docx, kind: &str, body: &str) -> Docx {
    let (label, fill, edge) = match kind {
        "tip" => ("\u{2714} Tip", "EAF6EC", "2E7D32"),
        "warning" => ("\u{26A0} Warning", "FBF1E2", "C77F18"),
        _ => ("\u{2139} Note", "EAF1FB", "1F3864"),
    };
    let text: String = body
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
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
        .add_paragraph(
            Paragraph::new()
                .line_spacing(body_spacing())
                .add_run(
                    Run::new()
                        .add_text(format!("{label}  "))
                        .bold()
                        .size(21)
                        .color(edge)
                        .fonts(head_fonts()),
                )
                .add_run(Run::new().add_text(text).size(22).fonts(body_fonts())),
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

/// A Word field `{ instr }` with a cached display value — lets us emit arbitrary
/// fields (SEQ, TOC \c, XE, INDEX) that docx-rs has no builder for.
fn field_run(instr: &str, cached: &str) -> Run {
    let mut r = Run::new()
        .add_field_char(FieldCharType::Begin, false)
        .add_instr_text(InstrText::Unsupported(instr.to_string()))
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
];

/// XE index-entry field runs for any curated term first seen in `text`.
fn index_marks(text: &str, seen: &mut std::collections::HashSet<String>) -> Vec<Run> {
    let lower = text.to_lowercase();
    let mut out = Vec::new();
    for term in INDEX_TERMS {
        if !seen.contains(*term) && lower.contains(&term.to_lowercase()) {
            seen.insert((*term).to_string());
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

fn render_block(
    mut doc: Docx,
    b: &DocxBlock,
    figdir: &Path,
    figno: &mut u32,
    chapter_start: bool,
    idx_seen: &mut std::collections::HashSet<String>,
) -> Docx {
    match b {
        DocxBlock::Heading { level, text } => {
            doc.add_paragraph(heading_para(*level, text, chapter_start && *level <= 2))
        }
        DocxBlock::Paragraph(runs) => {
            let mut p = para_of(runs);
            let text: String = runs.iter().map(|r| r.text.as_str()).collect();
            for xe in index_marks(&text, idx_seen) {
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
            p = add_runs(p, runs);
            doc.add_paragraph(p)
        }
        DocxBlock::OrderedItem(runs) => {
            let mut p = Paragraph::new().line_spacing(body_spacing()).add_run(
                Run::new()
                    .add_text("–  ")
                    .size(22)
                    .color(NAVY)
                    .bold()
                    .fonts(body_fonts()),
            );
            p = add_runs(p, runs);
            doc.add_paragraph(p)
        }
        DocxBlock::CodeBlock { lang, body } => match lang.as_str() {
            // chapter_extras.py port: the "Key topics at a glance" box.
            "keypoints" => keypoints_box(doc, body),
            // chapter_extras.py port: the per-chapter "Review questions".
            "quiz" => quiz_block(doc, body),
            // bookkit.py port: note / tip / warning admonition callouts.
            "note" | "tip" | "warning" => admonition_box(doc, lang, body),
            // bookkit.py port: a generic titled key-point callout box.
            "callout" => callout_box(doc, body),
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
        DocxBlock::HorizontalRule => doc.add_paragraph(
            Paragraph::new().add_run(Run::new().add_text("────────────").color(GREY)),
        ),
        DocxBlock::Table { header, rows } => {
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
            let full = figdir.join(path.replace('/', std::path::MAIN_SEPARATOR_STR));
            if let Ok(bytes) = std::fs::read(&full) {
                *figno += 1;
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
                        .add_run(cap_style("Figure "))
                        .add_run(field_run("SEQ Figure \\* ARABIC", &format!("{}", *figno)))
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

pub fn render_book(
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
    // List of Figures (filled from the caption SEQ fields on field update).
    for p in list_of("Figure", "List of Figures") {
        doc = doc.add_paragraph(p);
    }
    doc = doc.add_paragraph(page_break());

    let mut figno = 0u32;
    let mut idx_seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (ci, (_label, md)) in chapters.iter().enumerate() {
        let blocks = to_docx_blocks(md);
        let mut first = true;
        for b in &blocks {
            doc = render_block(doc, b, figdir, &mut figno, first && ci > 0, &mut idx_seen);
            first = false;
        }
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
    doc = doc.add_paragraph(Paragraph::new().add_run(field_run("INDEX \\c 2 \\z 1031", "")));

    let mut cur = Cursor::new(Vec::<u8>::new());
    doc.build().pack(&mut cur).context("pack book docx")?;
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
        };
        let md = "# Chapter\n\nA **bold** paragraph.\n\n| H1 | H2 |\n|----|----|\n| a | b |\n"
            .to_string();
        let bytes = render_book(&meta, &[("c1".into(), md)], Path::new(".")).unwrap();
        assert_eq!(&bytes[..4], b"PK\x03\x04");
        assert!(bytes.len() > 2000);
    }

    #[test]
    fn admonition_renders() {
        let meta = BookMeta {
            title: "T".into(),
            subtitle: String::new(),
            author: "A".into(),
            context: "C".into(),
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

    fn render_book_to_docx(meta: &BookMeta, header: &[String], row: &[String]) -> Vec<u8> {
        let head = header.join(" | ");
        let sep = vec!["---"; header.len()].join(" | ");
        let cells = row.join(" | ");
        let md = format!("# Chapter\n\n| {head} |\n| {sep} |\n| {cells} |\n");
        render_book(meta, &[("c1".into(), md)], Path::new(".")).unwrap()
    }
}
