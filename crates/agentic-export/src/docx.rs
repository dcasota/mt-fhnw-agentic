//! DOCX renderer.
//!
//! Builds a [`docx_rs::Docx`] from a sequence of [`Chapter`]s. Each chapter
//! gets a top-level heading (the file's first H1, or the path stem), followed
//! by the markdown body translated to docx paragraphs.

use std::io::Cursor;

use anyhow::{Context, Result};
use docx_rs::{AlignmentType, BreakType, Docx, IndentLevel, NumberingId, Paragraph, Run, RunFonts};

use crate::collect::Chapter;
use crate::markdown::{DocxBlock, DocxRun, to_docx_blocks};

/// Render `chapters` to .docx bytes.
pub fn render(title: &str, chapters: &[Chapter]) -> Result<Vec<u8>> {
    let mut doc = Docx::new();

    // Title page-ish heading.
    doc = doc.add_paragraph(
        Paragraph::new()
            .style("Heading1")
            .align(AlignmentType::Center)
            .add_run(Run::new().add_text(title).bold().size(40)),
    );
    doc = doc.add_paragraph(Paragraph::new().add_run(Run::new().add_break(BreakType::Page)));

    for chapter in chapters {
        let blocks = to_docx_blocks(&chapter.body);
        for block in blocks {
            doc = match block {
                DocxBlock::Heading { level, text } => {
                    let style = match level {
                        1 => "Heading1",
                        2 => "Heading2",
                        3 => "Heading3",
                        _ => "Heading4",
                    };
                    let size = match level {
                        1 => 32,
                        2 => 28,
                        3 => 24,
                        _ => 22,
                    };
                    doc.add_paragraph(
                        Paragraph::new()
                            .style(style)
                            .add_run(Run::new().add_text(text).bold().size(size)),
                    )
                }
                DocxBlock::Paragraph(runs) => doc.add_paragraph(paragraph_from_runs(&runs)),
                DocxBlock::BulletItem(runs) => {
                    let p = paragraph_from_runs(&runs)
                        .numbering(NumberingId::new(2), IndentLevel::new(0));
                    doc.add_paragraph(p)
                }
                DocxBlock::OrderedItem { runs, .. } => {
                    let p = paragraph_from_runs(&runs)
                        .numbering(NumberingId::new(1), IndentLevel::new(0));
                    doc.add_paragraph(p)
                }
                DocxBlock::CodeBlock { lang: _, body } => {
                    let mut p = Paragraph::new().style("Code");
                    for (i, line) in body.split('\n').enumerate() {
                        if i > 0 {
                            p = p.add_run(Run::new().add_break(BreakType::TextWrapping));
                        }
                        p = p.add_run(code_run(line));
                    }
                    doc.add_paragraph(p)
                }
                DocxBlock::HorizontalRule => {
                    doc.add_paragraph(Paragraph::new().add_run(Run::new().add_text("───────────")))
                }
                DocxBlock::Table { header, rows, .. } => {
                    let mut d = doc;
                    if !header.is_empty() {
                        d = d.add_paragraph(
                            Paragraph::new()
                                .add_run(Run::new().add_text(header.join(" | ")).bold()),
                        );
                    }
                    for r in &rows {
                        d = d.add_paragraph(
                            Paragraph::new().add_run(Run::new().add_text(r.join(" | "))),
                        );
                    }
                    d
                }
                DocxBlock::Image { path, caption } => doc.add_paragraph(
                    Paragraph::new().add_run(
                        Run::new()
                            .add_text(format!("[figure {path}: {caption}]"))
                            .italic(),
                    ),
                ),
            };
        }
        // Page break between chapters.
        doc = doc.add_paragraph(Paragraph::new().add_run(Run::new().add_break(BreakType::Page)));
    }

    let mut cursor = Cursor::new(Vec::<u8>::new());
    doc.build()
        .pack(&mut cursor)
        .context("pack docx into byte buffer")?;
    Ok(cursor.into_inner())
}

fn paragraph_from_runs(runs: &[DocxRun]) -> Paragraph {
    let mut p = Paragraph::new();
    for r in runs {
        p = p.add_run(run_from(r));
    }
    p
}

fn run_from(r: &DocxRun) -> Run {
    let mut run = Run::new().add_text(&r.text);
    if r.bold {
        run = run.bold();
    }
    if r.italic {
        run = run.italic();
    }
    if r.code {
        run = run.fonts(RunFonts::new().ascii("Consolas"));
    }
    run
}

fn code_run(text: &str) -> Run {
    Run::new()
        .add_text(text)
        .fonts(RunFonts::new().ascii("Consolas"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_minimal_doc_without_panicking() {
        let chapter = Chapter {
            path: "ch-01.md".into(),
            body: "# Hi\n\nA **bold** word.\n".into(),
            lang: Some("en".into()),
        };
        let bytes = render("My Thesis", &[chapter]).unwrap();
        // .docx files are ZIP archives — they always start with PK\u{0003}\u{0004}.
        assert_eq!(&bytes[..4], b"PK\x03\x04");
        assert!(bytes.len() > 1024); // non-trivial
    }

    #[test]
    fn renders_empty_chapter_list() {
        let bytes = render("Empty", &[]).unwrap();
        assert_eq!(&bytes[..4], b"PK\x03\x04");
    }
}
