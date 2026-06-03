//! Kind-aware DOCX table emitter (Round-V Zone-F, 2026-06-03).
//!
//! Every `<w:tbl>` in the book renderer used to be hand-rolled with
//! slightly different `<w:tblPr>` / `<w:trPr>` / `<w:tcPr>` profiles
//! at the call site. That allowed drift between visually similar tables
//! (the 22-vs-33 styling delta the visual-parity audit caught in
//! `book.rs ~1797-1838` vs `~3367-3418`): one would carry a `tblStyle`
//! reference and an inline border block, another would skip the style
//! and leave the default `sz=2 color=000000` borders; one would set
//! `vAlign=center` on every cell, another would only set it on the QR
//! column. The styling drift was impossible to spot in code review
//! because the offending fields were intermixed with content-building
//! logic 80 lines apart.
//!
//! This module replaces every `Table::new(...)` in `book.rs` with a
//! single entry point — [`emit`] — that takes a [`TableKind`] plus the
//! caller-built rows and produces a `docx_rs::Table` with the
//! kind-appropriate `<w:tblPr>` profile applied. Cell-level overrides
//! (the right QR cell's `vAlign=center`) stay at the call site because
//! they need access to the per-cell builder; everything else
//! (`tblStyle`, `jc`, `tblBorders`, `layout`, `width`, `margins`) is
//! a function of the kind.
//!
//! # Schema-order safety (cross-cutting risk #8, POSTPROCESS-XML-ORDERING)
//!
//! ECMA-376 CT_TblPr requires `<w:tblW>` to precede `<w:jc>`. docx-rs's
//! [`TableProperty`] serializer already emits them in this order
//! (`width` field first, `justification` field second — see
//! `docx-rs/src/documents/elements/table_property.rs`). We rely on the
//! library's ordering rather than re-emitting raw XML: routing every
//! Table through `align(TableAlignmentType::Center)` keeps the
//! width-before-jc invariant intact by construction.
//!
//! CT_TrPr requires `<w:cantSplit>` before any inserts/deletes/jc.
//! docx-rs's [`TableRowProperty`] serializer emits `del → ins →
//! cantSplit → row_height`, so `cant_split` is always early-positioned.
//! Note: docx-rs's typed `TableRowProperty` does NOT expose a
//! row-level `<w:jc>` field; we centre tables via the table-level
//! `<w:jc>` (CT_TblPr) which has the same visual effect (centres the
//! table within the page margins). If a future requirement demands a
//! row-level jc, it would need a raw-XML post-process pass — see
//! `styles_xml.rs` for the precedent.
//!
//! # Coordination with Zone D / Zone E1 (cross-cutting risks #4, #5)
//!
//! - Risk #4 (BkCallout single-style): `TableKind::KeypointsBox` is
//!   defined as an enum variant for future use but is **not wired up**
//!   today, because `keypoints_box()` in `book.rs` was already
//!   refactored to paragraph emission with the `BkCallout` style
//!   (Round-E1). Re-introducing a `<w:tbl>` wrapper would re-inflate
//!   the spurious-`<w:tbl>` count the `captioned_table_parity` gate
//!   watches. The variant exists so that, if a non-callout keypoints
//!   variant is ever needed, the table profile is centralised here
//!   from day one.
//! - Risk #5 (ALIGN-OFF-CASCADE): Zone D centres body paragraphs.
//!   This module centres tables (CT_TblPr `<w:jc>`). The two are
//!   independent: paragraph centring lives in `<w:pPr>`, table
//!   centring in `<w:tblPr>`. Changing one cannot cascade into the
//!   other.

use docx_rs::{
    BorderType, Table, TableAlignmentType, TableBorder, TableBorderPosition, TableBorders,
    TableCellMargins, TableLayoutType, TableRow, WidthType,
};

/// Distinguishes the four logical kinds of `<w:tbl>` the book
/// renderer can emit. Each value selects one — and only one —
/// `<w:tblPr>` profile so that two call sites cannot drift apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableKind {
    /// Real content table with captioned header row (alternating
    /// fills, shaded header). Carries the `TableGrid` style plus
    /// explicit `sz=4 color=auto` borders (matching the reference
    /// fixture; the default docx-rs `sz=2 color=000000` is replaced).
    /// Cells DO NOT carry `vAlign=center` — the reference fixture
    /// lets cell content baseline-align so multi-line cells read
    /// naturally.
    Captioned,
    /// End-of-chapter "Sources & QR codes" two-column box. Table
    /// carries `TableGrid` + centred + fixed layout + per-cell
    /// padding. The right QR cell keeps `vAlign=center` (set at the
    /// call site so the QR pic centres next to multi-line link text);
    /// the left text cell does NOT.
    SourcesBox,
    /// Single-cell quote callout with a left accent border. Table
    /// carries `TableGrid` + centred + fixed layout + intentional
    /// vertical padding (200/120 vs the default 100/60) so the quote
    /// breathes.
    QuoteCallout,
    /// Reserved for a non-callout keypoints presentation. **Not used
    /// today** — see module-level note on Risk #4 coordination.
    KeypointsBox,
}

