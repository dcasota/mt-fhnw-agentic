//! `word/fontTable.xml` — font declarations (Palatino Linotype + fallbacks).
//! Empty-template baseline 3 205 B. Word COM may add fonts when content
//! introduces new faces; the baseline stays canonical.

const FONT_TABLE_XML: &[u8] = include_bytes!("../tests/fixtures/empty_fontTable.xml");

/// Emit `word/fontTable.xml`.
pub fn emit_font_table_xml() -> Vec<u8> {
    FONT_TABLE_XML.to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_matches_fixture() {
        assert_eq!(emit_font_table_xml().len(), 3_205);
    }
}
