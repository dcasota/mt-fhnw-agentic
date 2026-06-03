//! `agentic-export` — render thesis content to DOCX/PDF.
//!
//! Two entry points: [`render_docx`] and [`render_pdf`]. Both walk a project's
//! HEAD tree, collect markdown chapters under an optional prefix, and emit
//! byte streams ready to be written to disk or piped to stdout.

#![warn(missing_debug_implementations)]
#![warn(rust_2018_idioms)]

pub mod book;
pub mod collect;
pub mod decorations;
pub mod docx;
pub mod icons;
pub mod index;
pub mod markdown;
pub mod numbering_xml;
pub mod pdf;
pub mod styles_xml;
pub mod theme_xml;

use anyhow::Result;
use rusqlite::Connection;

pub use collect::{Chapter, collect_chapters};

/// Render the project's chapters as a `.docx` file.
pub fn render_docx(
    conn: &Connection,
    project_id: &str,
    prefix: &str,
    title: &str,
) -> Result<Vec<u8>> {
    let chapters = collect_chapters(conn, project_id, prefix)?;
    docx::render(title, &chapters)
}

/// Render the project's chapters as a PDF file.
pub fn render_pdf(
    conn: &Connection,
    project_id: &str,
    prefix: &str,
    title: &str,
) -> Result<Vec<u8>> {
    let chapters = collect_chapters(conn, project_id, prefix)?;
    pdf::render(title, &chapters)
}
