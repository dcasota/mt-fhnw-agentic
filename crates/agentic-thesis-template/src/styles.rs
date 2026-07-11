//! `word/styles.xml` emitter.
//!
//! The FHNW branding styles (Palatino Linotype 11 pt body, H1 24 pt, custom
//! `Chapter Number` 17 pt bold, hyperlink `#294F6D`, etc.) do not depend on
//! thesis content — they are Word-COM's canonical output after
//! `configure_styles()` runs on an empty template. Rather than piecemeal-port
//! 40 KB of `generate_template.py` OOXML-emit logic (which the Word-COM
//! `$doc.Save()` would re-shape anyway), we ship the canonical bytes verbatim
//! as an embedded fixture.
//!
//! The Word-COM finalize step (P4) may inject additional content-specific
//! styles (table styles, etc.) after body content is populated — that growth
//! is expected and handled deterministically by Word, not by us.
//!
//! Parity: `emit_styles_xml() == include_bytes!("../tests/fixtures/empty_styles.xml")`.

/// Canonical FHNW `word/styles.xml` — the empty-template baseline.
/// Byte-for-byte matches `MT-Template/dist/FHNW_MasterThesis_Template.docx :: word/styles.xml`
/// (346 290 B).
const STYLES_XML: &[u8] = include_bytes!("../tests/fixtures/empty_styles.xml");

/// Same bytes as [`STYLES_XML`], embedded as `&'static str` so callers that
/// route through `&'static str` APIs (e.g. `agentic-export`'s
/// `emit_styles_xml_for_profile`) can plug this fixture in without copying
/// through `Vec<u8>` and without a runtime UTF-8 validation. The XML is
/// UTF-8 by construction (the fixture ships with a UTF-8 BOM-less XML
/// declaration `encoding="UTF-8"`).
const STYLES_XML_STR: &str = include_str!("../tests/fixtures/empty_styles.xml");

/// Emit `word/styles.xml` as UTF-8 bytes.
///
/// Returns the canonical FHNW styles baseline. Word-COM finalize is expected
/// to add content-specific styles (e.g. `TOC1`, `TOF`) after body population.
pub fn emit_styles_xml() -> Vec<u8> {
    STYLES_XML.to_vec()
}

/// Emit `word/styles.xml` as a borrowed UTF-8 slice. Same bytes as
/// [`emit_styles_xml`]; use this variant when routing through a
/// `&'static str` API (avoids allocating a per-call `Vec`).
#[must_use]
pub fn emit_styles_xml_str() -> &'static str {
    STYLES_XML_STR
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_matches_fixture() {
        assert_eq!(emit_styles_xml().len(), 346_290);
    }

    #[test]
    fn str_and_bytes_variants_match() {
        // The `&'static str` route must expose the exact same bytes as the
        // `Vec<u8>` route — callers can pick either without behavioural drift.
        assert_eq!(
            emit_styles_xml_str().as_bytes(),
            emit_styles_xml().as_slice()
        );
        assert_eq!(emit_styles_xml_str().len(), 346_290);
    }

    #[test]
    fn contains_palatino() {
        // MT-Template ADR-0002: font pinned on all four slots.
        let bytes = emit_styles_xml();
        let s = String::from_utf8_lossy(&bytes);
        assert!(
            s.contains("Palatino Linotype"),
            "Palatino Linotype missing from styles.xml"
        );
    }

    #[test]
    fn contains_accent_hyperlink_color() {
        // MT-Template ADR-0002: hyperlink accent = 294F6D (dark navy).
        let bytes = emit_styles_xml();
        let s = String::from_utf8_lossy(&bytes);
        assert!(
            s.contains("294F6D") || s.contains("294f6d"),
            "hyperlink accent 294F6D missing from styles.xml"
        );
    }

    #[test]
    fn contains_chapter_number_style() {
        let bytes = emit_styles_xml();
        let s = String::from_utf8_lossy(&bytes);
        assert!(
            s.contains("Chapter Number"),
            "custom 'Chapter Number' style missing from styles.xml"
        );
    }
}
