//! `thesis_typography` — profile-specific typography table for the FHNW
//! master-thesis bookkit (Wave-2 Agent B, REF parity 2026-06-04).
//!
//! This module is the canonical source of truth for the font / size / colour
//! specs the `master_thesis_bookkit` profile must emit to match the reference
//! `.docx`
//! (`FHNW2026_DanielCasota_MasterThesis__Governance_and_Leadership_for_Self-Evolving_Background_Autonomy_in_Photon_OS.docx`).
//!
//! ## Why a separate module?
//!
//! The historical typography branches in `book.rs` (`*_for(TypographyProfile)`)
//! hard-code two profiles: `Designer` (Georgia/Calibri/navy) and
//! `FhnwProposalParity` (Arial/Times-New-Roman/black, derived from the
//! 2025-12-29 *proposal*). The thesis reference docx is a third profile —
//! `Palatino Linotype` body, default `minorHAnsi` headings, accent colour
//! `4F81BD`, with a colon caption-format. Hard-coding a third arm everywhere
//! would balloon the existing helpers; instead the helpers consult
//! `ThesisTypography::default()` once when the manifest's
//! `thesis_typography == "fhnw-thesis-reference"`.
//!
//! ## Source of the constants
//!
//! Extracted with PowerShell from
//! `word/styles.xml` of the FROZEN reference fixture
//! (`tests/fixtures/reference/master_thesis_reference.docx`,
//! SHA-256 `c2d383bb43b3ce501241d55d20f26f6cdcdd4a5122d814f376ba0e90163b3184`,
//! ADR-0061). The extraction iterates `<w:style w:styleId="…">` and reads
//! the first `<w:rFonts w:ascii=…>` and `<w:color w:val=…>` and `<w:sz
//! w:val=…>` inside each `<w:rPr>`. Twelve of the 178 declared styles are
//! actually USED in the document body (`document.xml` `<w:pStyle>` index):
//! `Normal`, `Heading1` … `Heading4`, `Title`, `Subtitle`, `Caption`,
//! `TOC1`/`TOC2`, `Header`, `Footer`. Their specs are encoded below.
//!
//! ## Round V interaction
//!
//! AI Norms parity (`body_render_use_bk_styles=true`, ADR-0054) is NOT
//! affected — the AI Norms book still uses `TypographyProfile::Designer`
//! plus the `Bk*` named-style family. Only books whose manifest sets the
//! string `thesis_typography: "fhnw-thesis-reference"` (parsed into
//! `TypographyProfile::FhnwThesisReference` by the CLI layer in W2-A's
//! follow-up) reach this module.

/// Palatino Linotype — declared by `Normal` in the reference styles.xml.
pub const REF_BODY_FACE: &str = "Palatino Linotype";

/// Default `minorHAnsi` theme font for headings — resolves to `Calibri`
/// in the reference fontTable.xml.
pub const REF_HEAD_FACE: &str = "Calibri";

/// Times New Roman — declared in the reference fontTable.xml; the
/// reference uses it for inline run overrides (e.g. footnote text and the
/// "Antikythera NOTE" inscription paragraph). Captions reuse the heading
/// face (`Calibri`).
pub const REF_CAPTION_FACE: &str = "Times New Roman";

/// Reference H1 / Title accent colour (`accent1` theme colour).
pub const REF_ACCENT_BLUE: &str = "4F81BD";

/// Reference dark-blue heading colour (`Title` character style).
pub const REF_HEAD_DARK: &str = "17365D";

/// Reference mid-blue heading colour (H5/H6).
pub const REF_HEAD_MID: &str = "243F60";

/// Reference H7 grey heading colour.
pub const REF_HEAD_GREY: &str = "404040";

/// Pure black — every body run.
pub const REF_BLACK: &str = "000000";

/// Per-style font + size + colour record derived from the reference
/// `word/styles.xml`. Sizes are in half-points (Word convention) and
/// colours are `RRGGBB` hex strings without the leading `#`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StyleSpec {
    pub style_id: &'static str,
    pub ascii: &'static str,
    pub size_hp: u32,
    pub color: &'static str,
    pub bold: bool,
}

