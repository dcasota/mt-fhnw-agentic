//! Raw-XML `word/numbering.xml` emitter (Round-V zone D, AI-Norms parity,
//! 2026-06-03).
//!
//! docx-rs ships an empty / minimal `numbering.xml` part — sufficient for
//! the legacy unstyled bullet/numbered output but **not** for matching the
//! reference book `AI_Norms_and_Regulations_BOOK.docx`, which declares
//! **9 `<w:abstractNum>` definitions and 9 `<w:num>` instances** covering
//! the bookkit list family (`ListBullet`, `ListBullet2`, `ListBullet3`,
//! `ListNumber`, `ListNumber2`, `ListNumber3`, plus three legacy ranges
//! singleLevel decimal/bullet definitions referenced by direct numId on
//! direct-formatted paragraphs).
//!
//! Round V zone D introduces this module as a sibling of `styles_xml`: the
//! reference `numbering.xml` is extracted from the reference book once at
//! Wave time, embedded at compile time via `include_str!`, and re-emitted
//! verbatim when the AI-Norms parity flag is set. A finalize-pass in
//! [`crate::book::postprocess_docx_inner_layout`] swaps the docx-rs-authored
//! `word/numbering.xml` with this string so paragraph-level `<w:numPr>`
//! references resolve to the expected list formatting in Word.
//!
//! Two **glyph color flavors** are derived from the verbatim reference XML
//! by post-processing the embedded constant: an ACCENT (`0B5C9E`) flavour
//! for body bullets (Designer profile zone-D switch) and a GREY (`666666`)
//! flavour for secondary lists (quiz answers, sub-numbering). Callers pick
//! the flavour at injection time via [`emit_numbering_xml`].

/// Reference `word/numbering.xml` from `AI_Norms_and_Regulations_BOOK.docx`,
/// embedded at compile time so the emitter is hermetic (no runtime file I/O).
/// Wave-0 fingerprint: 7,476 bytes (declares 9 `<w:abstractNum>` definitions
/// + 9 `<w:num>` instances).
const REFERENCE_NUMBERING_XML: &str =
    include_str!("../tests/fixtures/numbering_reference.xml");

/// Glyph color flavour selector for [`emit_numbering_xml`].
///
/// The reference `numbering.xml` does NOT declare glyph colour on any
/// `<w:rPr>` inside `<w:lvl>` (Symbol font, default colour). Round V zone D
/// adds a colour `<w:rPr><w:color w:val="…"/></w:rPr>` to every level run
/// properties block so bullets inherit the bookkit accent rather than the
/// default black.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumberingFlavour {
    /// Verbatim reference (no glyph-color injection).
    Verbatim,
    /// Designer bookkit: bullets coloured ACCENT `0B5C9E` (Round V zone D
    /// switch from NAVY `1F3864` to ACCENT, matching the inline-glyph
    /// change in [`crate::book`]).
    Accent,
    /// Secondary numbering: glyphs coloured GREY `666666` (quiz answers
    /// and sibling secondary-numbered lists).
    Grey,
}

/// Emit the complete `<w:numbering>` document (XML declaration + namespace
/// preamble + 9 `<w:abstractNum>` + 9 `<w:num>` elements).
///
/// Returns the embedded reference XML, optionally post-processed to inject
/// a glyph colour on every level definition.
pub fn emit_numbering_xml(flavour: NumberingFlavour) -> String {
    match flavour {
        NumberingFlavour::Verbatim => REFERENCE_NUMBERING_XML.to_string(),
        NumberingFlavour::Accent => inject_glyph_color(REFERENCE_NUMBERING_XML, "0B5C9E"),
        NumberingFlavour::Grey => inject_glyph_color(REFERENCE_NUMBERING_XML, "666666"),
    }
}

/// Count `<w:abstractNum ` elements in the emitted numbering document. Used
/// by the parity test to assert the 9-abstractNum target.
pub fn count_abstract_nums(xml: &str) -> usize {
    xml.matches("<w:abstractNum ").count()
}

/// Count `<w:num ` instances (concrete numId references). Used by the
/// parity test to assert the 9-numId target.
pub fn count_num_instances(xml: &str) -> usize {
    xml.matches("<w:num ").count()
}