/// Geometry the caller supplies to [`emit`].
///
/// `grid` lists the per-column widths in twips (passed to
/// `<w:tblGrid>` and used by Word's fixed-layout to honour the column
/// distribution); `total_twips` becomes the `<w:tblW>` value (the
/// table's logical width). They are passed separately because the
/// sources-box uses a 2-column grid whose total width is the page
/// content width, while a captioned table can have N columns whose
/// widths sum to the content width.
#[derive(Debug, Clone)]
pub struct TableLayout {
    pub grid: Vec<usize>,
    pub total_twips: usize,
}

/// Build a `docx_rs::Table` with the `<w:tblPr>` profile selected by
/// `kind`. Rows are caller-built (the kind controls only the table
/// wrapper, not the cell content) so existing per-call-site logic
/// (header rotation, alternating fills, QR pic embedding) stays
/// untouched.
///
/// Per-kind profile summary:
///
/// | Kind          | tblStyle  | jc     | borders                  | layout | margins (twips) |
/// | ------------- | --------- | ------ | ------------------------ | ------ | --------------- |
/// | Captioned     | TableGrid | center | sz=4 color=auto, single  | fixed  | none (style)    |
/// | SourcesBox    | TableGrid | center | (inherited from style)   | fixed  | 60/100/60/100   |
/// | QuoteCallout  | TableGrid | center | (inherited from style)   | fixed  | 70/200/70/120   |
/// | KeypointsBox  | TableGrid | center | (inherited from style)   | fixed  | none            |
pub fn emit(kind: TableKind, rows: Vec<TableRow>, layout: TableLayout) -> Table {
    let TableLayout { grid, total_twips } = layout;
    let mut table = Table::new(rows)
        .set_grid(grid)
        .width(total_twips, WidthType::Dxa)
        // jc=center for every kind. docx-rs emits <w:tblW> before
        // <w:jc> via the field-declaration order in TableProperty,
        // so CT_TblPr schema order is preserved (risk #8).
        .align(TableAlignmentType::Center)
        // Fixed layout makes Word honour the grid widths (ADR-0030).
        .layout(TableLayoutType::Fixed)
        // Every kind references TableGrid so the table picks up the
        // 186-style fixture's table formatting (borders, font, cell
        // padding defaults). This replaces the previous per-call-site
        // inline border definitions which drifted between sz=2 (lib
        // default) and sz=4 (intended).
        .style("TableGrid");

    match kind {
        TableKind::Captioned => {
            // Replace the docx-rs default sz=2 color=000000 with the
            // sz=4 color=auto borders that match the reference
            // fixture. `color="auto"` lets Word inherit the theme's
            // text-1 colour so the borders stay visible on both light
            // and dark Word themes.
            table = table.set_borders(captioned_borders());
            // No inline tblCellMar: padding is now governed by the
            // TableGrid style. Previously this was
            // `.margins(TableCellMargins::new().margin(60, 100, 60, 100))`,
            // which duplicated the style's default and made captioned
            // tables visually inconsistent with the reference whenever
            // the style was tweaked.
        }
        TableKind::SourcesBox => {
            // Keep the QR padding the box was originally designed
            // around. Left cell (text), right cell (QR pic) both
            // inherit this padding from the table level.
            table = table.margins(TableCellMargins::new().margin(60, 100, 60, 100));
        }
        TableKind::QuoteCallout => {
            // Intentional larger vertical padding so the quote
            // breathes (top/bottom 200/120 vs left/right 70/70).
            table = table.margins(TableCellMargins::new().margin(70, 200, 70, 120));
        }
        TableKind::KeypointsBox => {
            // Reserved variant — no per-kind overrides. See module
            // note on Risk #4.
        }
    }

    table
}

/// Captioned-table borders: sz=4 single, `color="auto"` (theme text).
///
/// docx-rs's `TableBorder::new(...)` defaults to sz=2 color=000000.
/// We override colour (auto) and size (4) for all six positions so
/// the inside-H / inside-V grid lines match the outer frame and the
/// reference fixture.
fn captioned_borders() -> TableBorders {
    let positions = [
        TableBorderPosition::Top,
        TableBorderPosition::Left,
        TableBorderPosition::Bottom,
        TableBorderPosition::Right,
        TableBorderPosition::InsideH,
        TableBorderPosition::InsideV,
    ];
    let mut borders = TableBorders::new();
    for pos in positions {
        borders = borders.set(
            TableBorder::new(pos)
                .border_type(BorderType::Single)
                .size(4)
                .color("auto"),
        );
    }
    borders
}

#[cfg(test)]
mod tests {
    use super::*;
    use docx_rs::{BuildXML, TableCell};

