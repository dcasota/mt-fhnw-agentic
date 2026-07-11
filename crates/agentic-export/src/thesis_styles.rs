//! Thesis-profile override for the raw-XML `word/styles.xml` emitter
//! (Wave 2 Agent D, master-thesis-bookkit parity, 2026-06-04).
//!
//! Companion to [`crate::styles_xml`]. The AI-Norms emitter ships the 186-
//! style table verbatim from `AI_Norms_and_Regulations_BOOK.docx`. The FHNW
//! master-thesis reference docx
//! (`FHNW2026_DanielCasota_MasterThesis__…__Photon_OS.docx`) declares a
//! **smaller** style table — 178 `<w:style>` elements — and references only
//! **11** of them from its body. Booking the AI-Norms 186-style block into a
//! thesis docx is harmless functionally (Word ignores unreferenced styles),
//! but a per-profile override gives the parity gate (ADR-0061) a true
//! byte-for-byte reference and avoids drift if either fixture is regenerated.
//!
//! Routing contract (callers / `book.rs`):
//! - Default path: keep `styles_xml::emit_styles_xml()` for every non-thesis
//!   book — the 186-style port stays untouched (constraint #4 in W2 brief).
//! - Thesis path: when `meta.thesis_typography ==
//!   TypographyProfile::FhnwProposalParity` AND the bookkit cascade is
//!   targeting `master_thesis_bookkit`, callers swap in
//!   [`emit_thesis_styles_xml`] via [`emit_styles_xml_for_profile`].
//!
//! Implementation strategy mirrors `styles_xml`: embed the reference XML at
//! compile time via `include_str!` so the emitter is hermetic (no runtime
//! file I/O, no fixture-search code paths to test). A structured re-emitter
//! was considered and rejected for the same reason — byte-for-byte parity is
//! the simplest contract for the ADR-0061 gate to assert.

/// The 11 styles actually referenced from the FHNW master-thesis reference
/// docx's `word/document.xml` (Wave-2 Agent D inventory, 2026-06-04). Each
/// must be present in the emitted `word/styles.xml` so paragraph-level
/// `pStyle` / `tblStyle` references resolve.
///
/// Counts (paragraph references in the reference body, descending):
/// Hyperlink (210), Heading2 (73), TOC2 (73), ListParagraph (26),
/// Heading1 (19), Heading3 (19), TOC1 (19), TOC3 (19), Caption (17),
/// TableofFigures (17), ChapterNumber (7).
///
/// Compare with [`crate::styles_xml::USED_STYLE_IDS`] (16 styles incl. the
/// `Bk*` family) — the thesis reference uses Word's built-in
/// `Heading1..3`/`Caption` family rather than the AI-Norms `BkH1..4`/`BkCaption`
/// family, hence the smaller USED-set.
pub const USED_STYLE_IDS: &[&str] = &[
    "Hyperlink",
    "Heading2",
    "TOC2",
    "ListParagraph",
    "Heading1",
    "Heading3",
    "TOC1",
    "TOC3",
    "Caption",
    "TableofFigures",
    "ChapterNumber",
];

/// Reference `word/styles.xml` from the FHNW master-thesis docx, embedded at
/// compile time so the emitter is hermetic (no runtime file I/O).
/// Fixture: 350,246 bytes, SHA-256
/// `5D8B1D50531FFE703B21D0A263A71AF3E1E42557F3521E2C677E1B08AD4361DB`.
const REFERENCE_STYLES_XML: &str = include_str!("../tests/fixtures/thesis_styles_reference.xml");

/// Emit the complete `<w:styles>` document for the FHNW master-thesis
/// profile (XML declaration + namespace preamble + `docDefaults` + 178
/// `<w:style>` elements + `latentStyles`).
///
/// Returns the embedded reference XML verbatim. Callers replace
/// `word/styles.xml` in the zip with this string during the finalize-pass.
pub fn emit_thesis_styles_xml() -> &'static str {
    REFERENCE_STYLES_XML
}

