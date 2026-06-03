//! Wave-9 AI-Norms parity (POLISH — captioned-tables + caption-style,
//! 2026-06-03): tables emit `tblStyle=TableGrid`, the table caption is the
//! IMMEDIATE preceding paragraph (no intervening empty spacer), and figure +
//! table captions adopt `pStyle=BkCaption` when
//! [`BookMeta::body_render_use_bk_styles`] is true. These three properties
//! together unblock the parity gate's `captioned_table_parity` (expects
//! `Table N.` caption + `<w:tblHeader/>` on row 1) and `BkCaption` style-usage
//! counts.

use std::io::{Cursor, Read};
use std::path::Path;

use agentic_export::book::{render_book, BookMeta};

fn document_xml(bytes: Vec<u8>) -> String {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).expect("open zip");
    let mut xml = String::new();
    zip.by_name("word/document.xml")
        .expect("document.xml present")
        .read_to_string(&mut xml)
        .expect("read document.xml");
    xml
}

const TABLE_MD: &str =
    "# Chapter\n\nTable: Demo caption\n\n| A | B |\n|---|---|\n| 1 | 2 |\n";

const FIGURE_TABLE_MD: &str = "# Chapter\n\n\
![A figure](figures/c1/missing.png)\n\n\
Some prose.\n\n\
Table: Demo caption\n\n\
| A | B |\n|---|---|\n| 1 | 2 |\n";

/// Task 1 — captioned-table parity: the rendered docx must contain a
/// `<w:tbl>` block that
///   1. carries `tblStyle="TableGrid"`,
///   2. has its IMMEDIATELY preceding paragraph being the "Table N." caption
///      (no empty spacer paragraph between caption and table), and
///   3. has `<w:tblHeader/>` on row 1 (added by `mark_header_rows`).
#[test]
fn table_renders_with_tblstyle_caption_and_header() {
    let meta = BookMeta {
        title: "Wave9 Tables".into(),
        body_render_use_bk_styles: true,
        ..Default::default()
    };
    let bytes = render_book(&meta, &[("c1".into(), TABLE_MD.into())], Path::new("."))
        .expect("render");
    let xml = document_xml(bytes);

    // (1) TableGrid style applied to the table block.
    assert!(
        xml.contains("w:tblStyle w:val=\"TableGrid\"")
            || xml.contains("w:val=\"TableGrid\""),
        "table must reference the TableGrid style id; got document.xml without it"
    );
    // (2) <w:tblHeader/> emitted on the first row (by mark_header_rows).
    assert!(
        xml.contains("<w:tblHeader"),
        "row 1 must be marked as a header row via <w:tblHeader>"
    );
    // (3) The LAST paragraph before <w:tbl> is the caption — the parity
    //     gate's `preceding_paragraph_is_table_caption` sniff walks back
    //     a window and checks the last `<w:p` for a "Table N." prefix.
    //     The previous emitter inserted an empty spacer paragraph between
    //     the caption and the table; with that gone the caption sits
    //     directly above <w:tbl>.
    let tbl = xml.find("<w:tbl>").expect("a <w:tbl> in document.xml");
    let preceding = &xml[..tbl];
    let last_p = preceding
        .rfind("<w:p ")
        .or_else(|| preceding.rfind("<w:p>"))
        .expect("a <w:p before <w:tbl");
    // Concatenate <w:t> bodies inside the last paragraph and check the
    // "Table " prefix is present in the immediately-preceding paragraph.
    let last_para = &xml[last_p..tbl];
    let mut caption_text = String::new();
    let mut rest = last_para;
    while let Some(open) = rest.find("<w:t") {
        let after = &rest[open..];
        let gt = after.find('>').expect("w:t open");
        let body_start = open + gt + 1;
        let body_end_rel = rest[body_start..]
            .find("</w:t>")
            .expect("w:t close");
        caption_text.push_str(&rest[body_start..body_start + body_end_rel]);
        rest = &rest[body_start + body_end_rel + "</w:t>".len()..];
    }
    assert!(
        caption_text.trim_start().starts_with("Table "),
        "caption paragraph immediately above <w:tbl> must start with 'Table '; got {caption_text:?}"
    );
}

