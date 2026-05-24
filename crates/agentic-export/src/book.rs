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
    AlignmentType, BreakType, Docx, Footer, HeightRule, LineSpacing, LineSpacingType, PageMargin,
    PageNum, PageOrientationType, PageSize, Paragraph, Pic, Run, RunFonts, SectionProperty,
    Shading, Style, StyleType, Table, TableCell, TableCellMargins, TableLayoutType,
    TableOfContents, TableRow, TextDirectionType, VAlignType, WidthType,
};

use crate::markdown::{DocxBlock, DocxRun, to_docx_blocks};

const NAVY: &str = "1F497D";
const HEAD2: &str = "2E4A7A";
const GREY: &str = "666666";
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

fn para_of(runs: &[DocxRun]) -> Paragraph {
    let mut p = Paragraph::new().line_spacing(body_spacing());
    for r in runs {
        p = p.add_run(run_of(r));
    }
    p
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

fn render_block(
    mut doc: Docx,
    b: &DocxBlock,
    figdir: &Path,
    figno: &mut u32,
    chapter_start: bool,
) -> Docx {
    match b {
        DocxBlock::Heading { level, text } => {
            doc.add_paragraph(heading_para(*level, text, chapter_start && *level <= 2))
        }
        DocxBlock::Paragraph(runs) => doc.add_paragraph(para_of(runs)),
        DocxBlock::BulletItem(runs) => {
            let mut p = Paragraph::new().line_spacing(body_spacing()).add_run(
                Run::new()
                    .add_text("•  ")
                    .size(22)
                    .color(NAVY)
                    .bold()
                    .fonts(body_fonts()),
            );
            for r in runs {
                p = p.add_run(run_of(r));
            }
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
            for r in runs {
                p = p.add_run(run_of(r));
            }
            doc.add_paragraph(p)
        }
        DocxBlock::CodeBlock { body, .. } => {
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
                doc.add_paragraph(
                    Paragraph::new()
                        .align(AlignmentType::Center)
                        .line_spacing(LineSpacing::new().after(SPACE_AROUND_FIG))
                        .add_run(
                            Run::new()
                                .add_text(format!("Figure {}. {caption}", *figno))
                                .italic()
                                .size(18)
                                .color(GREY)
                                .fonts(body_fonts()),
                        ),
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

    let mut figno = 0u32;
    for (ci, (_label, md)) in chapters.iter().enumerate() {
        let blocks = to_docx_blocks(md);
        let mut first = true;
        for b in &blocks {
            doc = render_block(doc, b, figdir, &mut figno, first && ci > 0);
            first = false;
        }
    }

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