/// Count `<w:style ` elements in the emitted thesis-styles document. Used by
/// the parity test to assert the 178-style target. Mirrors the substring
/// counter in [`crate::styles_xml::count_styles`].
pub fn count_styles(xml: &str) -> usize {
    xml.matches("<w:style ").count()
}

/// Returns true if every style id in [`USED_STYLE_IDS`] is present in `xml`
/// as a `<w:style ... w:styleId="ID"`. Used as a quick presence check by
/// the parity test (the byte-for-byte assertion is the actual parity gate).
pub fn all_used_styles_present(xml: &str) -> bool {
    USED_STYLE_IDS
        .iter()
        .all(|id| xml.contains(&format!("w:styleId=\"{id}\"")))
}

/// Identifies which styles flavour a caller wants. Values map 1:1 onto a
/// `meta.thesis_typography` discriminant, but lives here (rather than
/// re-exporting `TypographyProfile`) to keep the styles-emitter decoupled
/// from the higher-level `BookMeta` API and to make the per-profile fixture
/// swap an explicit, audited decision.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub enum StylesProfile {
    /// AI-Norms-and-Regulations parity (Round V / Wave-2-A/B). The 186-style
    /// block embedded in [`crate::styles_xml`].
    AiNorms,
    /// FHNW master-thesis bookkit parity (Wave-2-D / ADR-0061). The 178-
    /// style block embedded in this module.
    FhnwMasterThesis,
    /// FHNW campaign bookkit parity (iter45.b/#558, 2026-07-11). Wires
    /// [`agentic_thesis_template::styles::emit_styles_xml_str`] — the
    /// byte-verbatim MT-Template `configure_styles()` output (170-style,
    /// 346 290 B) with Palatino Linotype pinned on all four `<w:rFonts>`
    /// slots of the `Normal` style. Campaigns previously fell through to
    /// docx-rs's default styles.xml (no Palatino pin), and the Word-COM
    /// finalize step normalised run-level `.fonts("Palatino Linotype")`
    /// back to the theme's `minorFont` (Cambria/Aptos) — so the body font
    /// silently regressed. Selecting this profile injects the MT-Template
    /// baseline so Palatino survives finalize.
    FhnwCampaignBookkit,
}

