//! `[Content_Types].xml` — MIME type registry for the OOXML package.
//!
//! The empty-template baseline (17 353 B) enumerates every headerN/footerN
//! Override present in the package. When section count changes, this file
//! will need to be regenerated dynamically — deferred to when the dynamic
//! `document.xml` emitter (P3c) lands and section count is variable.

const CONTENT_TYPES_XML: &[u8] = include_bytes!("../tests/fixtures/empty_content_types.xml");

/// Emit `[Content_Types].xml`.
pub fn emit_content_types_xml() -> Vec<u8> {
    CONTENT_TYPES_XML.to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_matches_fixture() {
        assert_eq!(emit_content_types_xml().len(), 17_353);
    }
}
