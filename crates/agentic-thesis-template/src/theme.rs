//! `word/theme/theme1.xml` — FHNW theme (7 643 B). Content-agnostic; embed as-is.

const THEME1_XML: &[u8] = include_bytes!("../tests/fixtures/empty_theme1.xml");

/// Emit `word/theme/theme1.xml`.
pub fn emit_theme1_xml() -> Vec<u8> {
    THEME1_XML.to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_matches_fixture() {
        assert_eq!(emit_theme1_xml().len(), 7_643);
    }
}