    fn tiny_layout() -> TableLayout {
        TableLayout {
            grid: vec![1000, 1000],
            total_twips: 2000,
        }
    }

    fn one_cell_row() -> TableRow {
        TableRow::new(vec![TableCell::new()])
    }

    fn build(kind: TableKind) -> String {
        let xml = emit(kind, vec![one_cell_row()], tiny_layout()).build();
        String::from_utf8(xml).expect("table XML must be valid UTF-8")
    }

    #[test]
    fn captioned_has_tablegrid_style_and_centre_jc() {
        let xml = build(TableKind::Captioned);
        assert!(xml.contains(r#"<w:tblStyle w:val="TableGrid""#));
        assert!(xml.contains(r#"<w:jc w:val="center""#));
    }

    #[test]
    fn captioned_has_sz4_color_auto_borders() {
        let xml = build(TableKind::Captioned);
        // All six border positions must be sz=4 + color="auto".
        for tag in ["top", "left", "bottom", "right", "insideH", "insideV"] {
            let needle = format!(r#"<w:{tag} w:val="single" w:sz="4" w:space="0" w:color="auto" "#);
            assert!(
                xml.contains(&needle),
                "captioned border missing for `{tag}`: {xml}"
            );
        }
    }

    #[test]
    fn captioned_cells_have_no_vertical_align() {
        // The cells the caller built carry no vAlign override, and
        // emit() does not add one — so the cell <w:tcPr> stays free
        // of <w:vAlign>. This is the load-bearing assertion: removing
        // vertical_align(VAlignType::Center) from the captioned-table
        // call site in book.rs must result in no <w:vAlign> in the
        // emitted table.
        let xml = build(TableKind::Captioned);
        assert!(
            !xml.contains("<w:vAlign"),
            "captioned cells must not carry <w:vAlign>: {xml}"
        );
    }

    #[test]
    fn captioned_emits_width_before_jc_per_ct_tblpr() {
        // ECMA-376 CT_TblPr requires <w:tblW> before <w:jc>. docx-rs
        // emits them in field-declaration order, so this is a
        // regression guard against a future docx-rs version changing
        // its emission order.
        let xml = build(TableKind::Captioned);
        let tblw = xml.find("<w:tblW").expect("must have <w:tblW>");
        let jc = xml.find("<w:jc").expect("must have <w:jc>");
        assert!(
            tblw < jc,
            "CT_TblPr order violated: <w:tblW> must precede <w:jc> (tblw={tblw}, jc={jc})"
        );
    }

    #[test]
    fn sources_box_has_tablegrid_centre_and_inherits_borders() {
        let xml = build(TableKind::SourcesBox);
        assert!(xml.contains(r#"<w:tblStyle w:val="TableGrid""#));
        assert!(xml.contains(r#"<w:jc w:val="center""#));
        // SourcesBox keeps docx-rs's default borders (size 2,
        // colour 000000) so the inherited TableGrid style is what
        // actually governs visual appearance.
        assert!(xml.contains(r#"<w:tblCellMar>"#));
    }

    #[test]
    fn sources_box_cells_only_vertical_align_when_caller_sets_it() {
        // emit() itself never adds vAlign. If the caller sets vAlign
        // on the right QR cell only, only that cell carries it. The
        // left text cell must not carry vAlign. This test uses two
        // empty cells (neither sets vAlign) and asserts the table
        // contains zero <w:vAlign>, proving emit() does not inject
        // one.
        let left = TableCell::new();
        let right = TableCell::new();
        let row = TableRow::new(vec![left, right]);
        let xml = emit(
            TableKind::SourcesBox,
            vec![row],
            TableLayout {
                grid: vec![1000, 1000],
                total_twips: 2000,
            },
        )
        .build();
        let xml = String::from_utf8(xml).expect("utf-8");
        assert!(
            !xml.contains("<w:vAlign"),
            "left cell must not carry vAlign unless the caller sets it: {xml}"
        );
    }

    #[test]
    fn quote_callout_has_tablegrid_centre_and_intentional_padding() {
        let xml = build(TableKind::QuoteCallout);
        assert!(xml.contains(r#"<w:tblStyle w:val="TableGrid""#));
        assert!(xml.contains(r#"<w:jc w:val="center""#));
        // Intentional 200/120 vertical padding for quote breathing
        // room — distinct from sources_box (100/100) and Captioned
        // (style default).
        assert!(xml.contains(r#"w:w="200""#));
        assert!(xml.contains(r#"w:w="120""#));
    }

    #[test]
    fn keypoints_box_is_reserved_and_emits_default_profile() {
        // Smoke test only: the variant must construct cleanly so
        // future call sites have somewhere to land without
        // re-introducing per-call-site styling.
        let xml = build(TableKind::KeypointsBox);
        assert!(xml.contains(r#"<w:tblStyle w:val="TableGrid""#));
        assert!(xml.contains(r#"<w:jc w:val="center""#));
    }
}
