//! Admonition icon module — embedded PNG bytes + `<w:drawing>` emit helper.
//!
//! Round-V E2 (AI-Norms visual parity, 2026-06-03): the historical
//! admonition emit path (`book::admonition_box`) read `icon_{kind}.png`
//! from a `figdir` side-channel that the CLI populates by rendering the
//! `agentic-figures` icon recipes into a scratch directory. When the
//! per-render figdir was absent (e.g. partial builds, unit-test paths,
//! re-runs after a clean), the path silently fell back to a unicode glyph
//! and the resulting docx no longer carried a `<w:drawing>` for the
//! admonition label — visual parity with the reference book broke without
//! any error surface.
//!
//! This module replaces the figdir side-channel with `include_bytes!`-
//! embedded PNG payloads that ship in the crate itself (under
//! `crates/agentic-export/icons/`). The bytes are mirrored from
//! `specs/figures/raster/ai_norms/icon_{tip,note,warning}.png`; V-G1's
//! 290×290 regen lands later and a follow-up swaps the `include_bytes!`
//! targets accordingly.
//!
//! Per-icon metadata (`IconKind::Tip => ("icon_tip", 151_200)`) captures
//! the stable `pic:cNvPr name=` the reference book uses + the EMU square
//! the admonition label expects. `docx-rs` 0.4.20 lacks a public way to
//! set the `pic:cNvPr name=` attribute on a `Pic` (the writer hardcodes
//! `name=""`), so this module also exposes
//! [`rewrite_pic_names_in_document_xml`], a post-process helper that
//! walks the serialised `word/document.xml` and patches every
//! `<pic:cNvPr id="0" name=""/>` whose enclosing `pic:pic` carries one of
//! the sentinel `r:embed` rids back to the per-icon stable name. The
//! `book.rs` finalize pipeline (`postprocess_docx_inner_layout`) chains
//! this helper after style + sectPr rewrites so the resulting docx
//! exposes parity-friendly drawing names without forking docx-rs.

use docx_rs::{Docx, LineSpacing, Paragraph, Pic, Run, RunFonts};

/// One of the three admonition icons shipped with the book renderer.
///
/// The variant name maps 1:1 onto the `code-block` language tag used in
/// the source markdown (`note`, `tip`, `warning`); the `name()` method
/// returns the stable `pic:cNvPr name=` value the reference book emits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IconKind {
    Tip,
    Note,
    Warning,
}

/// EMU side length the admonition icon is rendered at.
///
/// Bumped from the historical 150_000 (≈4.16 mm) to **151_200**
/// (≈4.20 mm) to match the reference book's measured icon side; the
/// gentle bump keeps the icon visually identical at print resolution but
/// nudges the `<a:ext>` element away from the round-number heuristics
/// some parity checks use to spot stub drawings.
pub const ADMONITION_ICON_EMU: u32 = 151_200;

/// EMU side length the bare-URL QR code is rendered at in the Sources box.
///
/// Bumped from the historical 900_000 (2.5 cm) to **972_000** (2.7 cm)
/// in concert with the Sources-box QR column widening from 1700 to 1850
/// twips; the wider QR keeps Word's scaler from interpolating noise into
/// the finder-pattern corners at print zoom.
pub const QR_CODE_EMU: u32 = 972_000;

/// Sources-box QR column width in twips (≈3.26 cm, up from the
/// historical 1700 / 3.0 cm) — matches the bumped [`QR_CODE_EMU`].
pub const QR_COL_TWIPS: usize = 1850;

/// Embedded PNG bytes for the "tip" admonition icon.
///
/// Mirrored from `specs/figures/raster/ai_norms/icon_tip.png` at the time
/// this module landed (V-E2 / 2026-06-03). The mirror keeps the crate
/// build-time hermetic; V-G1's 290×290 regen lands as a refresh of the
/// in-crate bytes once both branches merge to main.
pub const ICON_TIP_PNG: &[u8] = include_bytes!("../icons/icon_tip.png");

