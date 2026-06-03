//! Raw-XML `word/styles.xml` emitter (Wave 2, AI-Norms parity, 2026-06-03).
//!
//! docx-rs's typed `Style` API can express only a small subset of the WordML
//! style schema: it has no representation for `<w:tblBorders>`, theme-font
//! references (`<w:rFonts w:asciiTheme="majorHAnsi"/>`), several `<w:pPr>`
//! fields (auto-spacing, contextual-spacing, snap-to-grid), `<w:tblStylePr>`
//! (conditional formatting per table region), and any of the
//! `<w:lsdException>` semi-hidden/UI-priority flags that sit under
//! `<w:latentStyles>`.
//!
//! The reference book `AI_Norms_and_Regulations_BOOK.docx` declares **186**
//! styles + a `<w:docDefaults>` preamble + a `<w:latentStyles>` block whose
//! cumulative formatting the bookkit harness depends on (paragraph-level theme
//! fonts, `TableGrid` borders, hyperlink underline, etc.). The previous
//! emitter shipped only 35 styles built via the typed API; this module emits
//! all 186 verbatim from the reference fixture so a finalize-pass can swap
//! the docx-rs-authored `word/styles.xml` with byte-identical reference XML.
//!
//! Approach: embed the reference XML at compile time via `include_str!` and
//! expose a tiny façade ([`emit_styles_xml`], [`USED_STYLE_IDS`],
//! [`count_styles`]). This is intentionally simple — Wave-0 inventory already
//! confirmed the reference fixture is byte-identical to the docx the
//! tooling stack (Word's Style pane, agentic check writing-quality,
//! third-party renderers) reads, so verbatim re-emit is the *least surprising*
//! parity strategy. A structured table-of-styles approach (parse → re-emit)
//! was considered and rejected: the round-trip risks reordering attributes
//! or normalising whitespace, both of which would break byte-for-byte
//! reference parity for the 16 USED styles.

/// The 16 styles that are actually referenced from the reference book's body
/// (Wave 0 inventory, 2026-06-02). Each must be present in the emitted
/// `word/styles.xml` *byte-for-byte* identical to the reference fixture so
/// that paragraph-level `pStyle` / `tblStyle` references resolve to the
/// expected formatting in Word.
///
/// Counts (paragraph references in the reference body, descending):
/// Hyperlink (816), BkBullet (659), BkCallout (364), BkH2 (254), TOC2 (254),
/// BkCaption (155), TableofFigures (155), Index1 (113), BkH3 (53),
/// TOC3 (53), TableGrid (50), BkH1 (44), TOC1 (44), IndexHeading (20),
/// BkH4 (9), BkSubtitle (2).
pub const USED_STYLE_IDS: &[&str] = &[
    "Hyperlink",
    "BkBullet",
    "BkCallout",
    "BkH2",
    "TOC2",
    "BkCaption",
    "TableofFigures",
    "Index1",
    "BkH3",
    "TOC3",
    "TableGrid",
    "BkH1",
    "TOC1",
    "IndexHeading",
    "BkH4",
    "BkSubtitle",
];

/// Reference `word/styles.xml` from `AI_Norms_and_Regulations_BOOK.docx`,
/// embedded at compile time so the emitter is hermetic (no runtime file I/O).
/// Wave-0 fingerprint: 353,534 bytes, SHA-256
/// `FD1FFD44556C86CA06974593F3C9082EEAE99E5EB2EB533E119AAD47C3C62175`.
const REFERENCE_STYLES_XML: &str =
    include_str!("../tests/fixtures/styles_reference.xml");

/// Emit the complete `<w:styles>` document (XML declaration + namespace
/// preamble + docDefaults + 186 `<w:style>` elements + latentStyles).
///
/// Returns the embedded reference XML verbatim. Callers replace
/// `word/styles.xml` in the zip with this string during the finalize-pass.
pub fn emit_styles_xml() -> &'static str {
    REFERENCE_STYLES_XML
}

/// Count `<w:style ` elements in the emitted styles document. Used by the
/// parity test to assert the 186-style target.
pub fn count_styles(xml: &str) -> usize {
    // The reference fixture uses `<w:style ` (with trailing space before
    // attributes) for every style entry; `<w:styles>` (no space) is the
    // root element, so a simple substring count is sufficient and avoids
    // pulling in an XML parser.
    xml.matches("<w:style ").count()
}

/// Returns true if every style id in `USED_STYLE_IDS` is present in `xml`
/// as a `<w:style ... w:styleId="ID"`. Used by the parity test as a quick
/// presence check (the byte-for-byte assertion is the actual parity gate).
pub fn all_used_styles_present(xml: &str) -> bool {
    USED_STYLE_IDS
        .iter()
        .all(|id| xml.contains(&format!("w:styleId=\"{id}\"")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emits_186_styles() {
        let xml = emit_styles_xml();
        assert_eq!(count_styles(xml), 186, "reference declares 186 styles");
    }

    #[test]
    fn all_used_styles_present_in_emitted() {
        assert!(all_used_styles_present(emit_styles_xml()));
    }
}
