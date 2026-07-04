//! `word/numbering.xml` — the multilevel outline list bound to Heading 1–3.
//!
//! Ports the intent of `post_process.ps1`'s ListTemplate setup (H1 = counter-only,
//! H2 = `%1.%2`, H3 = `%1.%2.%3`) via the canonical bytes captured from the
//! FHNW MT-Template's finalized empty output.

const NUMBERING_XML: &[u8] = include_bytes!("../tests/fixtures/empty_numbering.xml");

/// Emit `word/numbering.xml`.
pub fn emit_numbering_xml() -> Vec<u8> {
    NUMBERING_XML.to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_matches_fixture() {
        assert_eq!(emit_numbering_xml().len(), 9_568);
    }
}