/// Task 2 — `BkCaption` style applied to BOTH figure and table captions
/// when `body_render_use_bk_styles=true`. Under the default (false) profile,
/// the historical `Caption` style id continues to apply (regression guard).
#[test]
fn captions_use_bkcaption_under_flag() {
    let meta = BookMeta {
        title: "Wave9 BkCaption".into(),
        body_render_use_bk_styles: true,
        ..Default::default()
    };
    let bytes = render_book(
        &meta,
        &[("c1".into(), FIGURE_TABLE_MD.into())],
        Path::new("."),
    )
    .expect("render");
    let xml = document_xml(bytes);

    let bk_count = xml.matches("w:pStyle w:val=\"BkCaption\"").count();
    assert!(
        bk_count > 0,
        "expected BkCaption pStyle on captions under flag=true; got count=0"
    );
    // Under flag=true the legacy `Caption` style id must NOT leak onto the
    // figure/table caption paragraphs — otherwise the BkCaption count
    // would split across two style ids and the parity gate would still
    // see BkCaption=0.
    assert!(
        !xml.contains("w:pStyle w:val=\"Caption\""),
        "flag=true unexpectedly emitted the legacy Caption pStyle on a caption paragraph"
    );
}

#[test]
fn captions_use_legacy_caption_under_default_flag() {
    let meta = BookMeta {
        title: "Wave9 LegacyCap".into(),
        // body_render_use_bk_styles defaults to false.
        ..Default::default()
    };
    let bytes = render_book(&meta, &[("c1".into(), TABLE_MD.into())], Path::new("."))
        .expect("render");
    let xml = document_xml(bytes);
    assert!(
        xml.contains("w:pStyle w:val=\"Caption\""),
        "flag=false must keep the historical Caption pStyle (regression guard)"
    );
    assert!(
        !xml.contains("w:pStyle w:val=\"BkCaption\""),
        "flag=false must not switch caption pStyle to BkCaption (opt-in regression)"
    );
}

/// Task 3 — `TableofFigures` style-usage stays within ±10 % of the reference
/// after the back-matter list-of-figures/list-of-tables render. The reference
/// has 155 entries (133 figures + 22 tables); a small sample chapter with 1
/// figure + 1 table produces ~2 TableofFigures entries when Word populates
/// the lists. The engine itself does NOT emit `pStyle="TableofFigures"`
/// directly — the style is applied by Word when it expands the `TOC \c`
/// fields under the list-of-figures heading. So the test asserts the engine
/// does not over-emit a pStyle reference (a 1 ± 10 % bound is overkill for
/// the engine emit-side; the gate's overshoot is downstream from too many
/// SEQ Figure captions, not a style misapplication).
#[test]
fn engine_does_not_directly_emit_tableoffigures_pstyle() {
    // The TableofFigures pStyle is supposed to be applied by Word during
    // TOC-field expansion, not by the engine. If the engine emits this
    // pStyle directly on a body paragraph, the parity gate's
    // `style_usage_parity` for TableofFigures would over-count by a
    // factor of (1 + SEQ Figure + SEQ Table) — exactly the 283 vs 155
    // overshoot observed in Wave 9. This regression guard asserts the
    // engine itself never writes the TableofFigures pStyle.
    let meta = BookMeta {
        title: "Wave9 TOF".into(),
        body_render_use_bk_styles: true,
        ..Default::default()
    };
    let bytes = render_book(
        &meta,
        &[("c1".into(), FIGURE_TABLE_MD.into())],
        Path::new("."),
    )
    .expect("render");
    let xml = document_xml(bytes);
    let direct = xml.matches("w:pStyle w:val=\"TableofFigures\"").count();
    assert_eq!(
        direct, 0,
        "engine must not directly emit pStyle=\"TableofFigures\" — Word \
         applies that style during TOC \\c field expansion (any count > 0 \
         means the engine is over-counting, causing the parity gate overshoot)",
    );
}