/// Per-profile router that picks the right reference styles fixture. The
/// `bookkit` cascade calls this once per book to decide which XML to inject
/// into `word/styles.xml` during finalize.
///
/// Keeping the routing here (a tiny pure function) means callers never have
/// to import both `styles_xml` and `thesis_styles` — they import one and
/// pass the profile.
#[must_use]
pub fn emit_styles_xml_for_profile(profile: StylesProfile) -> &'static str {
    match profile {
        StylesProfile::AiNorms => crate::styles_xml::emit_styles_xml(),
        StylesProfile::FhnwMasterThesis => emit_thesis_styles_xml(),
        StylesProfile::FhnwCampaignBookkit => {
            agentic_thesis_template::styles::emit_styles_xml_str()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emits_178_styles_for_thesis_reference() {
        let xml = emit_thesis_styles_xml();
        assert_eq!(
            count_styles(xml),
            178,
            "FHNW master-thesis reference declares 178 styles"
        );
    }

    #[test]
    fn all_used_styles_present_in_emitted() {
        assert!(all_used_styles_present(emit_thesis_styles_xml()));
    }

    /// Wave-2 Agent D parity gate: the thesis style block MUST be smaller
    /// than the AI-Norms one (the whole point of the per-profile override —
    /// the AI-Norms 186-style port stays for AI-Norms, the thesis ships its
    /// own narrower table). 178 < 186 is the literal contract; a larger
    /// regression would indicate the wrong fixture was wired in.
    #[test]
    fn thesis_styles_emit_smaller_style_block_than_ai_norms() {
        let thesis = count_styles(emit_thesis_styles_xml());
        let ai_norms = crate::styles_xml::count_styles(crate::styles_xml::emit_styles_xml());
        assert!(
            thesis < ai_norms,
            "thesis style block ({thesis}) must be strictly smaller \
             than the AI-Norms block ({ai_norms}); per-profile override \
             only makes sense if the thesis fixture is the narrower one"
        );
        assert_eq!(thesis, 178, "thesis reference fixture declares 178 styles");
        assert_eq!(
            ai_norms, 186,
            "AI-Norms reference fixture declares 186 styles"
        );
    }

    /// The Hyperlink character style in the thesis reference uses the same
    /// `0000FF` color as Round V's V-BC port — verified against the live
    /// reference fixture (`word/styles.xml`, `<w:styleId="Hyperlink">`,
    /// `<w:color w:val="0000FF" w:themeColor="hyperlink"/>`).
    #[test]
    fn hyperlink_style_uses_0000ff_color() {
        let xml = emit_thesis_styles_xml();
        // The Hyperlink style entry must carry the standard 0000FF color
        // bound to the theme's `hyperlink` slot (so the theme1.xml swap
        // in book.rs propagates).
        assert!(
            xml.contains(r#"<w:color w:val="0000FF" w:themeColor="hyperlink"/>"#),
            "Hyperlink style must reference 0000FF / themeColor=hyperlink"
        );
    }

    #[test]
    fn router_dispatches_per_profile() {
        // Identity-of-pointer comparison: each branch must return exactly
        // the bytes its sibling emitter returns (no wrapping / no copy).
        assert!(std::ptr::eq(
            emit_styles_xml_for_profile(StylesProfile::AiNorms),
            crate::styles_xml::emit_styles_xml(),
        ));
        assert!(std::ptr::eq(
            emit_styles_xml_for_profile(StylesProfile::FhnwMasterThesis),
            emit_thesis_styles_xml(),
        ));
        assert!(std::ptr::eq(
            emit_styles_xml_for_profile(StylesProfile::FhnwCampaignBookkit),
            agentic_thesis_template::styles::emit_styles_xml_str(),
        ));
    }

    /// #558 (2026-07-11): campaign profile must ship the MT-Template
    /// `Normal` style with Palatino pinned on all four `<w:rFonts>` slots.
    /// Without this, Word-COM finalize strips run-level `.fonts(...)`
    /// back to the theme's `minorFont` — silently regressing the body font.
    #[test]
    fn campaign_profile_pins_palatino_on_normal_style() {
        let xml = emit_styles_xml_for_profile(StylesProfile::FhnwCampaignBookkit);
        // The `Normal` style must reference Palatino Linotype on ALL four
        // rFonts slots (ascii/eastAsia/hAnsi/cs); Word inherits the run
        // font from Normal when no run-level rFonts wins.
        assert!(
            xml.contains(r#"<w:rFonts w:ascii="Palatino Linotype" w:eastAsia="Palatino Linotype" w:hAnsi="Palatino Linotype" w:cs="Palatino Linotype"/>"#),
            "FhnwCampaignBookkit styles.xml must pin Palatino on all four rFonts slots"
        );
    }

    /// #558 (2026-07-11): campaign fixture ships the MT-Template style
    /// count (170), not the AI-Norms (186) or thesis-reference (178)
    /// counts. Regression guard against wiring the wrong fixture in.
    #[test]
    fn campaign_profile_ships_mt_template_style_count() {
        let xml = emit_styles_xml_for_profile(StylesProfile::FhnwCampaignBookkit);
        assert_eq!(
            count_styles(xml),
            170,
            "campaign fixture (MT-Template baseline) declares 170 styles"
        );
    }

    #[test]
    fn used_style_ids_is_thirteen_styles_smaller_than_ai_norms() {
        // 11 thesis USED styles vs 16 AI-Norms USED styles — the thesis
        // body never references the BkBullet/BkCallout/BkH1..4/BkSubtitle/
        // Index1/IndexHeading/TableGrid family, so the USED-set is narrower
        // as well. Asserted as a regression guard: if the thesis fixture
        // ever picks up a Bk* family reference, that's worth investigating.
        assert_eq!(USED_STYLE_IDS.len(), 11);
        assert!(USED_STYLE_IDS.len() < crate::styles_xml::USED_STYLE_IDS.len());
    }
}
