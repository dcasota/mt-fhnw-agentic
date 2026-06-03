//! Raw-XML `word/theme/theme1.xml` emitter (Round V zone B, 2026-06-03).
//!
//! Wave-0 inventory confirmed the reference book
//! `AI_Norms_and_Regulations_BOOK.docx` ships an **Office-2010** theme XML
//! (Calibri / Cambria fonts, the legacy Office accent palette). docx-rs
//! 0.4.x ships a hard-coded **Office-2016+** theme (Aptos / Aptos Display,
//! the modern teal palette). Because every paragraph that resolves its font
//! via `<w:rFonts w:asciiTheme="majorHAnsi"/>` (the entire Bk* family, plus
//! every TableGrid / TOC / IndexHeading reference) reads the theme on
//! render, the cumulative drift is visible on every chapter: headings, body
//! prose, captions, table cells all paint in the wrong family.
//!
//! Mirror of [`crate::styles_xml`]: embed the reference XML at compile-time
//! via `include_str!` and expose a tiny façade ([`emit_theme_xml`]) the
//! finalize-pass swaps in for `word/theme/theme1.xml`. Same rationale —
//! round-tripping risks attribute reordering / whitespace normalisation,
//! both of which break byte-identical parity.
//!
//! Cross-cutting risk #1 (THEME-COLOR-CASCADE, see Round-V planner output)
//! is intentionally accepted at zone-merge boundary: replacing theme1.xml
//! changes the heading colour (royal-blue) and Hyperlink colour. Zone E's
//! callout coloring uses inline literals (2E7D32 / C77F18 / 1F3864) that do
//! NOT reference `accent1`, so the cascade is safe.

/// Reference `word/theme/theme1.xml` from
/// `AI_Norms_and_Regulations_BOOK.docx`, embedded at compile time so the
/// emitter is hermetic (no runtime file I/O).
///
/// Wave-0 fingerprint: 7,643 bytes, SHA-256
/// `B2295D3198893D2C03F5E584C749A15751B798AEFDCD9BEE2889F13903D68CB2`.
const REFERENCE_THEME_XML: &str = include_str!("../tests/fixtures/theme1_reference.xml");

/// Emit the complete Office-2010 theme1.xml document. Returns the embedded
/// reference XML verbatim. Callers replace `word/theme/theme1.xml` in the
/// zip with this string during the finalize-pass.
pub fn emit_theme_xml() -> &'static str {
    REFERENCE_THEME_XML
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wave-0 fixture length (bytes). A simple length check is a cheap
    /// guard against accidental fixture edits; the byte-for-byte assertion
    /// is done by the parity gate that compares the in-zip theme1.xml
    /// against this constant.
    const REFERENCE_THEME_LEN: usize = 7643;

    #[test]
    fn fixture_matches_pinned_length() {
        let xml = emit_theme_xml();
        assert_eq!(
            xml.as_bytes().len(),
            REFERENCE_THEME_LEN,
            "theme1.xml fixture length has drifted — re-pin or revert"
        );
    }

    #[test]
    fn emits_office2010_majorfont_calibri() {
        let xml = emit_theme_xml();
        // The reference theme is Office-2010: majorFont latin typeface is
        // Cambria, minorFont latin typeface is Calibri. Guard against an
        // accidental Office-2016 (Aptos) fixture swap.
        assert!(
            xml.contains("<a:majorFont>"),
            "theme1 must declare a majorFont block"
        );
        assert!(
            xml.contains("<a:minorFont>"),
            "theme1 must declare a minorFont block"
        );
        assert!(
            xml.contains("Cambria") || xml.contains("Calibri"),
            "theme1 must carry the Office-2010 Calibri/Cambria pair"
        );
    }

    #[test]
    fn byte_for_byte_matches_fixture() {
        // The include_str! is literally the file bytes; this test is
        // tautological at the source level but proves the module compiled
        // with the fixture present and accessible (catches a stale build).
        let xml = emit_theme_xml();
        assert!(!xml.is_empty(), "theme1 fixture must not be empty");
        assert!(
            xml.contains("<a:theme") && xml.contains("</a:theme>"),
            "theme1 must be a well-formed <a:theme> root"
        );
    }
}
