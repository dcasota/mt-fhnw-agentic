//! FHNW Master-Thesis Word template — Rust port of MT-Template.
//!
//! Ports (see [`consolidation-fhnw-mt-template`] project memory):
//!
//! - `MT-Template/build/generate_template.py` — deterministic OOXML build:
//!   styles, sections, headers/footers, mirror-margins, bookmarks.
//! - `MT-Template/build/post_process.ps1` — Word-COM finalize: TOC/field
//!   refresh, multilevel list bind, back-matter Roman-page auto-tune,
//!   `.dotx SaveAs2`. (Word-COM steps live in `agentic::commands::book`.)
//!
//! Parity target: **byte-identical OOXML content** on each part after XML
//! canonicalisation (Gate A). Full ZIP-byte parity of the delivered `.docx`
//! is Gate B, achieved by running the Word-COM finalize step on the
//! Rust-emitted pre-COM `.docx` (see `parity_finding.md`).

#![allow(clippy::doc_markdown)]

pub mod content_types;
pub mod document;
pub mod font_table;
pub mod footers;
pub mod headers;
pub mod numbering;
pub mod package;
pub mod rels;
pub mod settings;
pub mod styles;
pub mod theme;
pub mod web_settings;

/// Style constants pulled verbatim from `MT-Template/build/generate_template.py`.
pub mod fhnw {
    /// Body font. `BODY_FONT = "Palatino Linotype"` (generate_template.py:42).
    pub const BODY_FONT: &str = "Palatino Linotype";

    /// Accent colour used for hyperlinks and cross-refs. `ACCENT = RGBColor(0x29, 0x4F, 0x6D)`
    /// (generate_template.py:43). Dark navy matching the ZHAW MT reference.
    pub const ACCENT_HEX: &str = "294F6D";

    /// Heading colour — pure black. `HEADING_BLACK = RGBColor(0x00, 0x00, 0x00)`
    /// (generate_template.py:44).
    pub const HEADING_BLACK_HEX: &str = "000000";

    /// Text width in cm. `TEXT_WIDTH_CM = 16.5` = A4-width − 2.5cm left − 2.0cm right
    /// (generate_template.py:47).
    pub const TEXT_WIDTH_CM: f32 = 16.5;

    /// Body font size (Pt).
    pub const BODY_SIZE_PT: u32 = 11;
    /// Heading 1 size (Pt).
    pub const HEADING_1_SIZE_PT: u32 = 24;
    /// Heading 2 size (Pt).
    pub const HEADING_2_SIZE_PT: u32 = 14;
    /// Heading 3 size (Pt).
    pub const HEADING_3_SIZE_PT: u32 = 12;
    /// Chapter Number (custom style, "Chapter N" line above H1) size (Pt).
    pub const CHAPTER_NUMBER_SIZE_PT: u32 = 17;
    /// Title style size (Pt).
    pub const TITLE_SIZE_PT: u32 = 28;
    /// Subtitle style size (Pt).
    pub const SUBTITLE_SIZE_PT: u32 = 16;

    /// A4 page dimensions (twips). 21 × 29.7 cm; 1 twip = 1/1440 inch.
    pub const PAGE_WIDTH_TWIPS: u32 = 11906; // 21 cm
    pub const PAGE_HEIGHT_TWIPS: u32 = 16838; // 29.7 cm

    /// Inside (spine-side) margin — 2.5 cm.
    pub const MARGIN_INSIDE_TWIPS: u32 = 1417;
    /// Outside margin — 2.0 cm.
    pub const MARGIN_OUTSIDE_TWIPS: u32 = 1134;
    /// Top / bottom margin — 2.5 cm.
    pub const MARGIN_TOP_BOTTOM_TWIPS: u32 = 1417;
}

/// Compute the sha256 of a byte slice as a lowercase hex string.
///
/// Used by parity tests to fingerprint fixtures.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    hex::encode(h.finalize())
}