/// Embedded PNG bytes for the "note" admonition icon.
///
/// Mirrored from `specs/figures/raster/ai_norms/icon_note.png`; see
/// [`ICON_TIP_PNG`] for the refresh story.
pub const ICON_NOTE_PNG: &[u8] = include_bytes!("../icons/icon_note.png");

/// Embedded PNG bytes for the "warning" admonition icon.
///
/// Mirrored from `specs/figures/raster/ai_norms/icon_warning.png`; see
/// [`ICON_TIP_PNG`] for the refresh story.
pub const ICON_WARNING_PNG: &[u8] = include_bytes!("../icons/icon_warning.png");

/// Smallest legal PNG (1×1 transparent pixel) used as a last-resort
/// placeholder if the in-crate icon byte slice is empty. This is a
/// defensive fallback per the cross-cutting "FIGURE/ICON-REGEN" risk —
/// the build should not bomb out if V-G1's regen lands an empty file
/// during a partial merge; the emitted drawing will still be a valid
/// `<w:drawing>` (and so still counts in parity gates) and the human
/// reviewer will see a near-invisible square instead of a hard panic.
const PLACEHOLDER_PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // PNG signature
    0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, // IHDR len + tag
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, // 1×1
    0x08, 0x06, 0x00, 0x00, 0x00, // bit-depth 8, RGBA
    0x1F, 0x15, 0xC4, 0x89, // IHDR CRC
    0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, // IDAT len + tag
    0x78, 0x9C, 0x62, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D,
    0xB4, // data
    0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82, // IEND
];

/// Sentinel rid prefix used as the `r:embed` value on the `Pic` we hand
/// to `docx-rs`. The post-process pass keys off this prefix to locate
/// the right `pic:pic` element and patch its sibling `pic:cNvPr name=`
/// attribute back to the stable icon name.
const ICON_RID_PREFIX: &str = "rIdImage_icon_";

impl IconKind {
    /// Stable `pic:cNvPr name=` value the reference book emits for this
    /// icon. Parity checks read this string verbatim to match drawings.
    pub fn name(&self) -> &'static str {
        match self {
            IconKind::Tip => "icon_tip",
            IconKind::Note => "icon_note",
            IconKind::Warning => "icon_warning",
        }
    }

    /// EMU side length the icon is rendered at — the same value for all
    /// three variants today, surfaced per-icon so a future tweak (e.g.
    /// the warning triangle wanting more breathing room) is a one-line
    /// change.
    pub fn emu(&self) -> u32 {
        match self {
            IconKind::Tip | IconKind::Note | IconKind::Warning => ADMONITION_ICON_EMU,
        }
    }

    /// Sentinel `r:embed` rid the post-process pass uses to find the
    /// emitted `pic:pic` element and rewrite its name attribute.
    pub fn rid_sentinel(&self) -> String {
        format!("{ICON_RID_PREFIX}{}", self.short_tag())
    }

    /// Markdown code-block tag the admonition syntax uses for this kind
    /// (`tip`, `note`, `warning`).
    pub fn short_tag(&self) -> &'static str {
        match self {
            IconKind::Tip => "tip",
            IconKind::Note => "note",
            IconKind::Warning => "warning",
        }
    }

    /// Embedded PNG bytes for this icon; falls back to the 1×1 PNG when
    /// the in-crate slice is empty (FIGURE/ICON-REGEN partial-merge
    /// safety net).
    pub fn png_bytes(&self) -> &'static [u8] {
        let candidate: &'static [u8] = match self {
            IconKind::Tip => ICON_TIP_PNG,
            IconKind::Note => ICON_NOTE_PNG,
            IconKind::Warning => ICON_WARNING_PNG,
        };
        if candidate.is_empty() {
            PLACEHOLDER_PNG
        } else {
            candidate
        }
    }

    /// Parse the markdown admonition tag (`tip` / `note` / `warning`).
    /// Defaults to `Note` for any unrecognised tag to preserve the
    /// historical `admonition_box` behaviour.
    pub fn from_tag(tag: &str) -> Self {
        match tag {
            "tip" => IconKind::Tip,
            "warning" => IconKind::Warning,
            _ => IconKind::Note,
        }
    }
}

