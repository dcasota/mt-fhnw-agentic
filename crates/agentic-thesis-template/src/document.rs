//! `word/document.xml` emitter — reproduces the FHNW MT-Template body layout.
//!
//! Ports `MT-Template/build/generate_template.py` sections:
//! - `build_titlepage()`, `build_frontmatter()`, `build_mainmatter()`, `build_backmatter()`
//! - mirror-margins per section (`<w:mirrorMargins/>` at settings + `<w:pgMar>`
//!   inside/outside twips)
//! - `evenAndOddHeaders` at settings level
//! - bookmarks: `fhnwFrontMatterEnd` (on Acronyms H1) + `fhnwBackMatterStart`
//!   (on List-of-Figures H1) — feed the Word-COM back-matter auto-tune.
//!
//! Target fixture: `tests/fixtures/empty_document.xml` (163 061 B).

/// Emit `word/document.xml` as UTF-8 bytes.
///
/// **Status:** stub — see P3c.
pub fn emit_document_xml() -> Vec<u8> {
    Vec::new()
}
