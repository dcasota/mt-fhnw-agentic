//! Relationship files.
//!
//! - `_rels/.rels` — package-level (points at `word/document.xml` and docProps)
//! - `word/_rels/document.xml.rels` — enumerates every headerN/footerN/media
//!   relationship consumed by document.xml

const ROOT_RELS: &[u8] = include_bytes!("../tests/fixtures/empty_root_rels.xml");
const DOCUMENT_RELS: &[u8] = include_bytes!("../tests/fixtures/empty_document_rels.xml");

/// Emit `_rels/.rels`.
pub fn emit_root_rels() -> Vec<u8> {
    ROOT_RELS.to_vec()
}

/// Emit `word/_rels/document.xml.rels`.
pub fn emit_document_rels() -> Vec<u8> {
    DOCUMENT_RELS.to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_rels_size_matches_fixture() {
        assert_eq!(emit_root_rels().len(), 590);
    }

    #[test]
    fn document_rels_size_matches_fixture() {
        assert_eq!(emit_document_rels().len(), 17_102);
    }
}