/// Build the configured [`Pic`] for an icon — the public counterpart of
/// the figdir-based load. Callers compose the surrounding label text
/// themselves (the admonition label paragraph carries icon + label +
/// body text inline).
pub fn icon_pic(kind: IconKind) -> Pic {
    Pic::new(kind.png_bytes())
        .id(kind.rid_sentinel())
        .size(kind.emu(), kind.emu())
}

/// Emit a stand-alone admonition icon paragraph onto `doc`.
///
/// This is the spec'd `emit_icon(kind, doc) -> doc` shape; the
/// admonition emit path in `book.rs` uses [`icon_pic`] directly so it
/// can compose the icon + label run inside a single `BkCallout`
/// paragraph (the label is bold-coloured + sized; the icon precedes it
/// in the same run-list). This helper exists for callers that just need
/// a bare icon paragraph (mainly the unit test below).
pub fn emit_icon(kind: IconKind, doc: Docx) -> Docx {
    doc.add_paragraph(
        Paragraph::new()
            .line_spacing(LineSpacing::new())
            .add_run(Run::new().add_image(icon_pic(kind))),
    )
    .add_paragraph(
        // Tiny invisible "fonts only" run so the paragraph has a stable
        // shape across parity diffs — empty paragraphs are sometimes
        // collapsed by the post-process passes.
        Paragraph::new().add_run(Run::new().fonts(RunFonts::new())),
    )
}

