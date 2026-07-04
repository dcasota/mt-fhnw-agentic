//! `word/webSettings.xml` — Word web-view rendering hints. Empty-template
//! baseline 1 046 B. Static.

const WEB_SETTINGS_XML: &[u8] = include_bytes!("../tests/fixtures/empty_webSettings.xml");

/// Emit `word/webSettings.xml`.
pub fn emit_web_settings_xml() -> Vec<u8> {
    WEB_SETTINGS_XML.to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_matches_fixture() {
        assert_eq!(emit_web_settings_xml().len(), 1_046);
    }
}
