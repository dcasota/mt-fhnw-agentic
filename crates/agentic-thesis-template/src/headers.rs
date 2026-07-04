//! `word/headerN.xml` emitters — 3 headers × 18 sections = 54 files in the
//! reference. Content:
//!
//! - **first_page_header** (section opening / chapter opening) — bare page-no
//!   only, right-aligned on odd pages, left-aligned on even pages, using
//!   `{ IF { =MOD({PAGE \* Arabic};2) }=0 {PAGE} "" }`. No chapter ref, no rule.
//! - **odd_page_header** (recto, inner pages) — chapter-ref LEFT · page-no RIGHT,
//!   thin bottom rule.
//! - **even_page_header** (verso, inner pages) — page-no LEFT · chapter-ref RIGHT,
//!   thin bottom rule.
//!
//! Chapter ref = `{ STYLEREF "Chapter Number" }. { STYLEREF "Heading 1" }` for
//! main-matter, `{ STYLEREF "Heading 1" }` for front/back-matter.
//!
//! Ports `generate_template.py` helpers:
//! `append_page_field`, `append_chapter_ref_field`, `_nested_page_field`,
//! `_nested_mod_page_2`, `append_conditional_page_field`, `add_bottom_border`.

/// A single header kind.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum HeaderKind {
    /// First page of a section — bare page-no only, conditional on parity.
    FirstPage,
    /// Odd (recto) page — chapter ref LEFT, page-no RIGHT, bottom rule.
    OddPage,
    /// Even (verso) page — page-no LEFT, chapter ref RIGHT, bottom rule.
    EvenPage,
}

/// Section kind — controls chapter-ref content.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum SectionKind {
    /// Title page — no header (empty).
    TitlePage,
    /// Front matter (Imprint … Acronyms) — chapter ref = `{ STYLEREF "Heading 1" }`.
    FrontMatter,
    /// Main matter (Introduction … Discussion) — chapter ref =
    /// `{ STYLEREF "Chapter Number" }. { STYLEREF "Heading 1" }`.
    MainMatter,
    /// Back matter (List of Figures … Bibliography) — chapter ref =
    /// `{ STYLEREF "Heading 1" }`.
    BackMatter,
}

/// Emit a single `word/headerN.xml`.
///
/// **Status:** stub — see P3d.
pub fn emit_header_xml(_kind: HeaderKind, _section: SectionKind) -> Vec<u8> {
    Vec::new()
}
