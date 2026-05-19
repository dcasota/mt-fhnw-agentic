//! PDF renderer (markdown → Typst markup → compiled PDF).
//!
//! Embeds typst as a library: we implement [`typst::World`] over an in-memory
//! source plus typst-kit's embedded fonts. No filesystem access, no
//! package resolution — purely deterministic from the input markdown.

use anyhow::{Result, anyhow};
use chrono::{Datelike, Utc};
use typst::Library;
use typst::diag::{FileError, FileResult};
use typst::foundations::{Bytes, Datetime};
use typst::syntax::{FileId, Source, VirtualPath};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst_kit::fonts::{FontSearcher, FontSlot};
use typst_pdf::PdfOptions;

use crate::collect::Chapter;
use crate::markdown::to_typst;

const PREAMBLE: &str = r#"#set page(paper: "a4", margin: 2.5cm)
#set text(font: "Linux Libertine", size: 11pt, lang: "en")
#set heading(numbering: "1.1")
#set par(justify: true, leading: 0.7em)

#let title-page(title) = [
  #v(3cm)
  #align(center)[
    #text(size: 28pt, weight: "bold")[#title]
  ]
  #v(1fr)
  #align(center)[
    #text(size: 10pt)[Generated #datetime.today().display()]
  ]
  #pagebreak()
]
"#;

/// Render `chapters` to PDF bytes with the given document title.
pub fn render(title: &str, chapters: &[Chapter]) -> Result<Vec<u8>> {
    let mut typst_src = String::new();
    typst_src.push_str(PREAMBLE);
    typst_src.push_str(&format!("\n#title-page([{}])\n", escape_inline(title)));
    for chapter in chapters {
        typst_src.push_str("\n#pagebreak(weak: true)\n");
        typst_src.push_str(&to_typst(&chapter.body));
    }

    let world = MemoryWorld::new(typst_src)?;
    let warned = typst::compile::<typst::layout::PagedDocument>(&world);
    let document = warned
        .output
        .map_err(|errs| anyhow!("typst compile failed: {}", format_diagnostics(&errs)))?;

    let pdf = typst_pdf::pdf(&document, &PdfOptions::default())
        .map_err(|errs| anyhow!("typst PDF emit failed: {}", format_diagnostics(&errs)))?;
    Ok(pdf)
}

fn format_diagnostics<T: std::fmt::Debug>(items: impl IntoIterator<Item = T>) -> String {
    items
        .into_iter()
        .map(|d| format!("{d:?}"))
        .collect::<Vec<_>>()
        .join("; ")
}

/// Lightly escape inline text for Typst: backslash anything Typst treats as markup.
fn escape_inline(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' | '*' | '_' | '`' | '$' | '#' | '<' | '>' | '@' | '~' | '[' | ']' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

/// Minimal in-memory `World`: one source file, embedded fonts, no packages.
struct MemoryWorld {
    main_id: FileId,
    main_source: Source,
    library: LazyHash<Library>,
    book: LazyHash<FontBook>,
    fonts: Vec<FontSlot>,
}

impl MemoryWorld {
    fn new(source_text: String) -> Result<Self> {
        let main_id = FileId::new(None, VirtualPath::new("main.typ"));
        let main_source = Source::new(main_id, source_text);

        let mut searcher = FontSearcher::new();
        searcher.include_system_fonts(false);
        let searched = searcher.search();
        if searched.fonts.is_empty() {
            return Err(anyhow!(
                "no fonts available (typst-kit embedded set is empty)"
            ));
        }

        Ok(Self {
            main_id,
            main_source,
            library: LazyHash::new(Library::default()),
            book: LazyHash::new(searched.book),
            fonts: searched.fonts,
        })
    }
}

impl typst::World for MemoryWorld {
    fn library(&self) -> &LazyHash<Library> {
        &self.library
    }

    fn book(&self) -> &LazyHash<FontBook> {
        &self.book
    }

    fn main(&self) -> FileId {
        self.main_id
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        if id == self.main_id {
            Ok(self.main_source.clone())
        } else {
            Err(FileError::NotFound(id.vpath().as_rootless_path().into()))
        }
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        Err(FileError::NotFound(id.vpath().as_rootless_path().into()))
    }

    fn font(&self, index: usize) -> Option<Font> {
        self.fonts.get(index).and_then(FontSlot::get)
    }

    fn today(&self, _offset: Option<i64>) -> Option<Datetime> {
        let now = Utc::now().date_naive();
        Datetime::from_ymd(now.year(), now.month() as u8, now.day() as u8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_inline_protects_specials() {
        let e = escape_inline("# $5 *star*");
        assert!(e.contains("\\#"));
        assert!(e.contains("\\$5"));
        assert!(e.contains("\\*star\\*"));
    }

    #[test]
    fn renders_minimal_pdf_without_panicking() {
        let chapter = Chapter {
            path: "ch.md".into(),
            body: "# Intro\n\nA paragraph.\n".into(),
            lang: Some("en".into()),
        };
        let bytes = render("Title", &[chapter]).unwrap();
        // PDFs always start with `%PDF-`.
        assert_eq!(&bytes[..5], b"%PDF-");
        assert!(bytes.len() > 1024);
    }
}