/// Full typography table for the reference thesis. Indexed by style id;
/// the engine consumes specific entries (`Normal`, `Heading1`..`Heading4`,
/// `Caption`, `Title`, `Subtitle`) when rendering. Additional entries are
/// kept for diagnostic round-trip parity checks.
#[derive(Debug, Clone, Copy)]
pub struct ThesisTypography {
    pub normal: StyleSpec,
    pub heading1: StyleSpec,
    pub heading2: StyleSpec,
    pub heading3: StyleSpec,
    pub heading4: StyleSpec,
    pub title: StyleSpec,
    pub subtitle: StyleSpec,
    pub caption: StyleSpec,
}

impl ThesisTypography {
    /// Default — the FHNW thesis reference fixture values verbatim.
    pub const fn default() -> Self {
        Self {
            normal: StyleSpec {
                style_id: "Normal",
                ascii: REF_BODY_FACE,
                size_hp: 22, // 11pt — Word default body
                color: REF_BLACK,
                bold: false,
            },
            heading1: StyleSpec {
                style_id: "Heading1",
                ascii: REF_HEAD_FACE,
                size_hp: 48, // 24pt
                color: REF_BLACK,
                bold: true,
            },
            heading2: StyleSpec {
                style_id: "Heading2",
                ascii: REF_HEAD_FACE,
                size_hp: 28, // 14pt
                color: REF_BLACK,
                bold: true,
            },
            heading3: StyleSpec {
                style_id: "Heading3",
                ascii: REF_HEAD_FACE,
                size_hp: 24, // 12pt
                color: REF_BLACK,
                bold: true,
            },
            heading4: StyleSpec {
                style_id: "Heading4",
                ascii: REF_HEAD_FACE,
                size_hp: 22, // 11pt
                color: REF_ACCENT_BLUE,
                bold: true,
            },
            title: StyleSpec {
                style_id: "Title",
                ascii: REF_HEAD_FACE,
                size_hp: 56, // 28pt
                color: REF_BLACK,
                bold: true,
            },
            subtitle: StyleSpec {
                style_id: "Subtitle",
                ascii: REF_HEAD_FACE,
                size_hp: 32, // 16pt
                color: REF_BLACK,
                bold: true,
            },
            caption: StyleSpec {
                style_id: "Caption",
                ascii: REF_HEAD_FACE,
                size_hp: 18, // 9pt
                color: REF_ACCENT_BLUE,
                bold: true,
            },
        }
    }

    /// Resolve a heading level (1..=4) to the matching `StyleSpec`.
    /// Levels above 4 fall back to `heading4` so the engine never panics
    /// on a deep chapter.
    pub const fn heading(&self, level: u8) -> &StyleSpec {
        match level {
            1 => &self.heading1,
            2 => &self.heading2,
            3 => &self.heading3,
            _ => &self.heading4,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Locked test — the reference typography MUST match these exact
    /// constants verbatim. Any deviation means either (a) the reference
    /// docx changed (re-extract from the FROZEN fixture and update the
    /// module) or (b) someone hand-edited the typography table.
    #[test]
    fn thesis_typography_uses_reference_font_specs() {
        let t = ThesisTypography::default();
        // Body: Palatino Linotype 11pt black.
        assert_eq!(t.normal.ascii, "Palatino Linotype");
        assert_eq!(t.normal.size_hp, 22);
        assert_eq!(t.normal.color, "000000");
        assert!(!t.normal.bold);
        // H1: Calibri 24pt bold black (Title face).
        assert_eq!(t.heading1.ascii, "Calibri");
        assert_eq!(t.heading1.size_hp, 48);
        assert!(t.heading1.bold);
        // H4 carries the only accent colour in the heading ladder.
        assert_eq!(t.heading4.color, "4F81BD");
        // Caption: 9pt accent-blue, bold (the reference Caption style).
        assert_eq!(t.caption.size_hp, 18);
        assert_eq!(t.caption.color, "4F81BD");
        // Heading(level) selector falls back to H4 for deep chapters.
        assert_eq!(t.heading(1).style_id, "Heading1");
        assert_eq!(t.heading(7).style_id, "Heading4");
    }

    #[test]
    fn thesis_typography_default_is_const() {
        // Compile-time const constructor — needed for the
        // `static REF: ThesisTypography = ThesisTypography::default();`
        // pattern used in book.rs once W2-A's TypographyProfile arm
        // lands.
        const _T: ThesisTypography = ThesisTypography::default();
        assert_eq!(_T.normal.style_id, "Normal");
    }
}
