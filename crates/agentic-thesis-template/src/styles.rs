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

/// Emit `word/styles.xml` as UTF-8 bytes.
///
/// Returns the canonical FHNW styles baseline. Word-COM finalize is expected
/// to add content-specific styles (e.g. `TOC1`, `TOF`) after body population.
pub fn emit_styles_xml() -> Vec<u8> {
    STYLES_XML.to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_matches_fixture() {
        assert_eq!(emit_styles_xml().len(), 346_290);
    }

    #[test]
    fn contains_palatino() {
        // MT-Template ADR-0002: font pinned on all four slots.
        let bytes = emit_styles_xml();
        let s = String::from_utf8_lossy(&bytes);
        assert!(s.contains("Palatino Linotype"), "Palatino Linotype missing from styles.xml");
    }

    #[test]
    fn contains_accent_hyperlink_color() {
        // MT-Template ADR-0002: hyperlink accent = 294F6D (dark navy).
        let bytes = emit_styles_xml();
        let s = String::from_utf8_lossy(&bytes);
        assert!(s.contains("294F6D") || s.contains("294f6d"),
                "hyperlink accent 294F6D missing from styles.xml");
    }

    #[test]
    fn contains_chapter_number_style() {
        let bytes = emit_styles_xml();
        let s = String::from_utf8_lossy(&bytes);
        assert!(s.contains("Chapter Number"),
                "custom 'Chapter Number' style missing from styles.xml");
    }
}
