//! Package assembly — collects all OOXML parts into a `Vec<PackagePart>` that
//! a downstream writer (`agentic-export`) can encode as a `.docx` ZIP.
//!
//! This module wires together every part emitter in the crate so the caller
//! can request `assemble_empty_template()` once and get every byte needed to
//! reproduce the FHNW MT-Template's empty output. When the `document.xml`
//! emitter (P3c) matures, it will be swapped in here.

/// One entry in the OOXML package — path relative to the ZIP root + bytes.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct PackagePart {
    /// Path inside the ZIP, forward-slash separated (e.g. `word/document.xml`).
    pub path: String,
    /// Raw part bytes.
    pub bytes: Vec<u8>,
}

impl PackagePart {
    fn new(path: impl Into<String>, bytes: Vec<u8>) -> Self {
        Self { path: path.into(), bytes }
    }
}

/// Assemble the full FHNW MT-Template package as it appears in the empty
/// `dist/FHNW_MasterThesis_Template.docx`.
///
/// **Note:** the headerN/footerN parts (60 + 60 files in the empty template)
/// are NOT yet emitted from patterns — they're an outstanding task (P3d).
/// Callers currently receive only the static baseline parts; the header set
/// will grow as P3d lands. `[Content_Types].xml` and `document.xml.rels` will
/// then need to be regenerated to match the header count.
#[must_use]
pub fn assemble_empty_template() -> Vec<PackagePart> {
    let logo_bytes = include_bytes!("../tests/fixtures/empty_image1_fhnwlogo.png").to_vec();
    let customxml_item1 = include_bytes!("../tests/fixtures/empty_customxml_item1.xml").to_vec();
    let customxml_item1_rels = include_bytes!("../tests/fixtures/empty_customxml_item1_rels.xml").to_vec();
    let customxml_itemprops = include_bytes!("../tests/fixtures/empty_customxml_itemprops1.xml").to_vec();
    let docprops_app = include_bytes!("../tests/fixtures/empty_docprops_app.xml").to_vec();
    let docprops_core = include_bytes!("../tests/fixtures/empty_docprops_core.xml").to_vec();
    let endnotes = include_bytes!("../tests/fixtures/empty_endnotes.xml").to_vec();
    let footnotes = include_bytes!("../tests/fixtures/empty_footnotes.xml").to_vec();

    vec![
        PackagePart::new("[Content_Types].xml", crate::content_types::emit_content_types_xml()),
        PackagePart::new("_rels/.rels", crate::rels::emit_root_rels()),
        PackagePart::new("customXml/_rels/item1.xml.rels", customxml_item1_rels),
        PackagePart::new("customXml/item1.xml", customxml_item1),
        PackagePart::new("customXml/itemProps1.xml", customxml_itemprops),
        PackagePart::new("docProps/app.xml", docprops_app),
        PackagePart::new("docProps/core.xml", docprops_core),
        PackagePart::new("word/_rels/document.xml.rels", crate::rels::emit_document_rels()),
        PackagePart::new("word/document.xml", crate::document::emit_document_xml()),
        PackagePart::new("word/endnotes.xml", endnotes),
        PackagePart::new("word/fontTable.xml", crate::font_table::emit_font_table_xml()),
        PackagePart::new("word/footnotes.xml", footnotes),
        PackagePart::new("word/media/image1.png", logo_bytes),
        PackagePart::new("word/numbering.xml", crate::numbering::emit_numbering_xml()),
        PackagePart::new("word/settings.xml", crate::settings::emit_settings_xml()),
        PackagePart::new("word/styles.xml", crate::styles::emit_styles_xml()),
        PackagePart::new("word/theme/theme1.xml", crate::theme::emit_theme1_xml()),
        PackagePart::new("word/webSettings.xml", crate::web_settings::emit_web_settings_xml()),
        // TODO(P3d): word/header{1..60}.xml, word/footer{1..60}.xml
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_paths_are_unique() {
        let parts = assemble_empty_template();
        let mut paths: Vec<&str> = parts.iter().map(|p| p.path.as_str()).collect();
        let n = paths.len();
        paths.sort_unstable();
        paths.dedup();
        assert_eq!(paths.len(), n, "duplicate paths in assembly");
    }

    #[test]
    fn logo_part_has_expected_size() {
        let parts = assemble_empty_template();
        let logo = parts.iter().find(|p| p.path == "word/media/image1.png").expect("logo present");
        assert_eq!(logo.bytes.len(), 129_051);
    }

    #[test]
    fn styles_part_has_expected_size() {
        let parts = assemble_empty_template();
        let styles = parts.iter().find(|p| p.path == "word/styles.xml").expect("styles present");
        assert_eq!(styles.bytes.len(), 346_290);
    }
}