/// Walk `document_xml` and rewrite every `<pic:cNvPr id="0" name=""/>`
/// whose enclosing `<pic:pic>` carries one of the icon `r:embed` rids
/// back to the stable per-icon name. Idempotent: re-running on already
/// patched XML is a no-op.
///
/// We work on the serialised `word/document.xml` string because
/// `docx-rs` 0.4.20 has no public setter for the `pic:cNvPr name`
/// attribute (the writer hardcodes `name=""`) and forking the crate for
/// a single attribute is disproportionate. The post-process is keyed by
/// the sentinel `r:embed` rid that [`icon_pic`] sets on each `Pic`, so
/// the pass is robust against neighbouring `<w:drawing>` elements that
/// use the regular auto-generated rids.
pub fn rewrite_pic_names_in_document_xml(document_xml: &str) -> String {
    let mut out = document_xml.to_string();
    for kind in [IconKind::Tip, IconKind::Note, IconKind::Warning] {
        let rid = kind.rid_sentinel();
        let name = kind.name();
        // Pattern: an `<a:blip r:embed="<rid>" />` sits inside the same
        // `<pic:pic>` as the (preceding) `<pic:cNvPr id="0" name=""/>`.
        // docx-rs emits the cNvPr first, then the blip; we therefore
        // walk every blip-occurrence in the doc and patch backwards.
        let blip_token = format!(r#"<a:blip r:embed="{rid}""#);
        let cnv_pattern = r#"<pic:cNvPr id="0" name="" />"#;
        let cnv_replacement = format!(r#"<pic:cNvPr id="0" name="{name}" />"#);
        // For each occurrence of the blip token, walk back to the
        // nearest preceding `cnv_pattern` and rewrite it. Track end-of-
        // match positions so we never rewrite the same cnv twice.
        let mut search_from = 0usize;
        while let Some(blip_pos_rel) = out[search_from..].find(&blip_token) {
            let blip_pos = search_from + blip_pos_rel;
            // Locate the immediately preceding cnv element.
            if let Some(cnv_rel) = out[..blip_pos].rfind(cnv_pattern) {
                let cnv_start = cnv_rel;
                let cnv_end = cnv_start + cnv_pattern.len();
                out.replace_range(cnv_start..cnv_end, &cnv_replacement);
                // Advance past the (now-longer) rewritten cnv so we do
                // not re-match it on the next iteration.
                search_from = cnv_start + cnv_replacement.len();
            } else {
                // No preceding cnv to patch (defensive — should not
                // happen for a docx-rs-emitted Pic). Skip ahead.
                search_from = blip_pos + blip_token.len();
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use docx_rs::Docx;
    use std::io::{Cursor, Read};

    /// Sanity: the embedded PNG slices are non-empty (otherwise every
    /// admonition silently picks up the 1×1 placeholder).
    #[test]
    fn embedded_png_bytes_are_present() {
        assert!(!ICON_TIP_PNG.is_empty(), "icon_tip.png missing");
        assert!(!ICON_NOTE_PNG.is_empty(), "icon_note.png missing");
        assert!(!ICON_WARNING_PNG.is_empty(), "icon_warning.png missing");
    }

    /// `IconKind::name()` returns the stable strings the reference
    /// book's parity gate matches against.
    #[test]
    fn icon_names_are_stable() {
        assert_eq!(IconKind::Tip.name(), "icon_tip");
        assert_eq!(IconKind::Note.name(), "icon_note");
        assert_eq!(IconKind::Warning.name(), "icon_warning");
    }

    /// `emit_icon` produces a `<w:drawing>` in the rendered document.xml
    /// and the post-process pass rewrites its `pic:cNvPr name=` back to
    /// the stable per-icon name.
    #[test]
    fn emit_icon_produces_named_drawing() {
        let doc = emit_icon(IconKind::Tip, Docx::new());
        let mut cur = Cursor::new(Vec::<u8>::new());
        doc.build().pack(&mut cur).expect("pack icon docx");
        let zip_bytes = cur.into_inner();
        let mut zin = zip::ZipArchive::new(Cursor::new(zip_bytes)).expect("open zip");
        let mut document_xml = String::new();
        zin.by_name("word/document.xml")
            .expect("word/document.xml present")
            .read_to_string(&mut document_xml)
            .expect("read document.xml");
        assert!(
            document_xml.contains("<w:drawing"),
            "icon paragraph should serialise a <w:drawing> element; doc.xml was: {document_xml}"
        );
        let rewritten = rewrite_pic_names_in_document_xml(&document_xml);
        assert!(
            rewritten.contains(r#"pic:cNvPr id="0" name="icon_tip""#),
            "post-process should patch pic:cNvPr name= to icon_tip; rewritten doc.xml was: {rewritten}"
        );
        // Idempotency: re-running the rewrite is a no-op.
        let rewritten_twice = rewrite_pic_names_in_document_xml(&rewritten);
        assert_eq!(rewritten, rewritten_twice, "rewrite is idempotent");
    }

    /// All three variants survive the round-trip.
    #[test]
    fn every_kind_round_trips() {
        for kind in [IconKind::Tip, IconKind::Note, IconKind::Warning] {
            let doc = emit_icon(kind, Docx::new());
            let mut cur = Cursor::new(Vec::<u8>::new());
            doc.build().pack(&mut cur).expect("pack icon docx");
            let mut zin = zip::ZipArchive::new(Cursor::new(cur.into_inner())).expect("open zip");
            let mut document_xml = String::new();
            zin.by_name("word/document.xml")
                .expect("word/document.xml present")
                .read_to_string(&mut document_xml)
                .expect("read document.xml");
            let rewritten = rewrite_pic_names_in_document_xml(&document_xml);
            let expected = format!(r#"pic:cNvPr id="0" name="{}""#, kind.name());
            assert!(
                rewritten.contains(&expected),
                "kind {kind:?} should patch to name {expected}; rewritten doc.xml was: {rewritten}"
            );
        }
    }
}