/// Walk every `<w:lvl …>` block and inject a `<w:color w:val="HEX"/>` into
/// (or onto) its `<w:rPr>` so the glyph itself renders in the chosen colour
/// without disturbing the level's other run-property settings (font, size).
///
/// Idempotency: re-running with the same colour is a no-op when the level
/// already carries that colour; a different colour replaces the previous
/// value.
fn inject_glyph_color(xml: &str, hex: &str) -> String {
    let color_tag = format!("<w:color w:val=\"{hex}\"/>");
    let mut out = String::with_capacity(xml.len() + 256);
    let mut cursor = 0;
    while let Some(start) = xml[cursor..].find("<w:lvl ") {
        let lvl_open_abs = cursor + start;
        let Some(lvl_close_rel) = xml[lvl_open_abs..].find("</w:lvl>") else {
            // Malformed; bail out and copy the remainder verbatim.
            break;
        };
        let lvl_close_abs = lvl_open_abs + lvl_close_rel;
        out.push_str(&xml[cursor..lvl_open_abs]);
        let lvl = &xml[lvl_open_abs..lvl_close_abs];
        out.push_str(&rewrite_lvl(lvl, &color_tag));
        out.push_str("</w:lvl>");
        cursor = lvl_close_abs + "</w:lvl>".len();
    }
    out.push_str(&xml[cursor..]);
    out
}

/// Inject (or replace) the `<w:color>` element inside a single `<w:lvl>`
/// block's `<w:rPr>`. If no `<w:rPr>` exists, one is appended just before
/// `</w:lvl>` carrying only the colour element.
fn rewrite_lvl(lvl: &str, color_tag: &str) -> String {
    // Case 1: lvl already has rPr. Drop any existing <w:color …/> and
    // splice the new one in just before </w:rPr>.
    if let Some(rpr_start) = lvl.find("<w:rPr>") {
        let rpr_end = lvl[rpr_start..]
            .find("</w:rPr>")
            .map(|r| rpr_start + r)
            .unwrap_or(lvl.len());
        let head = &lvl[..rpr_start + "<w:rPr>".len()];
        let inner_old = &lvl[rpr_start + "<w:rPr>".len()..rpr_end];
        let inner_stripped = strip_existing_color(inner_old);
        let tail = &lvl[rpr_end..];
        let mut s = String::with_capacity(lvl.len() + color_tag.len());
        s.push_str(head);
        s.push_str(&inner_stripped);
        s.push_str(color_tag);
        s.push_str(tail);
        return s;
    }
    // Case 2: no rPr — append one just before the lvl's end (caller
    // appends </w:lvl>; we return the inner content unchanged + new rPr).
    let mut s = String::with_capacity(lvl.len() + color_tag.len() + 32);
    s.push_str(lvl);
    s.push_str("<w:rPr>");
    s.push_str(color_tag);
    s.push_str("</w:rPr>");
    s
}

/// Remove any `<w:color w:val="…"/>` element from a string. Idempotent.
fn strip_existing_color(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while let Some(start) = s[i..].find("<w:color ") {
        out.push_str(&s[i..i + start]);
        let abs_start = i + start;
        if let Some(end) = s[abs_start..].find("/>") {
            i = abs_start + end + "/>".len();
        } else {
            // Malformed; preserve remainder.
            out.push_str(&s[abs_start..]);
            return out;
        }
    }
    out.push_str(&s[i..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emits_9_abstract_nums() {
        let xml = emit_numbering_xml(NumberingFlavour::Verbatim);
        assert_eq!(
            count_abstract_nums(&xml),
            9,
            "reference declares 9 abstractNum definitions"
        );
    }

    #[test]
    fn emits_9_num_instances() {
        let xml = emit_numbering_xml(NumberingFlavour::Verbatim);
        assert_eq!(
            count_num_instances(&xml),
            9,
            "reference declares 9 num instances"
        );
    }

    #[test]
    fn accent_flavour_injects_color() {
        let xml = emit_numbering_xml(NumberingFlavour::Accent);
        assert!(
            xml.contains("<w:color w:val=\"0B5C9E\"/>"),
            "accent flavour must inject ACCENT glyph colour into every level"
        );
        // Should not leave NAVY behind (sanity).
        assert!(!xml.contains("<w:color w:val=\"1F3864\"/>"));
    }

    #[test]
    fn grey_flavour_injects_color() {
        let xml = emit_numbering_xml(NumberingFlavour::Grey);
        assert!(xml.contains("<w:color w:val=\"666666\"/>"));
        assert!(!xml.contains("<w:color w:val=\"0B5C9E\"/>"));
    }

    #[test]
    fn accent_flavour_preserves_abstract_count() {
        let xml = emit_numbering_xml(NumberingFlavour::Accent);
        assert_eq!(count_abstract_nums(&xml), 9);
        assert_eq!(count_num_instances(&xml), 9);
    }
}
