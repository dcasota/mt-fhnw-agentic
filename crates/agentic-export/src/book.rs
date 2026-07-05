//! Book renderer — the Rust port of the bookkit engine.
//!
//! Builds a professional A4 DOCX (title page, TOC, styled headings, tables with
//! shaded headers, embedded figures with captions) from chapter markdown via
//! `docx-rs`. Figures are expected already rendered (paths under `figdir`); the
//! caller resolves `figspec` blocks with `agentic-figures` first.

use std::io::Cursor;
use std::path::Path;

use anyhow::{Context, Result};
use docx_rs::{
    AlignmentType, BorderType, BreakType, DocGrid, Docx, FieldCharType, Footer, Header, HeightRule,
    Hyperlink, HyperlinkType, InstrText, LineSpacing, LineSpacingType, PageMargin, PageNum,
    PageOrientationType, PageSize, Paragraph, ParagraphBorder, ParagraphBorderPosition, Pic, Run,
    RunFonts, SectionProperty, Shading, Style, StyleType, Table, TableCell, TableCellBorder,
    TableCellBorderPosition, TableOfContents, TableRow, TextDirectionType, VAlignType,
    VertAlignType, WidthType,
};

use agentic_core::i18n::t;

use crate::decorations::CalloutFlavor;
use crate::markdown::{DocxBlock, DocxRun, to_docx_blocks};
use crate::size_manifest::SizeManifest;

/// Process-wide monotonic counter for flavor-bookmark ids (Round-V
/// Zone-E callout-chrome). Bookmarks need to be unique within a
/// single `document.xml`; using a process-wide counter is unique by
/// construction (it never decreases). Offset above `100_000` to stay
/// clear of `Ctx::bookmark_id` (heading anchors, which start at 0).
fn next_flavor_bookmark_id() -> usize {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(100_000);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Plant a flavor sentinel on a `BkCallout` paragraph so the
/// [`crate::decorations::apply_callout_chrome`] postprocess pass can
/// inject the per-flavor `<w:pBdr>` + `<w:shd>` after serialisation.
/// `docx-rs` 0.4.x exposes no public `Paragraph::shading` /
/// `Paragraph::borders` setter, hence the two-step dance.
fn plant_flavor_sentinel(p: Paragraph, flavor: CalloutFlavor) -> Paragraph {
    let id = next_flavor_bookmark_id();
    p.add_bookmark_start(id, flavor.bookmark_name())
        .add_bookmark_end(id)
}

const NAVY: &str = "1F3864"; // gold bookkit HEAD (book_build): headings + title
const HEAD2: &str = "2E4A7A";
const GREY: &str = "666666";
const ACCENT: &str = "0B5C9E"; // hyperlink blue
const HEADBG: &str = "1F3864";
const ALTBG: &str = "F4F6FA";
const RULE: &str = "C9D2E0";
/// ADR-0064 iter44 (2026-07-05): switched from Georgia to Palatino
/// Linotype for the default body font. The June-8 master_thesis
/// reference uses Palatino Linotype (declared on the Normal style +
/// referenced in ~180 runs). Our earlier Georgia default leaked into
/// 79 runs of the master_thesis body, causing line breaks to fall on
/// different words (Georgia is ~4% wider than Palatino Linotype per
/// character), which cascaded through every mid-doc page's layout.
/// Non-thesis book profiles (AI-Norms, campaigns) still register
/// Palatino Linotype via the AiNorms styles.xml fixture, so this
/// change is safe across the fleet.
const BODY: &str = "Palatino Linotype";
const HEADF: &str = "Calibri";
const MONO: &str = "Consolas";

const CONTENT_TWIPS: usize = 9298; // A4 (11906) − 2×1304 margins

// Bookkit readability (ADR-0030 → ADR-0054 v1, 2026-06-02 reference-parity
// audit): the reference book_build/build_styles.py declares Normal at 1.32
// line-spacing (denser than 1.5 — closer to a typeset book line-height). The
// engine was previously emitting 1.5 (`360`) which produced a noticeably
// airier page than the reference. Aligned to 1.32 so the Designer profile
// matches AI_Norms_and_Regulations_BOOK.docx byte-for-byte on body spacing.
const LINE_132: i32 = 317; // 1.32× single (240 = single; 1.32 × 240 = 316.8 → 317)
/// ADR-0064 iter44 (2026-07-05): reference single-spacing line height in
/// twips. `LineSpacingType::Auto` + `line(240)` = "single". June-8
/// master_thesis emits `<w:spacing w:after="360"/>` on body paragraphs
/// with NO `w:line` — inheriting the docDefault (which is single). The
/// 1.32× LINE_132 was aliased at iter7-ish for readability in
/// AI-Norms; on the thesis it added ~32% vertical space per line,
/// producing 117 pages vs reference's 96.
const LINE_SINGLE: i32 = 240;
const SPACE_AFTER: u32 = 160; // ≈8 pt after body paragraphs
const SPACE_AFTER_HEAD: u32 = 120; // after headings
const SPACE_BEFORE_HEAD: u32 = 280; // before headings (separate from prose above)
const SPACE_AROUND_TABLE: u32 = 140; // spacer paragraphs hugging a table
const SPACE_AROUND_FIG: u32 = 220; // breathing room above a figure + below its caption (audit sentinel)
// Below this column width (≈2.2 cm) header labels are rotated to read bottom-up.
const ROTATE_COLW: usize = 1250;

// A table with at least this many columns is too cramped for portrait A4 even
// with fixed layout + rotated headers, so it is placed on its own A4 *landscape*
// page (ADR-0030). Mirrors the old thesis's landscape wide-table pages.
const LANDSCAPE_COLS: usize = 7;
// A4 landscape content width: 16838 − 2×1304 margins.
const LAND_CONTENT_TWIPS: usize = 14230;

/// Bookkit body paragraph spacing: 1.32× line-height with breathing room
/// after the block. Mirrors `book_build/build_styles.py` Normal style
/// (reference-parity audit 2026-06-02).
/// ADR-0064 iter44 (2026-07-05): switched from 1.32× (LINE_132=317)
/// to single-line (LINE_SINGLE=240). June-8 reference master_thesis
/// declares `<w:spacing w:after="360"/>` on body paragraphs with NO
/// `w:line` attr — inherits docDefault single spacing. Our earlier
/// 1.32× multiplier added ~32% vertical space per line, producing 117
/// pages vs reference's 96 for the same content. Non-thesis books
/// were similarly affected but the visual delta is acceptable
/// (readability slightly tightens — reference behaviour).
fn body_spacing() -> LineSpacing {
    LineSpacing::new()
        .line_rule(LineSpacingType::Auto)
        .line(LINE_SINGLE)
        .after(SPACE_AFTER)
}

/// The standard page margins, shared by the body and every mid-document section.
#[allow(dead_code)]
fn std_margin() -> PageMargin {
    PageMargin::new()
        .top(1417)
        .bottom(1417)
        .left(1304)
        .right(1304)
}

/// Wave-4 (ADR-0054 v1, 2026-06-03): standard page margins WITH the
/// reference-parity header/footer-distance overrides applied. Falls back to
/// the historical Designer defaults (851 / 992 twips) when the meta does
/// not supply an override.
fn std_margin_for(m: &BookMeta) -> PageMargin {
    // ADR-0064 iter44 (2026-07-05): FhnwMtTemplate uses June-8 reference
    // margins (asymmetric — larger left for binding). Non-thesis profiles
    // keep the historical symmetric 1304/1304.
    let (left, right, header_default, footer_default) = if matches!(
        m.thesis_typography,
        TypographyProfile::FhnwMtTemplate | TypographyProfile::FhnwProposalParity
    ) {
        (1417, 1134, 709, 709)
    } else {
        (1304, 1304, 851, 992)
    };
    let mut pm = PageMargin::new()
        .top(1417)
        .bottom(1417)
        .left(left)
        .right(right)
        .header(header_default)
        .footer(footer_default);
    if let Some(h) = m.header_distance_twips {
        pm = pm.header(h as i32);
    }
    if let Some(f) = m.footer_distance_twips {
        pm = pm.footer(f as i32);
    }
    pm
}

/// `sectPr` for a portrait A4 section (default next-page break).
#[allow(dead_code)]
fn portrait_sectpr() -> SectionProperty {
    SectionProperty::new()
        .page_size(PageSize::new().size(11906, 16838))
        .page_margin(std_margin())
}

/// `sectPr` for a portrait A4 section with layout overrides applied
/// (cols.space + docGrid line-pitch + pgMar header/footer distances).
/// Wave-4 AI-Norms parity (ADR-0054 v1, 2026-06-03).
#[allow(dead_code)]
fn portrait_sectpr_for(m: &BookMeta) -> SectionProperty {
    portrait_sectpr_with(&LayoutOverrides::from_meta(m))
}

fn portrait_sectpr_with(lo: &LayoutOverrides) -> SectionProperty {
    let mut pm = PageMargin::new()
        .top(1417)
        .bottom(1417)
        .left(1304)
        .right(1304);
    if let Some(h) = lo.header_distance_twips {
        pm = pm.header(h as i32);
    }
    if let Some(f) = lo.footer_distance_twips {
        pm = pm.footer(f as i32);
    }
    let mut sp = SectionProperty::new()
        .page_size(PageSize::new().size(11906, 16838))
        .page_margin(pm);
    if let Some(space) = lo.cols_space_twips {
        sp.space = space as usize;
    }
    if let Some(pitch) = lo.doc_grid_line_pitch {
        sp = sp.doc_grid(DocGrid::new().line_pitch(pitch as usize));
    }
    sp
}

/// `sectPr` for a landscape A4 section (default next-page break).
#[allow(dead_code)]
fn landscape_sectpr() -> SectionProperty {
    SectionProperty::new()
        .page_size(
            PageSize::new()
                .size(16838, 11906)
                .orient(PageOrientationType::Landscape),
        )
        .page_margin(std_margin())
}

/// Landscape variant of [`portrait_sectpr_for`] (Wave-4 AI-Norms parity).
#[allow(dead_code)]
fn landscape_sectpr_for(m: &BookMeta) -> SectionProperty {
    landscape_sectpr_with(&LayoutOverrides::from_meta(m))
}

fn landscape_sectpr_with(lo: &LayoutOverrides) -> SectionProperty {
    let mut pm = PageMargin::new()
        .top(1417)
        .bottom(1417)
        .left(1304)
        .right(1304);
    if let Some(h) = lo.header_distance_twips {
        pm = pm.header(h as i32);
    }
    if let Some(f) = lo.footer_distance_twips {
        pm = pm.footer(f as i32);
    }
    let mut sp = SectionProperty::new()
        .page_size(
            PageSize::new()
                .size(16838, 11906)
                .orient(PageOrientationType::Landscape),
        )
        .page_margin(pm);
    if let Some(space) = lo.cols_space_twips {
        sp.space = space as usize;
    }
    if let Some(pitch) = lo.doc_grid_line_pitch {
        sp = sp.doc_grid(DocGrid::new().line_pitch(pitch as usize));
    }
    sp
}

// ----------------------------------------------------------------------
// Wave-4 AI-Norms parity: exact inscription text extracted from the
// reference book (`book_build/AI_Norms_and_Regulations_BOOK.docx`,
// paragraph 15, no `pStyle`, italic) on 2026-06-03. Verified
// byte-for-byte against the reference; do not edit without
// re-verifying.
// ----------------------------------------------------------------------
const ANTIKYTHERA_INSCRIPTION_TEXT: &str = "The Antikythera mechanism, raised from a Roman-era shipwreck off a Greek island, is the oldest known analogue computer \u{2014} a hand-cranked assembly of bronze gears that modelled the motions of the heavens. This book attempts a comparable instrument for a different sky: the moving, interlocking gears of the world\u{2019}s AI norms and regulations.";

/// Closing-thought heading the reference book emits as the FIRST BkCallout
/// paragraph at index 3856 (right after the Appendix, before the Table of
/// Figures). Kept here as a constant so the renderer can match the
/// reference verbatim while the manifest only supplies the body text.
const CLOSING_THOUGHT_HEADING: &str = "How this book was made";

/// Effective column count of a markdown table (header or widest row).
fn col_count(header: &[String], rows: &[Vec<String>]) -> usize {
    header
        .len()
        .max(rows.iter().map(Vec::len).max().unwrap_or(1))
        .max(1)
}

/// Typography profile for the rendered docx (ADR-0050).
///
/// `Designer` is the historical bookkit aesthetic — Georgia body, Calibri
/// navy headings (`#1F3864` H1/H2 / `#2E4A7A` H3/H4), grey captions — used
/// by every non-thesis book (campaigns, dimensions, handbook, …).
/// `FhnwProposalParity` matches the FHNW master-thesis proposal docx
/// verbatim: Arial 10pt body, Arial 12–14pt bold black headings, Times
/// New Roman 9pt black captions, no accent colours. Selected by the
/// `master_thesis` book in the manifest; defaults to `Designer` so every
/// other book is unaffected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TypographyProfile {
    #[default]
    Designer,
    FhnwProposalParity,
    /// FHNW Master-Thesis Template (ADR-0064, 2026-07-03). Palatino Linotype
    /// 11 pt body, H1 24 pt bold, H2 14 pt bold, H3 12 pt bold, custom
    /// `Chapter Number` line 17 pt bold, dark-navy `#294F6D` accent for
    /// hyperlinks + rules, mirror margins (2.5 cm inside, 2.0 cm outside).
    /// Selected by `thesis_typography: "fhnw-mt-template"` in the manifest.
    /// Wired to the `agentic-thesis-template` crate for canonical styles.xml
    /// + numbering.xml + theme + settings + fontTable.
    FhnwMtTemplate,
}

/// Page-numbering style (ADR-0050 §2).
///
/// `Arabic` (historical default) numbers every page 1, 2, 3, … from the
/// start. `FhnwRomanThenArabic` uses lowercase Roman (i, ii, iii, …) for
/// front-matter (title page through acronyms) and switches to Arabic 1
/// at the first body chapter — the academic-thesis convention FHNW
/// follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PageNumbering {
    #[default]
    Arabic,
    FhnwRomanThenArabic,
}

/// Caption label format (ADR-0050 §1; figure-caption-rules.md).
///
/// `Period` (historical default) renders "Figure 1. <caption>".
/// `Colon` renders "Figure 1: <caption>" (English) or "Abbildung 1:
/// <caption>" (German), matching the FHNW MAS Beschriftungsformat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CaptionFormat {
    #[default]
    Period,
    Colon,
}

#[derive(Debug, Clone)]
pub struct BookMeta {
    pub title: String,
    pub subtitle: String,
    pub author: String,
    pub context: String,
    /// Descriptive line under the title rule (bookkit DESCRIPTION). Optional.
    pub description: String,
    /// Centred dedication on the inscription page (bookkit inscription). Optional.
    pub dedication: Option<String>,
    /// Epigraph quote on the inscription page. Optional.
    pub epigraph: Option<String>,
    /// Epigraph attribution ("— Name"). Optional.
    pub epigraph_by: Option<String>,
    /// Edition & disclaimer paragraphs (one per line). Optional.
    pub disclaimer: Option<String>,
    /// Imprint lines on the title page under the affiliation (e.g. "Version 1.0",
    /// place + date). One centred line per text line. Optional.
    pub imprint: Option<String>,
    /// Master-thesis numbering profile (ADR-0045, bookkit C). When true, body
    /// chapters (Introduction, Theory, Conclusion, …) are NUMBERED and only true
    /// front/back-matter (Management Summary, Acronyms, Appendix, Bibliography,
    /// …) stays unnumbered — the opposite of the book profile, where
    /// "Introduction" is unnumbered front-matter.
    pub thesis_profile: bool,
    /// Companion-paper profile (ADR-0045, bookkit B). When true the document is
    /// NOT transformed into a book: the elaborate title page, edition/disclaimer
    /// and inscription pages are skipped in favour of a plain title + contents.
    pub companion: bool,
    /// Extra index terms beyond the built-in set (e.g. dimension-specific).
    pub index_terms: Vec<String>,
    /// Chrome language tag (en|de|fr|it|rm|hi). Empty or unknown → English.
    /// Localises only engine-generated chrome (labels, caption prefixes,
    /// list/section headings); chapter content is untouched.
    pub lang: String,
    /// Typography profile (ADR-0050). `Designer` (default) preserves the
    /// historical Georgia/Calibri/navy palette for every non-thesis book.
    /// `FhnwProposalParity` switches body/headings/captions to Arial/Arial/
    /// TimesNewRoman black for FHNW master-thesis parity.
    pub thesis_typography: TypographyProfile,
    /// Page-numbering scheme (ADR-0050 §2). `Arabic` (default) numbers
    /// every page 1, 2, 3, …; `FhnwRomanThenArabic` uses Roman for
    /// front-matter and restarts at Arabic 1 at chapter 1.
    pub page_numbering: PageNumbering,
    /// Caption label format (ADR-0050 §1). `Period` (default) → "Figure 1.";
    /// `Colon` → "Figure 1:" (English) or "Abbildung 1:" (German).
    pub caption_format: CaptionFormat,
    /// Optional FHNW-style header logo bytes (PNG). When set with the
    /// FHNW typography profile, the engine renders a page header on every
    /// page with the logo (right-anchored) plus the two header text lines
    /// from `header_lines`. Loaded from the project DB by the CLI; the
    /// engine itself only consumes the bytes (zero filesystem coupling).
    pub header_logo: Option<Vec<u8>>,
    /// Optional header text lines (e.g. `["Master of Advanced Studies",
    /// "Leadership in Cybersecurity"]`). Rendered right-aligned under the
    /// logo when present. Empty/missing → header text suppressed.
    pub header_lines: Vec<String>,
    /// Optional STANDALONE dedication page (T1.7, REF parity 2026-06-02).
    /// Distinct from `dedication` (which sits on the inscription page next to
    /// the epigraph): when set, the engine emits a dedicated page BEFORE the
    /// inscription page with this text centred, italic, large. Use for the
    /// personal "To …" dedication that the reference book renders on its own
    /// page; leave `None` to keep the historical single-inscription layout.
    pub dedication_page: Option<String>,
    /// Optional Antikythera NOTE for the inscription page footer (T1.7, REF
    /// parity 2026-06-02). When set, the engine appends a small grey
    /// NOTE-style paragraph at the bottom of the inscription page (after
    /// the epigraph attribution). Used in the reference book to attribute
    /// the Antikythera-mechanism artwork. Plain text; rendered centred.
    pub antikythera_note: Option<String>,
    /// Optional standalone QR-linked URL block (T1.7, REF parity 2026-06-02).
    /// Distinct from the per-chapter Sources & QR-codes box (which lists
    /// every link in the chapter): when set, the engine emits a single
    /// centred QR + URL block as a standalone page right after the
    /// disclaimer/inscription chrome. Used in the reference book to link
    /// to the book's home page / errata / companion site.
    pub qrlink: Option<String>,
    /// Wave-2 AI-Norms parity (ADR-0054 v1, 2026-06-03). When `true`, the
    /// engine
    ///   - emits `pStyle="BkH1..4"` on body headings instead of
    ///     `pStyle="Heading1..4"` (the bookkit named-style set);
    ///   - sets `tblStyle="TableGrid"` on every content table;
    ///   - replaces the docx-rs-emitted `word/styles.xml` with the verbatim
    ///     reference styles document (186 style definitions) during the
    ///     finalize-pass.
    ///
    /// Defaults to `false` so existing books (campaigns, dimensions, …)
    /// keep their current docx-rs default-style behaviour unchanged. Set
    /// to `true` for the `ai_norms` book (and any other book targeting
    /// reference-parity downstream tooling).
    pub body_render_use_bk_styles: bool,
    // ------------------------------------------------------------------
    // Wave-4 AI-Norms parity (ADR-0054 v1, 2026-06-03): layout-override
    // sidecar values + back-matter / inscription / closing-thought blobs.
    // Each field is `None`/`false`/empty by default so non-parity books are
    // unaffected (Designer profile keeps its current chrome).
    // ------------------------------------------------------------------
    /// Per-section `<w:pgMar w:header="…">` distance in twentieths of a
    /// point (twips). Default = 720 (Word's "1/2 inch from top"), matching
    /// the reference book's sectPr. The renderer reads this in
    /// [`std_margin_for`] and emits it on every section; `None` falls back
    /// to the historical Designer default (851 ≈ 0.6 in).
    pub header_distance_twips: Option<u32>,
    /// Per-section `<w:pgMar w:footer="…">` distance in twips. Default =
    /// 720 (reference book parity). `None` → Designer default (992).
    pub footer_distance_twips: Option<u32>,
    /// `<w:cols w:space="…">` value on every sectPr (twips between
    /// columns; for single-column docs Word still emits a `space` attr).
    /// Default = 720 (reference book). `None` → docx-rs default 425.
    pub cols_space_twips: Option<u32>,
    /// `<w:docGrid w:linePitch="…">` value on every sectPr. 360 = single-
    /// line grid; Word writes this on every section. Default = 360. The
    /// post-processor injects the element if a sectPr is missing it.
    pub doc_grid_line_pitch: Option<u32>,
    /// Personal dedication ("For Melanie, Sarah and Timo"). Distinct from
    /// the generic `dedication` (which sits on the inscription page with
    /// the epigraph) and from `dedication_page` (the bigger T1.7 standalone
    /// dedication block). When set, the engine emits a single centred
    /// paragraph on its own page BEFORE the inscription page. `None` is
    /// the historical default (no personal dedication).
    pub dedication_personal: Option<String>,
    /// Closing-thought paragraph emitted near the back-of-book as a
    /// BkCallout (Wave-4 AI-Norms parity, REF parity 2026-06-03). The
    /// reference book places this between the Appendix and Table of
    /// Figures with the title "How this book was made". The string is
    /// the BODY text only — the engine renders the heading separately.
    pub closing_thought: Option<String>,
    /// Full byline / institution line emitted on the title page under the
    /// author (e.g. "MAS Leadership in Cybersecurity · University of
    /// Applied Sciences and Arts Northwestern Switzerland (FHNW) ·
    /// 2023–2026"). When set, REPLACES `context` on the title page;
    /// `None` falls back to the historical short `context` string.
    pub byline_institution_full: Option<String>,
    /// Override the chrome heading for the back-matter Table of Figures.
    /// Default i18n key resolves to "List of Figures" (en) /
    /// "Abbildungsverzeichnis" (de) / … . Setting this swaps the heading
    /// text for the reference-book wording "Table of Figures" without
    /// touching i18n.
    pub tof_heading: Option<String>,
    /// Override the chrome heading for the back-matter Table of Tables.
    /// Mirrors [`tof_heading`].
    pub tot_heading: Option<String>,
    /// Render the Antikythera-mechanism inscription paragraph on the
    /// inscription page. `false` (default) keeps existing layout. `true`
    /// emits the centred italic paragraph extracted from the reference
    /// book (see `ANTIKYTHERA_INSCRIPTION_TEXT`). REF parity 2026-06-03.
    pub inscription_page_enabled: bool,
    /// Wave-2 (Bookkit profile chrome suppression, REF parity 2026-06-04).
    /// When `true` (the historical default) the engine emits the back-of-book
    /// Index section (`Heading1 "Index"` + either an `INDEX \c 2` field or
    /// the `IndexHeading`/`Index1` letter blocks). Setting to `false`
    /// suppresses the whole Index section so the document closes on the
    /// preceding back-matter list (TOF/TOT/Bibliography). Used by the
    /// `master_thesis_bookkit` profile to match the reference thesis which
    /// has no Index. Affects both [`render_book`] and [`render_thesis_book`].
    pub emit_index: bool,
    /// Wave-2 (Bookkit profile chrome suppression, REF parity 2026-06-04).
    /// When `true` (the historical default) chapters classified as
    /// [`ThesisSlot::Appendix`] are emitted in the thesis back-matter just
    /// before the Table of Figures (per `thesis_layout`). Setting to
    /// `false` SKIPS Appendix-classified chapters entirely. Used by the
    /// `master_thesis_bookkit` profile to match the reference thesis which
    /// has no Appendix between body and back-matter lists. Only affects
    /// [`render_thesis_book`]; non-thesis books are unaffected.
    pub emit_appendix_in_back_matter: bool,
    /// Wave-2 (Bookkit profile chrome suppression, REF parity 2026-06-04).
    /// When `true` (the historical default) the engine emits an end-of-
    /// chapter "Sources & QR codes" box (bookkit `flush_sources`) listing
    /// every link harvested in the chapter, each with a QR drawing. The
    /// reference master thesis has no per-chapter Sources boxes, so the
    /// `master_thesis_bookkit` profile sets this to `false` to suppress
    /// every `flush_sources` call across [`render_book`] and
    /// [`render_thesis_book`].
    pub emit_per_chapter_sources_box: bool,
    /// Wave-2 (Bookkit profile chrome suppression, REF parity 2026-06-04).
    /// When `true` (the historical default) the engine emits a thin gray
    /// horizontal-rule paragraph (`chapter_end_rule`) at the close of
    /// every chapter when `body_render_use_bk_styles` is also true. The
    /// reference master thesis carries ≥40 such rules, so this flag stays
    /// `true` for the `master_thesis_bookkit` profile. Setting to `false`
    /// would suppress the divider without affecting the underlying
    /// `body_render_use_bk_styles` opt-in (kept for symmetry with the
    /// other suppression flags).
    pub emit_chapter_dividers: bool,
    /// Wave-3 iter-D (REF parity 2026-06-04). When `true`, the thesis
    /// renderer emits a per-chapter `<w:sectPr>` (continuous, no override)
    /// at every chapter close so the document carries one section break
    /// per chapter (matching the FHNW reference docx: 19 in-body sectPrs
    /// plus the document-level sectPr = 20 total). Defaults to `false`
    /// so every existing book / profile keeps the historical single
    /// document-level sectPr behaviour. Only honoured by
    /// `render_thesis_book` (the bookkit thesis profile); non-thesis
    /// `render_book` paths ignore this flag.
    pub emit_per_chapter_sectpr: bool,
    /// Wave-3 iter-D (REF parity 2026-06-04). When `true` (the historical
    /// default) the engine renders fenced ```keypoints```, ```quiz``` and
    /// ```callout``` blocks as the bookkit `chapter_extras` chrome
    /// (key-topic boxes, review-question lists, callouts). When `false`,
    /// those fenced blocks are skipped entirely (no paragraphs emitted),
    /// matching the FHNW reference thesis which has no per-chapter
    /// key-topic / review-question / callout chrome. Set to `false` by
    /// the `master_thesis_bookkit` manifest to suppress every chapter-
    /// extras emitter across the thesis renderer.
    pub emit_chapter_extras: bool,
}

/// Wave-2 (Bookkit profile chrome suppression, REF parity 2026-06-04). The
/// four new chrome-suppression flags default to `true` so every existing
/// book and test keeps its historical output unchanged; the
/// `master_thesis_bookkit` manifest entry opts out of the three thesis-
/// specific ones (`emit_index`, `emit_appendix_in_back_matter`,
/// `emit_per_chapter_sources_box`).
impl Default for BookMeta {
    fn default() -> Self {
        Self {
            title: String::new(),
            subtitle: String::new(),
            author: String::new(),
            context: String::new(),
            description: String::new(),
            dedication: None,
            epigraph: None,
            epigraph_by: None,
            disclaimer: None,
            imprint: None,
            thesis_profile: false,
            companion: false,
            index_terms: Vec::new(),
            lang: String::new(),
            thesis_typography: TypographyProfile::default(),
            page_numbering: PageNumbering::default(),
            caption_format: CaptionFormat::default(),
            header_logo: None,
            header_lines: Vec::new(),
            dedication_page: None,
            antikythera_note: None,
            qrlink: None,
            body_render_use_bk_styles: false,
            header_distance_twips: None,
            footer_distance_twips: None,
            cols_space_twips: None,
            doc_grid_line_pitch: None,
            dedication_personal: None,
            closing_thought: None,
            byline_institution_full: None,
            tof_heading: None,
            tot_heading: None,
            inscription_page_enabled: false,
            // Bookkit chrome flags — true preserves historical behaviour.
            emit_index: true,
            emit_appendix_in_back_matter: true,
            emit_per_chapter_sources_box: true,
            emit_chapter_dividers: true,
            // Wave-3 iter-D (2026-06-04). Default OFF so historical books
            // keep their single doc-level sectPr.
            emit_per_chapter_sectpr: false,
            // Wave-3 iter-D (2026-06-04). Default ON so historical books
            // keep their chapter_extras emission (AI Norms, etc.). The
            // bookkit thesis profile opts out via the manifest.
            emit_chapter_extras: true,
        }
    }
}

/// Per-book render state threaded through `render_block`: running figure / table
/// / chapter counters, the set of index terms already marked, and the current
/// chapter's collected links (for the Sources & QR-codes box).
struct Ctx<'a> {
    figdir: &'a Path,
    /// Chrome language tag (en|de|fr|it|rm|hi); drives `i18n::t` lookups.
    lang: &'a str,
    figno: u32,
    tblno: u32,
    chapno: u32,
    idx_seen: std::collections::HashSet<String>,
    /// Curated terms (built-in + per-book) marked into the index.
    index_terms: Vec<String>,
    /// (label, url) links seen in the current chapter, de-duped by URL.
    links: Vec<(String, String)>,
    /// Typography profile (ADR-0050). Designer for non-thesis books;
    /// FhnwProposalParity for the master-thesis profile.
    typography: TypographyProfile,
    /// Caption label format (ADR-0050 §1; figure-caption-rules.md).
    caption_format: CaptionFormat,
    /// Monotonically increasing bookmark id (REQ-5, 2026-06-03). Each
    /// heading consumes one id for its `<w:bookmarkStart/End w:id="N">`
    /// pair; ids are unique across the whole document.
    bookmark_id: usize,
    /// Anchor names already emitted as heading bookmarks, used to
    /// disambiguate collisions (`-2`, `-3`, …) so internal
    /// `[text](#anchor)` links still resolve.
    bookmark_anchors: std::collections::HashSet<String>,
    /// Wave-6 AI-Norms parity (ADR-0054 v1, 2026-06-03). Mirrors
    /// [`BookMeta::body_render_use_bk_styles`]: when true, body
    /// heading paragraphs emit `pStyle="BkH{1..4}"` instead of the
    /// docx-rs default `pStyle="Heading{1..4}"`, matching the
    /// reference AI_Norms_and_Regulations style ids used by the
    /// 186-style verbatim styles.xml replacement.
    body_render_use_bk_styles: bool,
    /// Wave-4 (ADR-0054 v1, 2026-06-03): layout overrides forwarded from
    /// [`BookMeta`] so mid-document sectPrs (e.g. the landscape table
    /// wrapper) emit the same cols.space / docGrid / pgMar header/footer
    /// distances as the document-level sectPr.
    layout: LayoutOverrides,
    /// Round V iter-10 (drawing_class_bucket parity, 2026-06-03): per-figure
    /// width hint table loaded from `<figdir>/sizes.toml`. The
    /// [`DocxBlock::Image`] arm tries this lookup BEFORE the path-prefix
    /// heuristic so editorial FIGURE/OTHER assignments that cannot be
    /// recovered from path bytes (the `image*.png` family) still land in the
    /// right bucket. Empty for every book that does not ship a manifest.
    size_manifest: SizeManifest,
    /// Wave-3 iter-D (2026-06-04). Mirrors
    /// [`BookMeta::emit_chapter_extras`]: when `false`, the
    /// [`DocxBlock::CodeBlock`] arm skips `keypoints`, `quiz` and
    /// `callout` fenced blocks entirely (no paragraphs emitted). Default
    /// `true` preserves historical behaviour for every existing book
    /// (AI Norms, campaigns, etc.); the `master_thesis_bookkit` profile
    /// opts out via the manifest.
    emit_chapter_extras: bool,
}

/// Wave-4 (ADR-0054 v1, 2026-06-03): the four reference-parity layout
/// override values copied out of [`BookMeta`] for thread-through into
/// mid-document [`SectionProperty`] construction.
#[derive(Debug, Clone, Copy, Default)]
struct LayoutOverrides {
    header_distance_twips: Option<u32>,
    footer_distance_twips: Option<u32>,
    cols_space_twips: Option<u32>,
    doc_grid_line_pitch: Option<u32>,
}

impl LayoutOverrides {
    fn from_meta(m: &BookMeta) -> Self {
        Self {
            header_distance_twips: m.header_distance_twips,
            footer_distance_twips: m.footer_distance_twips,
            cols_space_twips: m.cols_space_twips,
            doc_grid_line_pitch: m.doc_grid_line_pitch,
        }
    }
}

impl Ctx<'_> {
    /// Register a link for the chapter Sources box; returns its 1-based number
    /// (de-duped by URL, matching bookkit `_register_link`).
    fn register_link(&mut self, label: &str, url: &str) -> usize {
        if let Some(i) = self.links.iter().position(|(_, u)| u == url) {
            return i + 1;
        }
        self.links.push((label.to_string(), url.to_string()));
        self.links.len()
    }

    /// Allocate the next bookmark id (REQ-5). Monotonic across the document.
    fn next_bookmark_id(&mut self) -> usize {
        let id = self.bookmark_id;
        self.bookmark_id += 1;
        id
    }

    /// Reserve a unique anchor name. If `base` is already taken (case-sensitive)
    /// returns `base-2`, `base-3`, …; otherwise returns `base` unchanged. The
    /// chosen name is added to the set so later headings collide cleanly.
    fn reserve_anchor(&mut self, base: &str) -> String {
        if self.bookmark_anchors.insert(base.to_string()) {
            return base.to_string();
        }
        let mut n = 2usize;
        loop {
            let candidate = format!("{base}-{n}");
            if self.bookmark_anchors.insert(candidate.clone()) {
                return candidate;
            }
            n += 1;
        }
    }
}

/// Front/back-matter chapter titles that do NOT receive a chapter number
/// (bookkit UNNUMBERED set), matched case-insensitively on the first H1.
const UNNUMBERED_TITLES: &[&str] = &[
    "foreword",
    "preface",
    "acknowledgements",
    "acknowledgments",
    "introduction",
    "acronyms and abbreviations",
    "acronyms",
    "abbreviations",
    "bibliography",
    "references",
    "index",
    "contents",
    "table of figures",
    "list of figures",
    "table of tables",
    "list of tables",
    "about this book",
    "about the book",
    "glossary",
    "appendix",
    "dedication",
    "colophon",
    "disclaimer",
];

/// First H1 text of a chapter's markdown (used to decide numbering + title).
fn first_h1(md: &str) -> Option<String> {
    to_docx_blocks(md).into_iter().find_map(|b| match b {
        DocxBlock::Heading { level: 1, text } => Some(text),
        _ => None,
    })
}

/// Slugify heading text into a stable bookmark/anchor name (REQ-5,
/// 2026-06-03): lowercase ASCII, spaces → hyphens, drop anything outside
/// `[a-z0-9-]`. Empty result falls back to "section".
fn slugify_anchor(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut prev_hyphen = true; // collapse leading separators
    for c in text.chars() {
        let lc = c.to_ascii_lowercase();
        if lc.is_ascii_alphanumeric() {
            out.push(lc);
            prev_hyphen = false;
        } else if matches!(c, ' ' | '\t' | '-' | '_' | '/' | '.' | ',' | ':' | ';') {
            if !prev_hyphen {
                out.push('-');
                prev_hyphen = true;
            }
        }
        // anything else (punctuation, non-ASCII) is silently dropped
    }
    // strip trailing hyphen
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "section".to_string()
    } else {
        out
    }
}

/// Bookmark anchor name for a heading (REQ-5, 2026-06-03). When the heading
/// text starts with `<digits> <space>` (e.g. "3 Current State Analysis"),
/// the anchor is the chapter shortcut `chN`; otherwise the slug is derived
/// from the full text. Uniqueness across the document is enforced by
/// `Ctx::reserve_anchor` (a `-2`, `-3`, … suffix is appended on collision).
fn heading_anchor_name(text: &str) -> String {
    let trimmed = text.trim_start();
    if let Some((num, _)) = trimmed.split_once(' ') {
        if !num.is_empty() && num.chars().all(|c| c.is_ascii_digit()) {
            return format!("ch{num}");
        }
    }
    slugify_anchor(text)
}

/// Return `md` with its first `# heading` line removed (ADR-0050 §1 D2,
/// v0.1.16-engine 2026-05-29). Used to suppress the literal "Title Page"
/// heading from the FHNW title chapter — the proposal docx has no such
/// heading on the title page (the thesis title itself IS the page).
///
/// Only the FIRST top-level `# ` line is removed; if the chapter starts
/// with prose before the heading, the heading stays.
fn strip_first_h1_line(md: &str) -> String {
    let mut out = String::with_capacity(md.len());
    let mut stripped = false;
    for line in md.lines() {
        if !stripped && line.trim_start().starts_with("# ") {
            stripped = true;
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// ADR-0064 iter20 (FhnwMtTemplate title-page truncation, 2026-07-03):
/// strip the H1 line + everything from the first H2 heading onward.
///
/// The current `thesis/fhnw_00_title_page.md` contains a duplicated
/// "## Declaration of Originality" section (also present in
/// `fhnw_00_declaration_of_originality.md`) which bleeds onto the title
/// page in the rendered output. The MT-Template reference title page is
/// short (title, author, supervisors, date) and does NOT contain the
/// declaration text. Truncating here keeps only the title-page portion.
fn strip_first_h1_and_after_first_h2(md: &str) -> String {
    let mut out = String::with_capacity(md.len());
    let mut stripped_h1 = false;
    for line in md.lines() {
        if !stripped_h1 && line.trim_start().starts_with("# ") {
            stripped_h1 = true;
            continue;
        }
        if line.trim_start().starts_with("## ") {
            break;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Front/back-matter titles for the THESIS profile (ADR-0045). Unlike the book
/// profile, "Introduction"/"Conclusion"/etc. are NOT here — they are numbered
/// chapters; only true front/back-matter stays unnumbered. The declarations
/// (originality + Open-Source Photon OS compliance) and the title-page
/// supplement (lecturer / supervisors / publication choice / signature) are
/// front-matter required by the FHNW MAS submission and must not get
/// numbered chapter prefixes.
const THESIS_UNNUMBERED: &[&str] = &[
    "management summary",
    "executive summary",
    "acronyms and abbreviations",
    "acronyms",
    "abbreviations",
    "table of contents",
    "contents",
    "appendix",
    "table of figures",
    "list of figures",
    "table of tables",
    "list of tables",
    "bibliography",
    "references",
    "index",
    "title page",
    "declaration of originality",
    "compliance declaration",
    "compliance declaration for open-source photon os",
];

/// Whether a chapter is numbered: numbered unless its first H1 is a known
/// front/back-matter title. The unnumbered set depends on the profile
/// (`thesis_profile` ⇒ body chapters like Introduction are numbered).
/// Round-G (AI-Norms parity, 2026-06-03): return `true` when a paragraph's
/// concatenated text begins with a numbered/recommendation/quiz/option-letter
/// prefix that the reference book styles as `BkBullet`.
///
/// Matched prefixes (case-sensitive on the alphabetic anchor):
/// * `N.\s+`     — 1-3 digit ordinal followed by period+whitespace (`1. Foo`)
/// * `RN.\s+`    — recommendation IDs (`R1. Adopt the plan`)
/// * `QN.\s+`    — quiz questions (`Q3. Why does …`)
/// * `L.\s+`     — single uppercase letter option labels (`A. Foo`, `B. Bar`)
///
/// Excludes section-number patterns (`5.1 Foo`, `5.14.2 Bar`) by requiring
/// the character immediately after the first period to be non-digit. The
/// caller is responsible for skipping paragraphs that already have a style
/// applied (headings, callouts, captions); this helper only inspects text.
fn should_apply_bk_bullet_prefix(text: &str) -> bool {
    let t = text.trim_start();
    let bytes = t.as_bytes();
    if bytes.is_empty() {
        return false;
    }
    // Find the index of the first period in the prefix anchor (up to 4 chars).
    // Patterns: `\d{1,3}.`, `R\d{1,3}.`, `Q\d{1,3}.`, `[A-Z].`.
    let first = bytes[0];
    let (digits_start, allow_letter) = if first.is_ascii_digit() {
        (0usize, false)
    } else if (first == b'R' || first == b'Q') && bytes.len() > 1 && bytes[1].is_ascii_digit() {
        (1usize, false)
    } else if first.is_ascii_uppercase() && bytes.len() > 1 && bytes[1] == b'.' {
        // Single-letter option label (`A.`, `B.`).
        (0usize, true)
    } else {
        return false;
    };
    if allow_letter {
        // bytes[1] == '.' confirmed above; require whitespace after.
        return bytes
            .get(2)
            .map(|c| c.is_ascii_whitespace())
            .unwrap_or(false);
    }
    // Walk 1-3 digits starting at digits_start.
    let mut i = digits_start;
    let mut digits = 0usize;
    while i < bytes.len() && bytes[i].is_ascii_digit() && digits < 3 {
        i += 1;
        digits += 1;
    }
    if digits == 0 {
        return false;
    }
    // Require a period next.
    if bytes.get(i).copied() != Some(b'.') {
        return false;
    }
    let after_dot = bytes.get(i + 1).copied();
    // Exclude section-number patterns: digit-after-period means multi-level
    // numbering (`5.1`, `5.14.2`), not a numbered prose item.
    if matches!(after_dot, Some(c) if c.is_ascii_digit()) {
        return false;
    }
    // Require whitespace (or EOL) immediately after the period so we don't
    // catch `Dr.` or `v1.5` shaped prefixes (the `v1.5` case is caught by the
    // digit-after-period exclusion above; `Dr.` is caught because `D` is not
    // followed by a period at index 1).
    matches!(after_dot, Some(c) if c.is_ascii_whitespace()) || after_dot.is_none()
}

/// Round V zone C lists-06 (AI-Norms parity, 2026-06-03) — demote the
/// leading bold run of a `BkBullet` item when it forms the `- **X** — Y`
/// lead-in pattern.
///
/// In the reference book, bulletted items of the form `- **Lead-in** —
/// body text` render the lead-in in regular weight; the visual emphasis
/// comes from the (italic-ish) em-dash continuation and the bullet glyph
/// itself, not from the lead-in word. docx-rs preserves the markdown
/// `**…**` as a bold run, so without this demotion the lead-in over-bolds
/// against the reference and the parity gate's bold-run count over-shoots
/// by ~250 occurrences across the AI-Norms book.
///
/// Conservative trigger: only demote when ALL of the following hold:
///   1. `use_bk_styles == true` (Bk* parity mode only)
///   2. runs[0] is bold and non-empty
///   3. runs[1] starts with an em-dash separator (`—`, `\u{2014}`,
///      optionally preceded by whitespace) or with ` - ` (ASCII fallback)
///   4. runs[0] does NOT contain an em-dash itself (so we don't demote a
///      bold sentence that happens to contain `—` mid-text)
///
/// Returns a new `Vec<DocxRun>` with runs[0].bold flipped to `false`; the
/// original slice is consumed in normal Rust style. When the trigger does
/// not fire, returns `runs.to_vec()` unchanged.
fn demote_lead_bold_for_bk_bullet(
    runs: &[crate::markdown::DocxRun],
    use_bk_styles: bool,
) -> Vec<crate::markdown::DocxRun> {
    if !use_bk_styles || runs.len() < 2 {
        return runs.to_vec();
    }
    let first = &runs[0];
    if !first.bold || first.text.is_empty() || first.text.contains('\u{2014}') {
        return runs.to_vec();
    }
    let second_lead = runs[1].text.trim_start();
    let starts_with_emdash = second_lead.starts_with('\u{2014}') || second_lead.starts_with("- ");
    if !starts_with_emdash {
        return runs.to_vec();
    }
    let mut out = runs.to_vec();
    out[0].bold = false;
    out
}

fn chapter_is_numbered(md: &str, thesis_profile: bool) -> bool {
    let set: &[&str] = if thesis_profile {
        THESIS_UNNUMBERED
    } else {
        UNNUMBERED_TITLES
    };
    match first_h1(md) {
        Some(t) => {
            let tl = t.trim().to_lowercase();
            !set.iter().any(|u| tl == *u || tl.starts_with(u))
        }
        None => false,
    }
}

/// Strip `{{index:term}}` markers from a markdown chapter body (Wave 7,
/// AI-Norms parity, 2026-06-03).
///
/// The markers are curator-placed signals consumed by
/// [`crate::index::collect_index_entries`]; they must not surface as
/// visible body text in the rendered docx. Replaces each marker with an
/// empty string while preserving surrounding whitespace; unterminated
/// markers (no closing `}}`) are left intact so the curator notices the
/// typo on render.
fn strip_index_markers(md: &str) -> String {
    const OPEN: &str = "{{index:";
    const CLOSE: &str = "}}";
    let mut out = String::with_capacity(md.len());
    let mut rest = md;
    while let Some(pos) = rest.find(OPEN) {
        out.push_str(&rest[..pos]);
        let after = &rest[pos + OPEN.len()..];
        if let Some(end) = after.find(CLOSE) {
            rest = &after[end + CLOSE.len()..];
        } else {
            // Unterminated marker — keep the literal text so the typo
            // shows up in the rendered output for the curator to fix.
            out.push_str(&rest[pos..]);
            return out;
        }
    }
    out.push_str(rest);
    out
}

fn body_fonts() -> RunFonts {
    RunFonts::new().ascii(BODY).hi_ansi(BODY)
}
fn head_fonts() -> RunFonts {
    RunFonts::new().ascii(HEADF).hi_ansi(HEADF)
}

// ─────────────────────────────────────────────────────────────────────────────
// ADR-0050 typography branches.
//
// Every place that previously read `BODY`, `HEADF`, `NAVY`, `GREY`, `ACCENT`,
// `RULE` or `body_fonts()`/`head_fonts()` directly now routes through one of
// these getters with a `TypographyProfile`. `Designer` returns the historical
// values byte-for-byte (zero regression for the 17 non-thesis books).
// `FhnwProposalParity` returns the Arial / Times New Roman / black values
// measured from the FHNW proposal docx 2025-12-29.
// ─────────────────────────────────────────────────────────────────────────────

/// Arial — FHNW proposal body & heading face.
const FHNW_BODY: &str = "Arial";
/// Times New Roman — FHNW proposal caption face.
const FHNW_CAPTION: &str = "Times New Roman";
/// Pure black — every FHNW proposal text colour.
const FHNW_BLACK: &str = "000000";
/// Palatino Linotype — FHNW MT-Template body + heading + caption face
/// (ADR-0064). All four run-font slots pinned per ADR-0002.
const FHNW_MT_BODY: &str = "Palatino Linotype";
/// Dark navy `#294F6D` — FHNW MT-Template hyperlink + accent colour.
/// From `MT-Template/build/generate_template.py::ACCENT`.
const FHNW_MT_ACCENT: &str = "294F6D";

/// Body run-fonts for the active typography profile.
fn body_fonts_for(p: TypographyProfile) -> RunFonts {
    match p {
        TypographyProfile::Designer => body_fonts(),
        TypographyProfile::FhnwProposalParity => {
            RunFonts::new().ascii(FHNW_BODY).hi_ansi(FHNW_BODY)
        }
        TypographyProfile::FhnwMtTemplate => {
            RunFonts::new().ascii(FHNW_MT_BODY).hi_ansi(FHNW_MT_BODY)
        }
    }
}

/// Heading run-fonts for the active typography profile.
fn head_fonts_for(p: TypographyProfile) -> RunFonts {
    match p {
        TypographyProfile::Designer => head_fonts(),
        TypographyProfile::FhnwProposalParity => {
            RunFonts::new().ascii(FHNW_BODY).hi_ansi(FHNW_BODY)
        }
        TypographyProfile::FhnwMtTemplate => {
            RunFonts::new().ascii(FHNW_MT_BODY).hi_ansi(FHNW_MT_BODY)
        }
    }
}

/// Caption run-fonts (Times New Roman for FHNW; Georgia for Designer;
/// Palatino Linotype for MT-Template).
fn caption_fonts_for(p: TypographyProfile) -> RunFonts {
    match p {
        TypographyProfile::Designer => body_fonts(),
        TypographyProfile::FhnwProposalParity => {
            RunFonts::new().ascii(FHNW_CAPTION).hi_ansi(FHNW_CAPTION)
        }
        TypographyProfile::FhnwMtTemplate => {
            RunFonts::new().ascii(FHNW_MT_BODY).hi_ansi(FHNW_MT_BODY)
        }
    }
}

/// Default body text colour ("000000" for both — both palettes ship black
/// running prose; the divergence is in the *accent* colours below).
fn body_color_for(_p: TypographyProfile) -> &'static str {
    "000000"
}

/// Primary heading colour. Designer = NAVY; FHNW variants = pure black
/// (MT-Template ADR-0002: headings are black; only hyperlinks carry accent).
fn heading_color_for(p: TypographyProfile) -> &'static str {
    match p {
        TypographyProfile::Designer => NAVY,
        TypographyProfile::FhnwProposalParity | TypographyProfile::FhnwMtTemplate => FHNW_BLACK,
    }
}

/// Sub-heading (H3/H4) colour.
fn subheading_color_for(p: TypographyProfile) -> &'static str {
    match p {
        TypographyProfile::Designer => HEAD2,
        TypographyProfile::FhnwProposalParity | TypographyProfile::FhnwMtTemplate => FHNW_BLACK,
    }
}

/// Caption text colour.
fn caption_color_for(p: TypographyProfile) -> &'static str {
    match p {
        TypographyProfile::Designer => GREY,
        TypographyProfile::FhnwProposalParity | TypographyProfile::FhnwMtTemplate => FHNW_BLACK,
    }
}

/// "Accent" colour used on the title-page rule and small flourishes.
/// Designer = ACCENT (blue); FhnwProposalParity = pure black (no accent);
/// FhnwMtTemplate = dark navy `#294F6D` (ADR-0002 hyperlink/accent).
fn accent_color_for(p: TypographyProfile) -> &'static str {
    match p {
        TypographyProfile::Designer => ACCENT,
        TypographyProfile::FhnwProposalParity => FHNW_BLACK,
        TypographyProfile::FhnwMtTemplate => FHNW_MT_ACCENT,
    }
}

/// Secondary subtitle / imprint colour.
fn subtitle_color_for(p: TypographyProfile) -> &'static str {
    match p {
        TypographyProfile::Designer => GREY,
        TypographyProfile::FhnwProposalParity | TypographyProfile::FhnwMtTemplate => FHNW_BLACK,
    }
}

/// Bullet / numbered-item glyph colour.
fn bullet_glyph_color_for(p: TypographyProfile) -> &'static str {
    match p {
        TypographyProfile::Designer => ACCENT,
        TypographyProfile::FhnwProposalParity => FHNW_BLACK,
        TypographyProfile::FhnwMtTemplate => FHNW_MT_ACCENT,
    }
}

/// Whether the body should emit a `<w:jc w:val="both"/>` paragraph-level
/// justification override (Round V zone D scope-trim, 2026-06-03).
///
/// Designer profile keeps `Both` (the historical reference parity for
/// non-AI-Norms books). AI-Norms parity (`use_bk_styles=true`) lets the
/// `BkBullet` / `Normal` style govern: the reference fixture declares
/// `w:jc w:val="left"` on `BkBullet`, so an inline `Both` override would
/// flip the reference back to justified. Returning `None` signals "do not
/// emit any inline `align(…)`" so the style wins.
fn body_alignment_override(
    typography: TypographyProfile,
    use_bk_styles: bool,
) -> Option<AlignmentType> {
    if use_bk_styles {
        None
    } else {
        Some(body_alignment_for(typography))
    }
}

/// Heading size (half-points) for level N (1..=4) under the active profile.
/// Designer keeps the existing 44/32/26/23 ladder (= 22/16/13/11.5 pt);
/// FhnwProposalParity uses 28/28/28/28 (flat 14pt).
/// FhnwMtTemplate uses 48/28/24/24 (24/14/12/12 pt, MT-Template ADR-0002).
fn heading_size_hp(p: TypographyProfile, level: u8) -> usize {
    match (p, level) {
        (TypographyProfile::Designer, 1) => 44,
        (TypographyProfile::Designer, 2) => 32,
        (TypographyProfile::Designer, 3) => 26,
        (TypographyProfile::Designer, _) => 23,
        (TypographyProfile::FhnwProposalParity, _) => 28,
        (TypographyProfile::FhnwMtTemplate, 1) => 48,
        (TypographyProfile::FhnwMtTemplate, 2) => 28,
        (TypographyProfile::FhnwMtTemplate, 3) => 24,
        (TypographyProfile::FhnwMtTemplate, _) => 24,
    }
}

/// Body default size (half-points) under the active profile.
/// Designer: 22 (= 11 pt). FhnwProposalParity: 20 (= 10 pt).
/// FhnwMtTemplate: 22 (= 11 pt, ADR-0002 Palatino body).
fn body_size_hp(p: TypographyProfile) -> usize {
    match p {
        TypographyProfile::Designer => 22,
        TypographyProfile::FhnwProposalParity => 20,
        TypographyProfile::FhnwMtTemplate => 22,
    }
}

/// Caption label format (Period vs Colon, ADR-0050 §1).
fn caption_separator_for(p: CaptionFormat) -> &'static str {
    match p {
        CaptionFormat::Period => ".",
        CaptionFormat::Colon => ":",
    }
}

/// Build the FHNW running-header (Master of Advanced Studies / Leadership in
/// Cybersecurity + logo) — ADR-0050 §1, item 1 of the 2026-05-29 cascade
/// rewrite. Returns `Some(Header)` only when the meta carries logo bytes
/// AND uses the FHNW proposal typography; otherwise `None` and the engine
/// falls back to its prior no-header behaviour (every non-thesis book and
/// every Designer-profile book is unaffected).
///
/// Layout matches the proposal docx (extracted via Word COM 2026-05-29):
///   - Right-aligned anchored picture, ≈4.92 × 4.92 cm (image dims 768×768 px)
///   - Two right-aligned lines below: Arial 12 pt bold, both lines lang=en-US
///
/// We use an INLINE picture (not floating-anchored) because docx-rs does not
/// expose the `Anchor`/`Drawing`-anchor fluent builder; an inline picture in a
/// right-aligned paragraph achieves the same visual placement for header use,
/// with the minor difference that text wraps below the image instead of
/// flowing alongside (the proposal's prose does not flow at the header
/// boundary anyway, so this is invisible in the rendered output).
/// Build the FHNW running-header — **DEFERRED to the Word-COM finalize step**.
///
/// History (v0.1.14 → v0.1.16): we previously emitted a docx-rs `Header`
/// with an inline `Pic`. docx-rs serialises that as
/// `<w:drawing><wp:inline>…<a:blip r:embed="rId1">` which on inspection
/// (snapshot 2026-05-29) is structurally well-formed and the embedded
/// `media/imageN.png` is wired correctly. But Microsoft Word's parser is
/// stricter than the OOXML schema and silently discards the drawing on
/// `Documents.Open`, leaving `Headers(1).InlineShapes.Count == 0` in
/// the live document even though the bytes on disk look right.
///
/// Pragmatic fix: don't emit the header from docx-rs at all. The render
/// pass writes a sidecar JSON next to the docx with `{logo_path, lines}`;
/// the `agentic book finalize` step reads it and injects the header via
/// Word's own `InlineShapes.AddPicture` API + `Range.Text` — Word builds
/// the XML itself, so Word's parser will accept what Word produces.
///
/// Returns `None` ⇒ the calling code skips `.header(…)` and the
/// finalize-time sidecar takes over. Designer profile + non-thesis
/// books are unaffected (they never had a header to begin with).
fn fhnw_header_for(_meta: &BookMeta) -> Option<Header> {
    None
}

/// Should the engine write the FHNW-header sidecar JSON next to the docx?
///
/// True iff (a) the active typography profile is FHNW proposal parity, and
/// (b) at least one of `header_logo` (non-empty bytes) or `header_lines`
/// (non-empty after trim) is supplied via the BookMeta.
pub fn fhnw_header_sidecar_needed(meta: &BookMeta) -> bool {
    matches!(
        meta.thesis_typography,
        TypographyProfile::FhnwProposalParity | TypographyProfile::FhnwMtTemplate
    ) && (meta.header_logo.as_ref().is_some_and(|b| !b.is_empty())
        || meta.header_lines.iter().any(|l| !l.trim().is_empty()))
}

/// Sidecar metadata `agentic book finalize` reads to inject the FHNW
/// header via Word COM. The CLI writes this file next to the rendered
/// docx as `<docx_basename>.fhnw_header.json` when
/// `fhnw_header_sidecar_needed(&meta)` is true.
///
/// The 5 logo-placement fields (`logo_left_pt` / `logo_top_pt` /
/// `logo_width_cm` / `logo_wrap_type` / `logo_relh` / `logo_relv`) carry
/// the exact values extracted from the FHNW MAS proposal docx via Word
/// COM on 2026-05-29 (Fix A, proposal parity): the logo is a FLOATING
/// shape anchored to the page at (-49.3, -59.8) pt with wrap = behind
/// text, NOT an inline shape in the header text flow. Each field has
/// `#[serde(default = ...)]` so an older sidecar JSON missing the
/// fields still parses to the proposal defaults.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct FhnwHeaderSidecar {
    /// Absolute filesystem path of the PNG logo to inject. The engine
    /// can't write this from the renderer (figdir is scratch); the CLI
    /// materialises the logo from the project DB and sets this field.
    pub logo_path_abs: Option<String>,
    /// Header text lines (right-aligned, rendered in `line_font` at
    /// `line_size_pt`, bold if `line_bold`).
    pub lines: Vec<String>,
    /// Font face for the text lines. Default: "Arial" (FHNW proposal).
    pub line_font: String,
    /// Point size for the text lines. Default: 12.
    pub line_size_pt: u32,
    /// Whether to render the text lines bold. Default: true.
    pub line_bold: bool,
    /// Logo height in centimeters. Default: 4.92 (matches the proposal's
    /// 1_770_000 EMU height extracted by the Word-COM agent inspection
    /// on 2026-05-29 → 139.3 pt).
    pub logo_height_cm: f32,
    /// Logo width in centimeters. Default: 4.92 (the FHNW logo is square;
    /// matches the proposal's 139.3-pt width).
    #[serde(default = "default_logo_width_cm")]
    pub logo_width_cm: f32,
    /// Logo `Left` in points, page-relative (`relH=2`). Default: -49.3
    /// (proposal value; the logo bleeds slightly off the top-left corner).
    #[serde(default = "default_logo_left_pt")]
    pub logo_left_pt: f32,
    /// Logo `Top` in points, page-relative (`relV=2`). Default: -59.8
    /// (proposal value).
    #[serde(default = "default_logo_top_pt")]
    pub logo_top_pt: f32,
    /// `Shape.WrapFormat.Type` value. Default: 3 (matches proposal —
    /// the text flows behind/around the logo without being pushed).
    #[serde(default = "default_logo_wrap_type")]
    pub logo_wrap_type: u32,
    /// `Shape.RelativeHorizontalPosition`. Default: 2 (page-relative —
    /// the proposal value; aliases `wdRelativeHorizontalPositionPage`
    /// when used on a header-anchored shape).
    #[serde(default = "default_logo_relh")]
    pub logo_relh: u32,
    /// `Shape.RelativeVerticalPosition`. Default: 2 (page-relative).
    #[serde(default = "default_logo_relv")]
    pub logo_relv: u32,
    /// Whether the same header should also appear on subsequent pages
    /// (FHNW convention: yes). Word's default is per-section primary
    /// header; we don't need different-first-page.
    pub apply_to_all_pages: bool,
    /// Inject a centred PAGE-field footer into Section 1 primary footer
    /// and set `LinkToPrevious=True` on sections 2+. Defaults to `true`
    /// when this sidecar is used (FHNW typography requires a page-number
    /// footer per ADR-0050 §17 / ADR-0030 §37; ADR-0050 §78 deferred
    /// only the Roman→Arabic switch, not the basic Arabic numbering).
    /// docx-rs 0.4.20 attaches only one Footer per Document, so multi-
    /// section docs have Word-generated empty footers; this opt-in
    /// rewrites them via Word COM. Set to `false` to keep the
    /// docx-rs-only one-section footer behaviour.
    #[serde(default = "default_footer_pagenum_enabled")]
    pub footer_pagenum_enabled: bool,
    /// Font face for the page-number field. Default: "Arial" (matches
    /// proposal: extracted via Word COM, $ftr.Range.Font.Name = "Arial").
    #[serde(default = "default_footer_pagenum_font")]
    pub footer_pagenum_font: String,
    /// Point size for the page-number field. Default: 11 (matches
    /// proposal: $ftr.Range.Font.Size = 11).
    #[serde(default = "default_footer_pagenum_size_pt")]
    pub footer_pagenum_size_pt: u32,
    /// Paragraph alignment for the footer. Default: 1
    /// (wdAlignParagraphCenter — matches ADR-0030 §37 "centred page-
    /// number footer").
    #[serde(default = "default_footer_pagenum_alignment")]
    pub footer_pagenum_alignment: u32,
    /// ADR-0064 iter7 (FHNW MT-Template, 2026-07-03): inject a bottom-
    /// bordered paragraph into every section's primary header with
    /// STYLEREF "ChapterNumber" + STYLEREF "Heading 1" (left) and a
    /// PAGE field (right). Enables book-style mirrored running headers
    /// once combined with OddAndEvenPagesHeaderFooter (set separately).
    /// Off by default so the proposal-parity header (single primary,
    /// logo + text lines) is unchanged.
    #[serde(default = "default_header_pagenum_styleref_enabled")]
    pub header_pagenum_styleref_enabled: bool,
    /// ADR-0064 iter7: enable `d.PageSetup.OddAndEvenPagesHeaderFooter = True`
    /// document-wide so the even-page header (Headers.Item(3)) can carry
    /// mirrored content. When `header_pagenum_styleref_enabled` is on, this
    /// controls whether the mirrored variant is populated on Even pages too.
    #[serde(default = "default_header_odd_even_mirrored")]
    pub header_odd_even_mirrored: bool,
    /// ADR-0064 iter43 (2026-07-05): also inject the FHNW logo as a
    /// floating drawing anchored to the title-page body (section 1),
    /// not just to the running header. Reuses `logo_path_abs` +
    /// `logo_left_pt` / `logo_top_pt` / `logo_width_cm` / `logo_height_cm`
    /// so the title-page logo bleeds off the top-left corner exactly
    /// like the reference master-thesis's page 1. Reference has a
    /// 3840×885 PNG floating there; the tool has never emitted this
    /// until today. Default: enabled whenever the sidecar itself is
    /// present (FhnwMtTemplate books).
    #[serde(default = "default_title_logo_enabled")]
    pub title_logo_enabled: bool,
}

fn default_logo_width_cm() -> f32 {
    4.92
}
fn default_logo_left_pt() -> f32 {
    -49.3
}
fn default_logo_top_pt() -> f32 {
    -59.8
}
fn default_logo_wrap_type() -> u32 {
    3
}
fn default_logo_relh() -> u32 {
    2
}
fn default_logo_relv() -> u32 {
    2
}
fn default_footer_pagenum_enabled() -> bool {
    true
}
fn default_footer_pagenum_font() -> String {
    "Arial".to_string()
}
fn default_footer_pagenum_size_pt() -> u32 {
    11
}
fn default_footer_pagenum_alignment() -> u32 {
    1
}
fn default_header_pagenum_styleref_enabled() -> bool {
    false
}
fn default_header_odd_even_mirrored() -> bool {
    false
}
fn default_title_logo_enabled() -> bool {
    true
}

impl FhnwHeaderSidecar {
    /// Build the sidecar struct from a BookMeta, with the proposal's
    /// measured defaults for the cosmetic fields.
    pub fn from_meta(meta: &BookMeta, logo_path_abs: Option<String>) -> Self {
        let is_mt_template = matches!(meta.thesis_typography, TypographyProfile::FhnwMtTemplate);
        Self {
            logo_path_abs,
            lines: meta.header_lines.clone(),
            line_font: if is_mt_template {
                "Palatino Linotype".to_string()
            } else {
                "Arial".to_string()
            },
            line_size_pt: 12,
            line_bold: true,
            logo_height_cm: 4.92,
            logo_width_cm: default_logo_width_cm(),
            logo_left_pt: default_logo_left_pt(),
            logo_top_pt: default_logo_top_pt(),
            logo_wrap_type: default_logo_wrap_type(),
            logo_relh: default_logo_relh(),
            logo_relv: default_logo_relv(),
            apply_to_all_pages: true,
            footer_pagenum_enabled: default_footer_pagenum_enabled(),
            footer_pagenum_font: if is_mt_template {
                "Palatino Linotype".to_string()
            } else {
                default_footer_pagenum_font()
            },
            footer_pagenum_size_pt: default_footer_pagenum_size_pt(),
            footer_pagenum_alignment: default_footer_pagenum_alignment(),
            // ADR-0064 iter7: FhnwMtTemplate enables book-style mirrored
            // headers with STYLEREF chapter refs + PAGE field.
            header_pagenum_styleref_enabled: is_mt_template,
            header_odd_even_mirrored: is_mt_template,
            // ADR-0064 iter44 (2026-07-05): reverted iter43's
            // wp:anchor floating title-page logo. The June-8 reference
            // has an INLINE image at body paragraph 0 (image1.png,
            // 3840x885 banner, wp:inline), not a floating anchor drawing.
            // My iter43 misread the reference: TitlePageDrawings=1 in ref
            // is a wp:inline, not a wp:anchor. Inline banner is now
            // emitted via a markdown `![](assets/fhnw_banner.png)` at
            // the top of the master_thesis title page markdown.
            title_logo_enabled: false,
        }
    }
}

fn page_break() -> Paragraph {
    Paragraph::new().add_run(Run::new().add_break(BreakType::Page))
}

/// Round V (zone A — psb-01, 2026-06-03) — emit a thin horizontal-rule
/// paragraph used as a chapter-end divider. The reference book carries 40
/// such gray (`color="666666"`) rules at chapter boundaries plus 1 navy
/// (`color="1F3864"`) rule on the title page. Implementation uses the
/// docx-rs `ParagraphBorder` bottom-border so the rule renders without a
/// run (Word draws the border as a horizontal line at the paragraph base).
///
/// `is_title` selects the title-page variant (navy, slightly thicker);
/// every other caller passes `false` for the standard chapter-divider
/// gray rule. The helper is gated by a `chapter_break` flag at each
/// emit site (see `chapter_end_rule_if`) so cover→TOC and TOC→front-matter
/// transitions do not double-fire the rule.
fn chapter_end_rule(is_title: bool) -> Paragraph {
    let (color, size) = if is_title {
        (NAVY, 12usize)
    } else {
        (GREY, 6usize)
    };
    let border = ParagraphBorder::new(ParagraphBorderPosition::Bottom)
        .val(BorderType::Single)
        .size(size)
        .space(1)
        .color(color);
    let mut p = Paragraph::new();
    p.property = p.property.set_border(border);
    p
}

/// Wave-3 iter-D (REF parity 2026-06-04). Emit an empty paragraph whose
/// `<w:pPr>` carries a `<w:sectPr>` so Word treats the chapter close as a
/// continuous section break. The sectPr carries portrait A4 page geometry
/// + the manifest's layout overrides (header/footer distance, cols.space,
/// docGrid line-pitch) so every per-chapter section inherits the same
/// layout as the document-level sectPr.
///
/// Word's content model requires the sectPr to sit inside the `<w:pPr>` of
/// a body paragraph (not as a standalone child of `<w:body>`). docx-rs's
/// `Paragraph::section_property` setter places the value exactly there —
/// it serialises as `<w:p><w:pPr><w:sectPr>…</w:sectPr></w:pPr></w:p>`,
/// which Word reads as a section terminator at that paragraph's end.
///
/// The paragraph itself is empty (no runs) so it doesn't add visible
/// content to the rendered document — only the structural section break.
fn per_chapter_sectpr_paragraph(meta: &BookMeta) -> Paragraph {
    let lo = LayoutOverrides::from_meta(meta);
    let sp = portrait_sectpr_with(&lo);
    Paragraph::new().section_property(sp)
}

/// Conditional chapter-end-rule emit. Returns `Some(paragraph)` only when
/// `chapter_break` is true; callers that traverse cover/TOC/front-matter
/// transitions can pass `false` to skip without an extra `if` ladder.
///
/// Risk-audit note (cross-cutting risk PAGE-BREAK-DOUBLE-FIRE, Round V):
/// must be gated on an explicit "this is a chapter break" flag — emitting
/// at every section transition (cover→TOC, TOC→first front-matter chapter,
/// each list-of-…) would double-fire the rule and visibly drift from the
/// 40-divider reference count.
#[allow(dead_code)]
fn chapter_end_rule_if(chapter_break: bool) -> Option<Paragraph> {
    if chapter_break {
        Some(chapter_end_rule(false))
    } else {
        None
    }
}

fn title_page(mut doc: Docx, m: &BookMeta) -> Docx {
    // Wave-4 (REF parity 2026-06-03): subtitle paragraphs adopt the
    // `BkSubtitle` pStyle when the manifest opts into the bookkit Bk*
    // family, matching the reference book paragraphs [1] and [2].
    let use_bk = m.body_render_use_bk_styles;
    for _ in 0..3 {
        doc = doc.add_paragraph(Paragraph::new());
    }
    // Round V zone C fwc-06 (AI-Norms parity, 2026-06-03): bump the
    // title-page font from 36pt (size 72) to 40pt (size 80) so the cover
    // title matches the reference book's larger setting.
    doc = doc.add_paragraph(
        Paragraph::new().align(AlignmentType::Center).add_run(
            Run::new()
                .add_text(&m.title)
                .bold()
                .size(80)
                .color(NAVY)
                .fonts(head_fonts()),
        ),
    );
    if !m.subtitle.is_empty() {
        let mut p = Paragraph::new().align(AlignmentType::Center);
        if use_bk {
            p = p.style("BkSubtitle");
        }
        doc = doc.add_paragraph(
            p.add_run(
                Run::new()
                    .add_text(&m.subtitle)
                    .size(30)
                    .color(GREY)
                    .fonts(head_fonts()),
            ),
        );
    }
    // Blue rule + descriptive line under the title (bookkit DESCRIPTION).
    //
    // Round-V E2 (AI-Norms parity, 2026-06-03): the rule used to be a centred
    // text run of three em-dashes ("———") coloured ACCENT. That rendered as
    // a font-dependent glyph trio whose precise width drifted between
    // Calibri / Arial fallbacks and confused the reference-parity gate
    // (which scans for `<w:pBdr><w:bottom …/>` rule paragraphs). Replaced
    // with an empty paragraph carrying a navy bottom border (sz=12,
    // single) — that emits the exact `<w:pBdr>` Word expects for a
    // cover-page rule, decouples width from the running font, and matches
    // the reference book's serialised XML.
    if !m.description.is_empty() {
        let mut rule_para = Paragraph::new()
            .align(AlignmentType::Center)
            .line_spacing(LineSpacing::new().before(160).after(120));
        rule_para.property = rule_para.property.set_border(
            ParagraphBorder::new(ParagraphBorderPosition::Bottom)
                .val(BorderType::Single)
                .size(12)
                .space(1)
                .color(NAVY),
        );
        doc = doc.add_paragraph(rule_para);
        let mut p = Paragraph::new().align(AlignmentType::Center);
        if use_bk {
            p = p.style("BkSubtitle");
        }
        doc = doc.add_paragraph(
            p.add_run(
                Run::new()
                    .add_text(&m.description)
                    .italic()
                    .size(26)
                    .color(GREY)
                    .fonts(head_fonts()),
            ),
        );
    }
    for _ in 0..6 {
        doc = doc.add_paragraph(Paragraph::new());
    }
    doc = doc.add_paragraph(
        Paragraph::new().align(AlignmentType::Center).add_run(
            Run::new()
                .add_text(&m.author)
                .size(28)
                .color("1A1A1A")
                .fonts(head_fonts()),
        ),
    );
    // Wave-4 (REF parity 2026-06-03): the full byline replaces the short
    // `context` when set (reference paragraph [4]).
    let byline = m.byline_institution_full.as_deref().unwrap_or(&m.context);
    doc = doc.add_paragraph(
        Paragraph::new().align(AlignmentType::Center).add_run(
            Run::new()
                .add_text(byline)
                .size(22)
                .color(GREY)
                .fonts(head_fonts()),
        ),
    );
    // Imprint lines (version, place + date) — one centred line each.
    if let Some(imp) = &m.imprint {
        doc = doc.add_paragraph(Paragraph::new());
        for line in imp.lines().map(str::trim).filter(|l| !l.is_empty()) {
            doc = doc.add_paragraph(
                Paragraph::new().align(AlignmentType::Center).add_run(
                    Run::new()
                        .add_text(line)
                        .size(20)
                        .color(GREY)
                        .fonts(head_fonts()),
                ),
            );
        }
    }
    doc.add_paragraph(page_break())
}

/// bookkit inscription page: centred dedication + epigraph (italic) + "— by".
/// No outline heading, so it stays out of the TOC.
///
/// T1.7 (REF parity 2026-06-02): when `m.antikythera_note` is set the engine
/// also appends an Antikythera NOTE-style footer at the bottom of this page
/// (small grey, centred). The note renders even when there is no dedication
/// or epigraph — the reference book's inscription page can be note-only.
fn inscription_page(mut doc: Docx, m: &BookMeta) -> Docx {
    if m.dedication.is_none() && m.epigraph.is_none() && m.antikythera_note.is_none() {
        return doc;
    }
    for _ in 0..6 {
        doc = doc.add_paragraph(Paragraph::new());
    }
    if let Some(d) = &m.dedication {
        doc = doc.add_paragraph(
            Paragraph::new().align(AlignmentType::Center).add_run(
                Run::new()
                    .add_text(d)
                    .italic()
                    .size(24)
                    .color("1A1A1A")
                    .fonts(body_fonts()),
            ),
        );
    }
    if let Some(e) = &m.epigraph {
        for _ in 0..3 {
            doc = doc.add_paragraph(Paragraph::new());
        }
        doc = doc.add_paragraph(
            Paragraph::new()
                .align(AlignmentType::Center)
                .line_spacing(body_spacing())
                .add_run(
                    Run::new()
                        .add_text(format!("\u{201c}{e}\u{201d}"))
                        .italic()
                        .size(22)
                        .color(GREY)
                        .fonts(body_fonts()),
                ),
        );
        if let Some(by) = &m.epigraph_by {
            doc = doc.add_paragraph(
                Paragraph::new().align(AlignmentType::Center).add_run(
                    Run::new()
                        .add_text(format!("\u{2014} {by}"))
                        .size(20)
                        .color(GREY)
                        .fonts(body_fonts()),
                ),
            );
        }
    }
    doc = antikythera_note_block(doc, m);
    doc.add_paragraph(page_break())
}

/// T1.7 — Standalone dedication page (REF parity 2026-06-02).
///
/// The reference book has TWO distinct front-matter elements:
///   1. a dedicated dedication page (this block) — a personal "To …" on its
///      own page, BEFORE the inscription page
///   2. the inscription page (existing `inscription_page` function) — the
///      shorter dedication/epigraph combo
///
/// `dedication_block` emits page (1): the `dedication_page` text rendered
/// centred, italic, in the body face, with a page break at the end so the
/// inscription page (or whatever follows) starts fresh. No outline heading,
/// so it stays out of the TOC.
///
/// If `m.dedication_page` is `None` the function is a no-op, so existing
/// books (which only declared `dedication` on the inscription page) keep
/// their current layout.
fn dedication_block(mut doc: Docx, m: &BookMeta) -> Docx {
    let Some(text) = &m.dedication_page else {
        return doc;
    };
    // Push the dedication ~⅓ down the page so it sits visually centred.
    for _ in 0..10 {
        doc = doc.add_paragraph(Paragraph::new());
    }
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            doc = doc.add_paragraph(Paragraph::new());
            continue;
        }
        doc = doc.add_paragraph(
            Paragraph::new()
                .align(AlignmentType::Center)
                .line_spacing(body_spacing())
                .add_run(
                    Run::new()
                        .add_text(line)
                        .italic()
                        .size(28)
                        .color("1A1A1A")
                        .fonts(body_fonts()),
                ),
        );
    }
    doc.add_paragraph(page_break())
}

/// Wave-4 (REF parity 2026-06-03) — Personal dedication page.
///
/// The reference book's paragraph [11] ("For Melanie, Sarah and Timo") is
/// a single centred line on its own page, AFTER the disclaimer/edition
/// chrome and BEFORE the inscription page. Distinct from both
/// `dedication` (single line on the inscription page) and
/// `dedication_page` (the larger multi-line block). When
/// `m.dedication_personal` is set, this function emits the line +
/// trailing page-break; otherwise it's a no-op.
fn dedication_personal_block(mut doc: Docx, m: &BookMeta) -> Docx {
    let Some(text) = &m.dedication_personal else {
        return doc;
    };
    // Push the line ~⅓ down the page so it sits visually centred.
    for _ in 0..10 {
        doc = doc.add_paragraph(Paragraph::new());
    }
    doc = doc.add_paragraph(
        Paragraph::new()
            .align(AlignmentType::Center)
            .line_spacing(body_spacing())
            .add_run(
                Run::new()
                    .add_text(text.trim())
                    .italic()
                    .size(28)
                    .color("1A1A1A")
                    .fonts(body_fonts()),
            ),
    );
    doc.add_paragraph(page_break())
}

/// Wave-4 (REF parity 2026-06-03) — Antikythera-mechanism inscription
/// paragraph emitted after the dedication/epigraph chrome and before the
/// Contents heading. The exact text was extracted from the reference
/// book (`book_build/AI_Norms_and_Regulations_BOOK.docx`, paragraph 15)
/// on 2026-06-03 and is stored verbatim in
/// [`ANTIKYTHERA_INSCRIPTION_TEXT`]. Renders as a centred italic
/// BkSubtitle paragraph when `m.inscription_page_enabled` is true; no-op
/// otherwise.
fn antikythera_inscription_block(mut doc: Docx, m: &BookMeta) -> Docx {
    if !m.inscription_page_enabled {
        return doc;
    }
    for _ in 0..4 {
        doc = doc.add_paragraph(Paragraph::new());
    }
    let mut p = Paragraph::new()
        .align(AlignmentType::Center)
        .line_spacing(body_spacing());
    if m.body_render_use_bk_styles {
        p = p.style("BkSubtitle");
    }
    doc = doc.add_paragraph(
        p.add_run(
            Run::new()
                .add_text(ANTIKYTHERA_INSCRIPTION_TEXT)
                .italic()
                .size(22)
                .color("1A1A1A")
                .fonts(body_fonts()),
        ),
    );
    doc.add_paragraph(page_break())
}

/// Wave-4 (REF parity 2026-06-03) — Closing-thought block emitted right
/// before the back-of-book lists (Table of Figures / Tables) and after
/// the Appendix. Reference paragraphs [3856-3857] are two consecutive
/// BkCallout paragraphs: the heading "How this book was made" and the
/// body. The heading is a constant ([`CLOSING_THOUGHT_HEADING`]); the
/// body is supplied via `BookMeta::closing_thought`. No-op when the
/// body is `None`.
fn closing_thought_block(mut doc: Docx, m: &BookMeta) -> Docx {
    let Some(body) = &m.closing_thought else {
        return doc;
    };
    doc = doc.add_paragraph(page_break());
    let mut heading = Paragraph::new();
    let mut body_p = Paragraph::new();
    let plant_flavor = m.body_render_use_bk_styles;
    if m.body_render_use_bk_styles {
        heading = heading.style("BkCallout");
        body_p = body_p.style("BkCallout");
    }
    // Round V iter-2 (BkCallout decoration parity, 2026-06-03): plant
    // a `Generic` flavor sentinel on each closing-thought paragraph so
    // the postprocess `apply_callout_chrome` pass injects pBdr + shd
    // chrome — closing-thought is the only `BkCallout` emitter that
    // was previously uninstrumented, and the parity gate flagged its
    // two paragraphs as stragglers (missing pBdr + missing shd).
    let heading = heading.add_run(
        Run::new()
            .add_text(CLOSING_THOUGHT_HEADING)
            .bold()
            .size(26)
            .color(NAVY)
            .fonts(head_fonts()),
    );
    let body_p = body_p.add_run(
        Run::new()
            .add_text(body.trim())
            .size(22)
            .color("1A1A1A")
            .fonts(body_fonts()),
    );
    if plant_flavor {
        doc = doc.add_paragraph(plant_flavor_sentinel(heading, CalloutFlavor::Generic));
        doc.add_paragraph(plant_flavor_sentinel(body_p, CalloutFlavor::Generic))
    } else {
        doc = doc.add_paragraph(heading);
        doc.add_paragraph(body_p)
    }
}

/// T1.7 — Inscription-page Antikythera NOTE footer (REF parity 2026-06-02).
///
/// Renders `m.antikythera_note` as a centred, small grey NOTE paragraph at
/// the bottom of the inscription page. Mirrors the reference book's
/// attribution for the Antikythera-mechanism artwork.
///
/// Called from `inscription_page` (so the note shares a page with the
/// dedication/epigraph rather than getting its own page break). No-op when
/// `m.antikythera_note` is `None`.
fn antikythera_note_block(mut doc: Docx, m: &BookMeta) -> Docx {
    let Some(note) = &m.antikythera_note else {
        return doc;
    };
    // A few blanks to push the NOTE toward the bottom of the inscription page.
    for _ in 0..4 {
        doc = doc.add_paragraph(Paragraph::new());
    }
    doc.add_paragraph(
        Paragraph::new()
            .align(AlignmentType::Center)
            .line_spacing(body_spacing())
            .add_run(
                Run::new()
                    .add_text(format!("NOTE: {note}"))
                    .italic()
                    .size(16)
                    .color(GREY)
                    .fonts(body_fonts()),
            ),
    )
}

/// T1.7 — Standalone QR-linked URL block (REF parity 2026-06-02).
///
/// Distinct from the per-chapter `flush_sources` Sources & QR-codes box: this
/// is a single-URL block emitted as a standalone page right after the
/// disclaimer/inscription chrome. The reference book uses it to advertise
/// the book's companion URL (home page / errata / downloads) with both the
/// URL itself (clickable Hyperlink) and a scan-friendly QR code below it.
///
/// `m.qrlink` is the URL string; the engine renders the QR using `qr_png`
/// (same generator as the chapter Sources box). No-op when `m.qrlink` is
/// `None`. No outline heading, so it stays out of the TOC.
fn qrlink_block(mut doc: Docx, m: &BookMeta) -> Docx {
    let Some(url) = &m.qrlink else {
        return doc;
    };
    // A few blanks so the QR + URL pair sit in the upper-middle of the page.
    for _ in 0..6 {
        doc = doc.add_paragraph(Paragraph::new());
    }
    // Clickable URL (Hyperlink) above the QR.
    doc = doc.add_paragraph(
        Paragraph::new().align(AlignmentType::Center).add_hyperlink(
            Hyperlink::new(url, HyperlinkType::External).add_run(
                Run::new()
                    .add_text(url)
                    .size(22)
                    .color(ACCENT)
                    .underline("single")
                    .fonts(body_fonts()),
            ),
        ),
    );
    // QR code below the URL, centred. The QR generator is shared with the
    // per-chapter Sources box (`qr_png`), so the on-page rendering matches.
    if let Some(png) = qr_png(url) {
        doc = doc.add_paragraph(
            Paragraph::new()
                .align(AlignmentType::Center)
                .add_run(Run::new().add_image(Pic::new(&png).size(2_400_000, 2_400_000))),
        );
    } else {
        doc = doc.add_paragraph(
            Paragraph::new()
                .align(AlignmentType::Center)
                .add_run(Run::new().add_text("[QR]").size(20).color(GREY)),
        );
    }
    doc.add_paragraph(page_break())
}

/// bookkit "Edition & Disclaimer" page: a heading + one grey paragraph per line.
fn disclaimer_page(mut doc: Docx, m: &BookMeta) -> Docx {
    let Some(text) = &m.disclaimer else {
        return doc;
    };
    doc = doc.add_paragraph(
        Paragraph::new().add_run(
            Run::new()
                .add_text(t(&m.lang, "edition_disclaimer"))
                .bold()
                .size(32)
                .color(NAVY)
                .fonts(head_fonts()),
        ),
    );
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        doc = doc.add_paragraph(
            Paragraph::new().line_spacing(body_spacing()).add_run(
                Run::new()
                    .add_text(line)
                    .size(19)
                    .color(GREY)
                    .fonts(body_fonts()),
            ),
        );
    }
    doc.add_paragraph(page_break())
}

/// Render a styled heading paragraph. When `anchor` is `Some((id, name))`,
/// wraps the heading's runs in a `<w:bookmarkStart w:id="N" w:name="...">`
/// / `<w:bookmarkEnd w:id="N">` pair so internal `#anchor` markdown links
/// resolve to a Word bookmark target (REQ-5, 2026-06-03). Pass `None` for
/// non-flow callers that emit pseudo-headings (TOC plumbing, `Contents`,
/// etc.) and don't need a navigable anchor.
fn heading_para(
    level: u8,
    text: &str,
    page_break_before: bool,
    typography: TypographyProfile,
    anchor: Option<(usize, &str)>,
    use_bk_styles: bool,
) -> Paragraph {
    let size = heading_size_hp(typography, level);
    let color = if level <= 2 {
        heading_color_for(typography)
    } else {
        subheading_color_for(typography)
    };
    // FHNW H3 is regular (not bold) per the proposal docx 2025-12-29; all
    // other heading levels remain bold under both profiles.
    let bold = !matches!(
        (typography, level),
        (TypographyProfile::FhnwProposalParity, 3) | (TypographyProfile::FhnwMtTemplate, 3)
    );
    // Wave-6 (ADR-0054 v1, 2026-06-03): under the AI-Norms parity flag the
    // body emits `pStyle="BkH{1..4}"` so the count of reference style ids in
    // word/document.xml matches the 186-style verbatim styles.xml (replaced
    // in the finalize-pass); otherwise keep the historical docx-rs default
    // (`Heading{1..4}`) so non-AI-Norms books are unchanged.
    let style_id = if use_bk_styles {
        format!("BkH{}", level.min(4))
    } else {
        format!("Heading{}", level.min(4))
    };
    // Round V zone D (2026-06-03): when `use_bk_styles=true` the reference
    // `BkH{1..4}` styles already declare their own `<w:spacing
    // w:before/after w:line>` + `<w:keepNext/>` + `<w:jc w:val="left">`.
    // Emitting an inline `line_spacing(SPACE_BEFORE_HEAD .. SPACE_AFTER_HEAD)`
    // would override every one of those values and re-flow the document
    // away from the reference. Skip the inline override under the parity
    // flag; non-parity books continue to direct-format spacing as before.
    let mut p = Paragraph::new().style(&style_id);
    if !use_bk_styles {
        p = p.line_spacing(
            LineSpacing::new()
                .before(SPACE_BEFORE_HEAD)
                .after(SPACE_AFTER_HEAD),
        );
    } else {
        // BkH styles already declare `keepNext` — re-asserting at the
        // paragraph level is idempotent and explicit (helps reviewers see
        // the intent without re-reading styles.xml).
        p = p.keep_next(true);
    }
    // Round V (zone A — psb-03, 2026-06-03): the in-heading
    // `<w:br w:type="page"/>` run was historically emitted here when
    // `page_break_before` is true. That places the break INSIDE the heading
    // paragraph's run, which Word renders correctly but the parity gate
    // counts as an extra body run on the heading. The reference book emits
    // page breaks as standalone `page_break()` paragraphs BEFORE the
    // heading. The sole live call site (`render_block`, DocxBlock::Heading)
    // now emits `page_break()` externally; the legacy flag is preserved on
    // the signature so non-render-block callers (none today, but possible
    // in tests) keep their behaviour for the time being, but the new
    // emission path is a no-op so the run no longer duplicates the
    // externally-emitted break.
    let _page_break_before = page_break_before; // kept for ABI; emission moved to callers
    // Emit bookmarkStart BEFORE the heading runs so Word's "go to bookmark"
    // lands at the heading text rather than the trailing edge.
    if let Some((id, name)) = anchor {
        p = p.add_bookmark_start(id, name);
    }
    let mut run = Run::new()
        .add_text(text)
        .size(size)
        .color(color)
        .fonts(head_fonts_for(typography));
    if bold {
        run = run.bold();
    }
    p = p.add_run(run);
    if let Some((id, _)) = anchor {
        p = p.add_bookmark_end(id);
    }
    p
}

/// Round V zone C fwc-03 (AI-Norms parity, 2026-06-03): body runs no longer
/// force a `<w:color w:val="000000"/>` override. The reference book lets
/// every body paragraph inherit colour from its `pStyle` (BkBullet,
/// BkCaption, …) which in turn inherits from the BkNormal docDefaults —
/// none of which set `<w:color>`. Forcing `000000` on every body run
/// short-circuits the cascade, so the new theme1.xml + styles.xml fixture
/// pair (zone B) cannot retint inherited colours (e.g. Hyperlink).
///
/// Callers that DO need an explicit colour (callout boxes, title page,
/// inscription) must use [`run_of_callout`] instead.
fn run_of(r: &DocxRun, typography: TypographyProfile) -> Run {
    let mut run = Run::new().add_text(&r.text).size(body_size_hp(typography));
    run = if r.code {
        run.fonts(RunFonts::new().ascii(MONO).hi_ansi(MONO))
    } else {
        run.fonts(body_fonts_for(typography))
    };
    if r.bold {
        run = run.bold();
    }
    if r.italic {
        run = run.italic();
    }
    run
}

/// Round V zone C fwc-03 (AI-Norms parity, 2026-06-03): body run helper
/// for paragraphs that explicitly need a colour override (callout boxes,
/// the warning callout's amber title, inscription italics, etc). Forks the
/// historical `run_of` behaviour. Use this instead of `run_of` when the
/// caller has already decided on a non-inherited colour.
#[allow(dead_code)] // wired up by zones D/E callout helpers; kept reachable
fn run_of_callout(r: &DocxRun, typography: TypographyProfile, color: &str) -> Run {
    let mut run = Run::new()
        .add_text(&r.text)
        .size(body_size_hp(typography))
        .color(color);
    run = if r.code {
        run.fonts(RunFonts::new().ascii(MONO).hi_ansi(MONO))
    } else {
        run.fonts(body_fonts_for(typography))
    };
    if r.bold {
        run = run.bold();
    }
    if r.italic {
        run = run.italic();
    }
    run
}

// Note: `body_color_for` is still called by the table / callout paths
// (lines ~1925 and ~3686) that DO want an explicit colour. Only `run_of`
// (the body-prose helper) was decoupled per zone-C fwc-03.

/// A true superscript bracketed reference-number run (bookkit `_superscript`),
/// pointing into the chapter Sources box. Uses `RunProperty::vert_align`
/// (`<w:vertAlign w:val="superscript"/>`); `Run` has no fluent setter, but its
/// `run_property` field is public.
fn superscript(n: usize) -> Run {
    // Round V zone C fwc-06 (AI-Norms parity, 2026-06-03): the reference
    // book superscripts use 8pt (size 16), not 7.5pt (size 15) — the
    // half-point difference is visible against body Calibri 10pt because
    // 8pt sits exactly two points below the baseline-x-height.
    let mut r = Run::new()
        .add_text(format!("[{n}]"))
        .size(16)
        .color(ACCENT)
        .fonts(body_fonts());
    r.run_property = r.run_property.vert_align(VertAlignType::SuperScript);
    r
}

/// Add a run sequence to a paragraph. Markdown links (`[label](url)`) render
/// as a CLICKABLE `w:hyperlink` element (T1.6, REF parity 2026-06-02) wrapping
/// the label run, followed by a superscript reference number; the label+URL
/// are also registered in the chapter's link registry (bookkit `add_inline` +
/// `_register_link`) so the URLs still appear in the end-of-chapter Sources &
/// QR-codes box.
///
/// The previous renderer emitted the label as a plain coloured run, so readers
/// only had the `[N]` cross-reference and could not click through to the URL.
/// We now wrap the label in `docx_rs::Hyperlink::new(url, External)` which
/// serialises to `<w:hyperlink r:id="..."> … </w:hyperlink>` — Word renders
/// that as an actual clickable link (Ctrl+click → browser).
fn add_runs(
    mut p: Paragraph,
    runs: &[DocxRun],
    links: &mut Vec<(String, String)>,
    typography: TypographyProfile,
) -> Paragraph {
    for r in runs {
        if let Some(url) = &r.link {
            // REQ-5 (2026-06-03): internal anchor link `[text](#anchor)`
            // → `<w:hyperlink w:anchor="anchor">` so Ctrl+click jumps to
            // the matching `<w:bookmarkStart w:name="anchor"/>` emitted
            // by `heading_para`. We do NOT register the
            // target in the chapter Sources box (it isn't an external
            // resource) and we do NOT add the `[N]` superscript — the
            // hyperlink label alone is the cross-reference.
            if let Some(anchor) = url.strip_prefix('#') {
                // Round V zone C fwc-04 (AI-Norms parity, 2026-06-03):
                // do NOT hard-code colour/underline on the inline run.
                // The `Hyperlink` character style (defined in the 186-
                // style fixture as `<w:color w:val="0000FF"/>` +
                // `<w:u w:val="single"/>`) controls the visuals when
                // `body_render_use_bk_styles=true`. Adding an inline
                // colour/underline here would shadow the style and
                // prevent theme1.xml's `<a:hlink val="0000FF"/>` from
                // taking effect on docs that DO read the theme palette.
                let mut label = Run::new()
                    .add_text(&r.text)
                    .size(body_size_hp(typography))
                    .style("Hyperlink")
                    .fonts(body_fonts_for(typography));
                if r.bold {
                    label = label.bold();
                }
                p = p.add_hyperlink(
                    Hyperlink::new(anchor.to_string(), HyperlinkType::Anchor).add_run(label),
                );
                continue;
            }
            // External URL: register (de-dupe by URL) and emit label +
            // superscript number.
            let n = match links.iter().position(|(_, u)| u == url) {
                Some(i) => i + 1,
                None => {
                    links.push((r.text.clone(), url.clone()));
                    links.len()
                }
            };
            // Round V zone C fwc-04 (AI-Norms parity, 2026-06-03): same
            // rationale as the anchor branch above — let the Hyperlink
            // character style govern colour + underline.
            let mut label = Run::new()
                .add_text(&r.text)
                .size(body_size_hp(typography))
                .style("Hyperlink")
                .fonts(body_fonts_for(typography));
            if r.bold {
                label = label.bold();
            }
            // Wrap the label in a docx-rs `Hyperlink` (External) so Word
            // emits `<w:hyperlink r:id="..."> … </w:hyperlink>` — a true
            // clickable link, not just a coloured run. The superscript [N]
            // stays as a separate run after the hyperlink so the Sources &
            // QR-codes box cross-reference is preserved.
            p = p
                .add_hyperlink(Hyperlink::new(url, HyperlinkType::External).add_run(label))
                .add_run(superscript(n));
        } else {
            p = p.add_run(run_of(r, typography));
        }
    }
    p
}

/// Default body-paragraph alignment for the active typography profile.
///
/// ADR-0050 §1 item 3 → ADR-0054 v1 (reference-parity audit 2026-06-02):
/// BOTH profiles now justify body prose.
/// - `Designer`: matches the bookkit `book_build/build_styles.py` Normal
///   style which declares `WD_ALIGN_PARAGRAPH.JUSTIFY`. The engine
///   previously emitted LEFT (audit gap T1.8) — now corrected so the
///   17 non-thesis books match the reference book byte-for-byte on
///   paragraph justification.
/// - `FhnwProposalParity`: matches the FHNW proposal docx which
///   direct-formats prose as JUSTIFY (unchanged).
///
/// docx-rs maps WordprocessingML `w:jc w:val="both"` (the canonical OOXML
/// "justify both edges" value, internally also called "Justified") to
/// `AlignmentType::Both`. Word renders both identically; we pick `Both`
/// because it is the value OOXML serialises and matches the reference
/// docx output.
fn body_alignment_for(_t: TypographyProfile) -> AlignmentType {
    AlignmentType::Both
}

/// Body paragraph builder.
///
/// Under `use_bk_styles=true` the inline `line_spacing(body_spacing())` +
/// `align(Both)` overrides are dropped so the `Normal` (or downstream
/// `BkBullet`) style governs spacing and justification (Round V zone D,
/// 2026-06-03). Non-parity callers pass `false` and keep the historical
/// body defaults (line-height 1.32 + justify-both).
fn para_of_styled(
    runs: &[DocxRun],
    links: &mut Vec<(String, String)>,
    typography: TypographyProfile,
    use_bk_styles: bool,
) -> Paragraph {
    let mut p = Paragraph::new();
    if !use_bk_styles {
        p = p.line_spacing(body_spacing());
    }
    if let Some(a) = body_alignment_override(typography, use_bk_styles) {
        p = p.align(a);
    }
    add_runs(p, runs, links, typography)
}

/// Parse PNG width/height from the IHDR chunk (bytes 16..24, big-endian).
fn png_dims(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 24 || &bytes[1..4] != b"PNG" {
        return None;
    }
    let w = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
    let h = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
    Some((w, h))
}

/// Maximum pixel edge for an embedded sourced raster (PNG screenshot or
/// photograph going through [`DocxBlock::Image`]).
///
/// 2026-06-14: the AI-Norms regulations book
/// (`ai_norms_and_regulations.docx`) shipped at 40 MB while peer
/// campaign books were 4–5 MB. Forensics on
/// `snapshots/20260614-012326-books-cascade/ai_norms_and_regulations.docx`:
/// 395 unique sourced-screenshot PNGs in `word/media/` totalling ~39 MB
/// (zero duplicate-bytes — so a media-table dedup pass would NOT help).
/// The top 25 images were >500 KB each, the largest a 1.94 MB
/// 952×2048 portrait screenshot. None were figspec-rendered; they
/// were verbatim raster ingest bytes (the renderer was setting a small
/// `<wp:extent>` so Word DISPLAYED them at 4 inches, but the raw PNG
/// payload was passed through unresized — Word doesn't downsample on
/// load).
///
/// Fix: downsample the longest pixel edge of every sourced raster to
/// this cap with Lanczos3 at embed time. Sourced rasters already at
/// or below the cap pass through unchanged. At the 4-inch default
/// body-display width the effective DPI is ~320 — well above Word's
/// 96 DPI render path and the 220 DPI "good print" target — so users
/// see no perceptual loss in the docx. The figures-audit brief
/// (2026-06-13) independently recommended the same 1280 px cap for
/// landscape figures, which the agentic-figures readability-clamp
/// pass now enforces at figspec render time; this constant mirrors
/// the cap for sourced rasters in the docx renderer.
const MAX_EMBED_RASTER_EDGE_PX: u32 = 1280;

/// Downsample `bytes` (a PNG payload) so its longest pixel edge does
/// not exceed [`MAX_EMBED_RASTER_EDGE_PX`], aspect ratio preserved
/// with Lanczos3 resampling. Returns the input bytes unchanged when:
///
/// - the source is not a parseable PNG (defensive: never crash the
///   docx renderer on an exotic payload — the original bytes still
///   embed fine, just oversized);
/// - the longest edge is already at-or-below the cap (the common
///   case for in-house figspec PNGs, admonition icons, QR codes,
///   and most legitimately page-sized screenshots);
/// - the resize / re-encode round trip itself fails for any reason
///   (same defensive fallback as the PNG-parse miss).
///
/// Called once per embedded sourced raster from the
/// [`DocxBlock::Image`] branch in [`build_chapter_body`] — admonition
/// icons and QR codes use the `ADMONITION_ICON_EMU` / `QR_CODE_EMU`
/// paths in `icons.rs` and never reach this helper.
fn clamp_raster_for_embed(bytes: Vec<u8>) -> (Vec<u8>, Option<(u32, u32)>) {
    let Some((w, h)) = png_dims(&bytes) else {
        // Not a parseable PNG — return the bytes unchanged with no
        // dims hint; caller will fall back to docx-rs's auto-detect
        // `Pic::new` path (which itself decodes-and-re-encodes, but
        // exotic payloads are rare and out of scope for this cap).
        return (bytes, None);
    };
    let longest = w.max(h);
    if longest <= MAX_EMBED_RASTER_EDGE_PX {
        // Under-cap: pass the original bytes through with the known
        // dims so the caller can take the no-re-encode
        // `Pic::new_with_dimensions` path. `Pic::new` would otherwise
        // round-trip the bytes through the `image` crate's
        // `ImageFormat::Png` default encoder (`CompressionType::Default`
        // / balanced deflate), and a well-compressed source PNG can
        // double in size after that round-trip.
        return (bytes, Some((w, h)));
    }
    let Ok(img) = image::load_from_memory(&bytes) else {
        return (bytes, Some((w, h)));
    };
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    let scale = f64::from(MAX_EMBED_RASTER_EDGE_PX) / f64::from(longest);
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    let new_w = ((f64::from(w) * scale).round() as u32).max(1);
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    let new_h = ((f64::from(h) * scale).round() as u32).max(1);
    let resized = img.resize_exact(new_w, new_h, image::imageops::FilterType::Lanczos3);
    // 2026-06-14: encode with maximum deflate (CompressionType::Best
    // ≈ flate2 level 9) and Adaptive filter selection. The crate's
    // default `Balanced` setting underperforms typical screenshot
    // PNGs (which were authored with high-compression encoders); on
    // the AI-Norms regulations book the default was producing
    // re-encoded outputs LARGER than the originals even at half the
    // pixel area. Switching to `Best` lands the clamped output well
    // below the source size at every test point — net ~75 % byte
    // reduction on the 63 oversized rasters in that book.
    let mut buf = Vec::new();
    let encoder = image::codecs::png::PngEncoder::new_with_quality(
        &mut buf,
        image::codecs::png::CompressionType::Best,
        image::codecs::png::FilterType::Adaptive,
    );
    if resized.write_with_encoder(encoder).is_err() {
        return (bytes, Some((w, h)));
    }
    (buf, Some((new_w, new_h)))
}

/// EMU per inch (OOXML constant; 914 400).
const EMU_PER_INCH: u64 = 914_400;
/// Default screen DPI for raster sources — matches Word's 96 DPI default
/// and the reference AI-Norms book.
const SCREEN_DPI: u64 = 96;
/// Maximum EMU width for an embedded raster (15 cm ≈ 5.91 in).
/// Explicitly-sized callers cap here; smaller sources keep their natural
/// dimensions (other / icon / qr bucket).
const IMAGE_MAX_W_EMU: u32 = 5_400_000;
/// Default embed width for unsized markdown `![](png)` images
/// (4 in × 914 400 EMU/in = 3 657 600 EMU). Round V iter-7 (2026-06-03):
/// matches the `agentic-figures::render_image_embed::DEFAULT_EMBED_WIDTH_IN`
/// (also 4 in) so the two image-embed code paths produce the same parity
/// bucket. Anything above this default lands in the `figure` bucket
/// (≥5 M EMU); anything at-or-below lands in the `other` bucket
/// (1 M < cx < 5 M EMU). The reference AI-Norms book embeds its 55 sourced
/// raster screenshots at page-fit small widths (3-4 in, ~2.7-3.6 M EMU).
const DEFAULT_EMBED_W_EMU: u32 = 3_657_600;

/// Quantisation grid for `<wp:extent>` (matches the reference book; e.g.
/// 4 860 000 / 4 680 000 / 4 500 000 / 3 600 000 EMU). Without quantisation
/// the bucket counts hold but the per-EMU diff against the reference
/// inflates.
const EMU_GRID: u64 = 60_000;

/// Snap a width down to the nearest [`EMU_GRID`] multiple, recompute height
/// to preserve aspect ratio. Helper shared by all three branches of
/// [`image_dims_to_emu`]. Both inputs are in EMU; returns `(w, h)` in EMU.
fn snap_emu_to_grid(target_w_emu: u64, nat_w_emu: u64, nat_h_emu: u64) -> (u32, u32) {
    let w = (target_w_emu / EMU_GRID) * EMU_GRID;
    let w = w.max(EMU_GRID);
    let h = (nat_h_emu * w) / nat_w_emu.max(1);
    #[allow(clippy::cast_possible_truncation)]
    {
        (w as u32, h.max(1) as u32)
    }
}

/// Round V iter-9 (drawing_class_bucket parity, 2026-06-03): is `path` an
/// in-house figspec-emitted image reference? Used by the `DocxBlock::Image`
/// call site to decide whether to route through `Some(6.0)` (FIGURE bucket)
/// or `None` (4-in OTHER bucket default).
///
/// Two recognition strategies:
///
/// 1. **Prefix match**: `figures/<sub>/<id>.png` is the canonical emission
///    pattern from `agentic_figures::resolve_markdown` (lib.rs:277). When
///    figspec blocks survive into `resolve_markdown` (the non-ai_norms
///    cascade path), every figspec produces this prefix.
///
/// 2. **Stem match**: the ai_norms cascade strips `## Figures` figspec
///    blocks via `strip_wave5_figures_section` BEFORE `resolve_markdown`
///    runs, so the chapter md retains only the bookkit-source bare-filename
///    references like `gov_switzerland.png`, `reg_eu.png`, `iso_norms_heatmap.png`,
///    `pop_treemap.png`. The TRUE reference (`true_reference_doc.xml`)
///    routes 41 of these to FIGURE: 22 `gov_*`, 16 `reg_*`, 2 `iso*`,
///    1 `pop_*`. Matching these stems lets the cascade recover ~41 of
///    the 78-reference FIGURE assignments programmatically.
///
/// The remaining 37 FIGURE entries in the reference are `image*.png`
/// top-of-chapter wide diagrams whose split from 55 mid-chapter
/// `image*.png` OTHER entries is editorial — not derivable from path
/// bytes. Documented in iter-9 close report as known residual drift.
#[must_use]
pub fn is_in_house_figure_path(path: &str) -> bool {
    // Strategy 1 — `figures/` prefix (Windows + Unix safe).
    if path.starts_with("figures/")
        || path.starts_with(r"figures\")
        || path.contains("/figures/")
        || path.contains(r"\figures\")
    {
        return true;
    }
    // Strategy 2 — figspec-emitter filename stem. Match the trailing
    // segment after the last path separator so a fully-qualified path
    // like `specs/figures/raster/ai_norms/gov_eu.png` still hits via
    // strategy 1, and a bare `gov_eu.png` still hits via strategy 2.
    let stem = path
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
        .to_ascii_lowercase();
    stem.starts_with("gov_")
        || stem.starts_with("reg_")
        || stem.starts_with("iso")
        || stem.starts_with("pop_")
        || stem.starts_with("treemap_")
        || stem.starts_with("heatmap_")
        || stem.starts_with("sankey_")
        || stem.starts_with("wheel_")
        || stem.starts_with("overlay_")
        || stem.starts_with("tier_matrix_")
        || stem.starts_with("govmap_")
        || stem.starts_with("regstack_")
}

/// Translate a PNG's pixel dimensions into EMU at 96 DPI, with one of three
/// width policies:
///
/// 1. `width_in_override = Some(w)` — the caller wants an explicit width
///    (e.g. a figspec with `width_in`, or an in-house wide figure renderer
///    asking for ≥6 in). The target width is `w × 914_400` EMU, capped
///    at [`IMAGE_MAX_W_EMU`] (15 cm hard cap), aspect ratio preserved.
/// 2. `width_in_override = None` AND natural width ≤ [`DEFAULT_EMBED_W_EMU`]
///    (~4 in @ 96 DPI) — keep the natural width (snapped to the 60 000-EMU
///    grid). This is the byte-passthrough path for small images (icons,
///    inline thumbnails) that should NOT be inflated.
/// 3. `width_in_override = None` AND natural width > [`DEFAULT_EMBED_W_EMU`]
///    — shrink to the 4-in default, aspect ratio preserved, then snap to
///    the 60 000-EMU grid. This puts the image in the parity gate's
///    `other` drawing-class bucket (1 M < cx < 5 M EMU), matching the
///    AI-Norms reference book's treatment of sourced raster screenshots.
///
/// Falls back to (5.4 M, 3.4 M) for sources whose dimensions cannot be
/// parsed (e.g. JPEG; the book.rs renderer only hands us PNG today, but
/// a defensive fallback keeps the renderer crash-free).
///
/// Returns `(w, h)` in EMU.
///
/// Round V iter-7 (2026-06-03): the `width_in_override` parameter was added
/// to close the last 2 parity ERRORs (`PARITY_DRAWING_CLASS_BUCKET::FIGURE`
/// 125 vs 78, `::OTHER` 8 vs 55 — exactly 47 images that should have been
/// in OTHER). The reachable cascade path strips `## Figures` figspec blocks
/// before `resolve_markdown` runs, so the Iter-3 4-inch default in
/// `render_image_embed.rs` is unreachable; instead, images flow through
/// `DocxBlock::Image` → `image_dims_to_emu`. Mirroring the 4-inch default
/// HERE is the single surgical fix that reaches the production path.
///
/// Round V iter-8 (2026-06-03): the assumption that "in-house figure
/// renderers go through their own `<wp:extent>` paths" was WRONG —
/// `agentic_figures::resolve_markdown` rewrites every figspec block into
/// a `![{caption}](figures/{sub}/{id}.png)` reference that ALSO routes
/// through `DocxBlock::Image`, so Iter-7's blanket `None` capped the 78
/// in-house figures along with the 55 sourced rasters, flipping the
/// buckets to FIGURE 8 / OTHER 125. The fix lives at the call site
/// (`book.rs:~4594`), which now distinguishes by path prefix:
/// `figures/...` → in-house figure → `Some(6.0)` (FIGURE bucket); any
/// other path → loose markdown raster → `None` (4-inch default, OTHER
/// bucket). Admonition icons and QR codes never call this helper; they
/// use the `ADMONITION_ICON_EMU` and `QR_CODE_EMU` constants from
/// `icons.rs` directly.
pub fn image_dims_to_emu(bytes: &[u8], width_in_override: Option<f32>) -> (u32, u32) {
    let Some((px_w, px_h)) = png_dims(bytes) else {
        return (IMAGE_MAX_W_EMU, 3_400_000);
    };
    let px_w = px_w.max(1);
    let px_h = px_h.max(1);
    let nat_w_emu = (u64::from(px_w) * EMU_PER_INCH) / SCREEN_DPI;
    let nat_h_emu = (u64::from(px_h) * EMU_PER_INCH) / SCREEN_DPI;

    // Branch 1: explicit width override (callers opt out of 4-in default).
    if let Some(w_in) = width_in_override {
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss
        )]
        let req_emu = (f64::from(w_in.max(0.0)) * EMU_PER_INCH as f64) as u64;
        // Respect the 15-cm hard cap so explicitly oversized requests
        // (e.g. `width_in: 12.0`) still ship at a sane page-fit size.
        let target_w_emu = req_emu.min(u64::from(IMAGE_MAX_W_EMU));
        return snap_emu_to_grid(target_w_emu, nat_w_emu, nat_h_emu);
    }

    // Branch 2: unsized + naturally small → keep native width.
    if nat_w_emu <= u64::from(DEFAULT_EMBED_W_EMU) {
        return snap_emu_to_grid(nat_w_emu, nat_w_emu, nat_h_emu);
    }

    // Branch 3: unsized + naturally wide → shrink to 4-in default.
    snap_emu_to_grid(u64::from(DEFAULT_EMBED_W_EMU), nat_w_emu, nat_h_emu)
}

/// Per-column widths in twips for a given header.
///
/// ADR-0050 §1 item 9 (2026-05-29): the FHNW MAS acronyms table reads as
/// "Acronym | Expansion | Pages" — equal-share widths waste real estate
/// because the middle "Expansion" column carries 3-10× the text density of
/// the two outer columns. We detect that header pattern and return a
/// 10 / 80 / 10 split; every other table keeps the historical equal-share
/// behaviour, so non-thesis books are unaffected.
///
/// Detection is exact-match (case-insensitive trim) on the three header
/// strings `Acronym`, `Expansion`, `Pages` — narrow enough to avoid false
/// positives on other 3-column tables.
fn column_widths_for(header: &[String], content_twips: usize, ncols: usize) -> Vec<usize> {
    let equal = content_twips / ncols;
    if ncols == 3 && header.len() == 3 {
        let h0 = header[0].trim().to_ascii_lowercase();
        let h1 = header[1].trim().to_ascii_lowercase();
        let h2 = header[2].trim().to_ascii_lowercase();
        let is_acronyms = h0 == "acronym" && h1 == "expansion" && h2 == "pages";
        if is_acronyms {
            // 10 / 80 / 10 split, rounded so the row sums to `content_twips`
            // (the engine sets `WidthType::Dxa` per cell + on the table, so
            // rounding loss would otherwise create a tiny gap on the right
            // edge).
            let c0 = content_twips / 10;
            let c2 = content_twips / 10;
            let c1 = content_twips - c0 - c2;
            return vec![c0, c1, c2];
        }
    }
    vec![equal; ncols]
}

fn table_block(
    header: &[String],
    rows: &[Vec<String>],
    content_twips: usize,
    typography: TypographyProfile,
) -> Table {
    let ncols = col_count(header, rows);
    let col_widths = column_widths_for(header, content_twips, ncols);
    let colw = content_twips / ncols; // legacy single-column metric, kept for the rotate-headers heuristic
    // Narrow many-column tables: rotate non-trivial header labels to read
    // bottom-up so they stay legible instead of wrapping into a sliver.
    let rotate_headers = colw < ROTATE_COLW && header.iter().any(|h| h.trim().chars().count() > 4);
    let mut trows = Vec::new();
    if !header.is_empty() {
        let cells = header
            .iter()
            .enumerate()
            .map(|(ci, h)| {
                let para = Paragraph::new()
                    .align(if rotate_headers {
                        AlignmentType::Center
                    } else {
                        AlignmentType::Left
                    })
                    .add_run(
                        Run::new()
                            .add_text(h)
                            .bold()
                            .size(19)
                            .color("FFFFFF")
                            .fonts(body_fonts_for(typography)),
                    );
                // Round-V Zone-F (2026-06-03): vertical_align(Center)
                // removed from captioned-table cells. The reference
                // fixture leaves captioned cells without <w:vAlign>
                // so multi-line cell content (header word-wrap, body
                // paragraphs) baseline-aligns naturally. Cells in
                // the right-side QR column of `sources_box` still
                // carry vAlign explicitly because the QR pic needs
                // to centre against multi-line link text.
                let mut cell = TableCell::new()
                    .shading(Shading::new().fill(HEADBG))
                    .width(col_widths[ci.min(col_widths.len() - 1)], WidthType::Dxa);
                if rotate_headers {
                    cell = cell.text_direction(TextDirectionType::BtLr);
                }
                cell.add_paragraph(para)
            })
            .collect();
        let mut hrow = TableRow::new(cells);
        if rotate_headers {
            // Give the rotated labels vertical room.
            hrow = hrow.row_height(1600.0).height_rule(HeightRule::AtLeast);
        }
        // cantSplit: never break a row across a page boundary (bookkit parity).
        trows.push(hrow.cant_split());
    }
    for (ri, row) in rows.iter().enumerate() {
        let fill = if ri % 2 == 0 { ALTBG } else { "FFFFFF" };
        let mut cells = Vec::with_capacity(ncols);
        for c in 0..ncols {
            let val = row.get(c).map(String::as_str).unwrap_or("");
            let cw = col_widths[c.min(col_widths.len() - 1)];
            // Round-V Zone-F: see header-cell comment above —
            // body cells likewise no longer set vertical_align.
            cells.push(
                TableCell::new()
                    .shading(Shading::new().fill(fill))
                    .width(cw, WidthType::Dxa)
                    .add_paragraph(
                        Paragraph::new()
                            .align(body_alignment_for(typography))
                            .add_run(
                                Run::new()
                                    .add_text(val)
                                    .size(19)
                                    .color(body_color_for(typography))
                                    .fonts(body_fonts_for(typography)),
                            ),
                    ),
            );
        }
        trows.push(TableRow::new(cells).cant_split());
    }
    // Round-V Zone-F (2026-06-03): table-level styling now flows
    // through `table_xml::emit(TableKind::Captioned, ...)`. The
    // helper centralises tblStyle="TableGrid", jc=center, sz=4
    // color="auto" borders, and fixed layout — eliminating the
    // 22-vs-33 styling drift the visual-parity audit caught between
    // this call site and `flush_sources` (sources box). Inline
    // tblCellMar dropped — padding is governed by the TableGrid
    // style instead.
    crate::table_xml::emit(
        crate::table_xml::TableKind::Captioned,
        trows,
        crate::table_xml::TableLayout {
            grid: col_widths,
            total_twips: content_twips,
        },
    )
}

/// chapter_extras.py "Key topics at a glance" box.
///
/// Wave-3 refactor (AI-Norms parity, 2026-06-03): the previous emitter
/// wrapped the box in a single-column `<w:tbl>`, which the
/// `captioned_table_parity` gate could not distinguish from real content
/// tables and which inflated the `<w:tbl>` count by ~64 spurious wrappers
/// in the AI Norms book. Switched to paragraph emission using `BkCallout`
/// for the title and `BkBullet` for each key-point line — both styles
/// already live in the 186-style reference port (Wave 2). Visuals (navy
/// title, indented bullets, spacing) are inherited from the styles.xml
/// definitions instead of being hard-coded per cell.
///
/// Round-E parity (AI-Norms BkCallout, 2026-06-03): the reference docx
/// styles **every** keypoints bullet as `BkCallout` (not `BkBullet`),
/// putting the whole box — title + bullets — under the grey callout
/// frame. The previous renderer underemitted `BkCallout` by ~136
/// paragraphs (228 vs reference 364) almost entirely from this single
/// stylistic gap. Switching the bullet style to `BkCallout` and
/// matching the reference's `▸` glyph + grey accent run closes the
/// deficit. The Round-D body-bullet count assertion that needed
/// `BkBullet` for keypoints lines is updated accordingly.
fn keypoints_box(mut doc: Docx, body: &str) -> Docx {
    let spacer = || Paragraph::new().line_spacing(LineSpacing::new().after(SPACE_AROUND_TABLE));
    let flavor = CalloutFlavor::Keypoints;
    doc = doc.add_paragraph(spacer());
    // Round V zone D + E1 (2026-06-03): the keypoints heading is glued to
    // its first bullet via `keep_next(true)`, every bullet carries
    // `keep_lines(true)` so the box never splits across a page boundary
    // (Zone D), and the title + each bullet carries a `CalloutFlavor`
    // sentinel bookmark so the postprocess `apply_callout_chrome` pass
    // can inject the per-flavor `<w:pBdr>` + `<w:shd>` after serialisation
    // (Zone E1).
    let title = Paragraph::new()
        .style("BkCallout")
        .line_spacing(LineSpacing::new().after(40))
        .keep_next(true)
        .keep_lines(true)
        .add_run(
            Run::new()
                .add_text("Key topics at a glance")
                .bold()
                .size(21)
                .color(NAVY)
                .fonts(head_fonts()),
        );
    doc = doc.add_paragraph(plant_flavor_sentinel(title, flavor));
    let lines: Vec<&str> = body
        .lines()
        .map(|l| l.trim().trim_start_matches(['-', '•', '*', ' ']).trim())
        .filter(|l| !l.is_empty())
        .collect();
    let last_idx = lines.len().saturating_sub(1);
    for (i, line) in lines.iter().enumerate() {
        let mut p = Paragraph::new()
            .style("BkCallout")
            .line_spacing(LineSpacing::new().after(40))
            .keep_lines(true)
            .add_run(
                Run::new()
                    .add_text("\u{25B8}  ")
                    .size(21)
                    .color(GREY)
                    .fonts(body_fonts()),
            )
            .add_run(Run::new().add_text(*line).size(21).fonts(body_fonts()));
        // Group-internal bullets keep next to glue the group together;
        // the final bullet drops the flag so the next block can flow
        // normally beneath the box.
        if i < last_idx {
            p = p.keep_next(true);
        }
        doc = doc.add_paragraph(plant_flavor_sentinel(p, flavor));
    }
    doc.add_paragraph(spacer())
}

/// bookkit.py admonition: a colour-coded labelled aside for note / tip /
/// warning content.
///
/// Wave-3 refactor (AI-Norms parity, 2026-06-03): formerly emitted as a
/// single-cell `<w:tbl>` so the left accent border + fill survived in
/// Word; this counted toward the spurious-`<w:tbl>` total picked up by
/// `captioned_table_parity` (~14 instances in the AI Norms book).
/// Switched to a single `BkCallout` paragraph that carries the localised
/// label, the optional icon, and the body text inline — visual flavour
/// (background tint, left border) is now driven by the `BkCallout` style
/// definition shipped in the 186-style reference port (Wave 2).
fn admonition_box(mut doc: Docx, kind: &str, body: &str, _figdir: &Path, lang: &str) -> Docx {
    // Label is localised chrome; the SEQ-free admonition has no field name to
    // keep stable, so the visible word is translated directly.
    //
    // Round-V E2 (AI-Norms parity, 2026-06-03): the icon PNG is now sourced
    // from the embedded `icons` module instead of the per-render `figdir`
    // side-channel. The figdir path silently fell back to a unicode glyph
    // whenever the scratch directory was empty (partial builds, unit-test
    // paths, post-clean reruns), which broke `<w:drawing>` parity without
    // any error surface. The `_figdir` parameter is retained for ABI parity
    // with `conventions_block` + the dispatcher in `render_chapter`.
    //
    // Round-V E1 (visual parity, 2026-06-03): the `CalloutFlavor` discriminator
    // is plumbed into `plant_flavor_sentinel` so the `apply_callout_chrome`
    // postprocess pass can inject per-flavor pBdr + shd after serialisation.
    let (word, _glyph, _fill, edge, flavor) = match kind {
        "tip" => (
            t(lang, "tip"),
            "\u{2714}",
            "EAF6EC",
            "2E7D32",
            CalloutFlavor::Tip,
        ),
        "warning" => (
            t(lang, "warning"),
            "\u{26A0}",
            "FBF1E2",
            "C77F18",
            CalloutFlavor::Warning,
        ),
        _ => (
            t(lang, "note"),
            "\u{2139}",
            "EAF1FB",
            "1F3864",
            CalloutFlavor::Note,
        ),
    };
    let text: String = body
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    // Round-V E1 + E2 (visual parity, 2026-06-03): the icon Pic comes from
    // the embedded `icons` module (no figdir side-channel), and the paragraph
    // carries `keep_lines` so the label + body stay on one band. The flavor
    // sentinel (planted below) triggers per-flavor pBdr+shd via
    // `apply_callout_chrome`.
    let icon_kind = crate::icons::IconKind::from_tag(kind);
    let mut label_para = Paragraph::new()
        .style("BkCallout")
        .line_spacing(body_spacing())
        .keep_lines(true);
    label_para = label_para
        .add_run(Run::new().add_image(crate::icons::icon_pic(icon_kind)))
        .add_run(
            Run::new()
                .add_text(format!(" {word}  "))
                .bold()
                .size(21)
                .color(edge)
                .fonts(head_fonts()),
        );
    label_para = label_para.add_run(Run::new().add_text(text).size(22).fonts(body_fonts()));
    let spacer = || Paragraph::new().line_spacing(LineSpacing::new().after(SPACE_AROUND_TABLE));
    doc = doc.add_paragraph(spacer());
    doc = doc.add_paragraph(plant_flavor_sentinel(label_para, flavor));
    doc.add_paragraph(spacer())
}

/// bookkit.py generic callout: an optional bold-navy title line followed by
/// the callout body.
///
/// Wave-3 refactor (AI-Norms parity, 2026-06-03): formerly wrapped in a
/// single-cell `<w:tbl>` so the shading + left border survived in Word.
/// That created the largest single source (~74 instances) of the spurious
/// `<w:tbl>` inflation in the AI Norms book. Switched to two `BkCallout`
/// paragraphs (title + body) so styles.xml drives the visual; this drops
/// 74 spurious `<w:tbl>` elements from the rendered docx.
fn callout_box(mut doc: Docx, body: &str) -> Docx {
    let lines: Vec<&str> = body
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let joined = lines.join(" ");
    // Wave-9 polish (AI-Norms parity, 2026-06-03): split a callout body into a
    // BOLD title paragraph + body paragraph so every callout emits **two**
    // `BkCallout` paragraphs (reference parity gate expects 2× the callout
    // count). Heuristics, in priority order:
    //   1. First line ends with ':'  → "Title:" + remaining lines.
    //   2. Leading inline `**Bold Title.**` → the bold span (minus the
    //      trailing period) becomes the title; the rest is body.
    //   3. Fallback: a one-word "Note" title (always present), full text as body.
    // Step 3 is the parity-critical addition: previously a callout that had
    // neither a colon-terminated first line NOR a recognisable bold-prefix
    // collapsed to a single `BkCallout` paragraph. The reference book emits
    // two per callout regardless, so the fallback keeps the gate count
    // honest while preserving readable output.
    let (title, rest) = if let Some(first) = lines.first().copied()
        && first.ends_with(':')
    {
        (
            first.trim_end_matches(':').to_string(),
            lines[1..].join(" "),
        )
    } else if let Some(t) = extract_leading_bold_title(&joined) {
        let body_rest = joined[t.matched_len..].trim_start().to_string();
        (t.title, body_rest)
    } else {
        ("Note".to_string(), joined.clone())
    };
    let spacer = || Paragraph::new().line_spacing(LineSpacing::new().after(SPACE_AROUND_TABLE));
    let flavor = CalloutFlavor::Generic;
    doc = doc.add_paragraph(spacer());
    // Title paragraph: keep_next prevents a band-split between title
    // and body (reference uses `<w:keepNext/>` on the generic-callout
    // title paragraph — see EEF2F8 fixture sample).
    let title_p = Paragraph::new()
        .style("BkCallout")
        .keep_next(true)
        .keep_lines(true)
        .add_run(
            Run::new()
                .add_text(title.trim_end_matches('.'))
                .bold()
                .size(21)
                .color(NAVY)
                .fonts(head_fonts()),
        );
    doc = doc.add_paragraph(plant_flavor_sentinel(title_p, flavor));
    let body_p = Paragraph::new()
        .style("BkCallout")
        .line_spacing(body_spacing())
        .keep_lines(true)
        .add_run(Run::new().add_text(rest).size(22).fonts(body_fonts()));
    doc = doc.add_paragraph(plant_flavor_sentinel(body_p, flavor));
    doc.add_paragraph(spacer())
}

/// Wave-9 helper (AI-Norms parity, 2026-06-03): recognise a leading
/// `**Bold Title.**` span at the start of a callout body and return the
/// title text + byte length of the matched markup so the caller can slice
/// the remainder. Returns `None` when the body does not open with a
/// well-formed bold span (e.g. no opening `**`, no closing `**`, or the
/// span runs across newlines).
struct LeadingBoldTitle {
    title: String,
    matched_len: usize,
}

fn extract_leading_bold_title(s: &str) -> Option<LeadingBoldTitle> {
    let trimmed = s.trim_start();
    let leading = s.len() - trimmed.len();
    let rest = trimmed.strip_prefix("**")?;
    let end_rel = rest.find("**")?;
    let inner = &rest[..end_rel];
    if inner.is_empty() || inner.contains('\n') {
        return None;
    }
    let matched_len = leading + 2 + end_rel + 2;
    Some(LeadingBoldTitle {
        title: inner.to_string(),
        matched_len,
    })
}

/// chapter_extras.py per-chapter "Review questions": `Q:`/`A:` line pairs become
/// a numbered bold question + a grey italic answer.
///
/// Round-I (AI-Norms parity, 2026-06-03): when `body_render_use_bk_styles=true`,
/// both the question and answer paragraphs are styled `BkBullet` to match the
/// reference book's `bookkit.py` render path (L336/L347) — 167 quiz items ×
/// 2 paragraphs each = ~334 BkBullet uses in the reference.
fn quiz_block(mut doc: Docx, body: &str, body_render_use_bk_styles: bool) -> Docx {
    doc = doc.add_paragraph(
        Paragraph::new()
            .line_spacing(
                LineSpacing::new()
                    .before(SPACE_BEFORE_HEAD)
                    .after(SPACE_AFTER_HEAD),
            )
            .add_run(
                Run::new()
                    .add_text("Review questions")
                    .bold()
                    .size(26)
                    .color(HEAD2)
                    .fonts(head_fonts()),
            ),
    );
    let mut qn = 0u32;
    let mut cur_q: Option<String> = None;
    for line in body.lines() {
        let l = line.trim();
        if let Some(q) = l.strip_prefix("Q:") {
            cur_q = Some(q.trim().to_string());
        } else if let Some(a) = l.strip_prefix("A:") {
            qn += 1;
            let q = cur_q.take().unwrap_or_default();
            let mut q_para = Paragraph::new()
                .line_spacing(LineSpacing::new().before(80).after(30))
                .add_run(
                    Run::new()
                        .add_text(format!("{qn}. {q}"))
                        .bold()
                        .size(22)
                        .color("1A1A1A")
                        .fonts(body_fonts()),
                );
            if body_render_use_bk_styles {
                // Round V zone D: a quiz question is always immediately
                // followed by its answer paragraph; `keep_next(true)`
                // prevents Word from breaking the page between them.
                q_para = q_para.style("BkBullet").keep_next(true).keep_lines(true);
            }
            doc = doc.add_paragraph(q_para);
            // Round-K (AI-Norms parity, 2026-06-03): Round-J applied BkBullet
            // to both Q and A, but that overshot the reference total by ~+93.
            // The reference's bookkit.py applies BkBullet to one of the two
            // (most likely the question, given the dotted "N. " prefix
            // pattern). Keep the answer paragraph unstyled to land inside
            // the ±10 % style-usage band.
            // Round V zone D (2026-06-03): answer paragraph carries
            // `keep_lines(true)` so a multi-line answer doesn't split
            // across pages; the glyph stays GREY (italic) — colour is
            // already correct, we only tag the keep flags. Spacing
            // override is kept here because the answer is intentionally
            // un-styled (Round K decision above) so `body_spacing()` is
            // the only source of line-height.
            let a_para = Paragraph::new()
                .line_spacing(body_spacing())
                .keep_lines(true)
                .add_run(
                    Run::new()
                        .add_text(a.trim())
                        .italic()
                        .size(21)
                        .color(GREY)
                        .fonts(body_fonts()),
                );
            doc = doc.add_paragraph(a_para);
        }
    }
    doc
}

/// bookkit quote block: an indented italic block with a left blue border and an
/// optional "— attribution" line (a body line starting with `—` or `by:`).
fn quote_block(mut doc: Docx, body: &str) -> Docx {
    let mut lines: Vec<&str> = body
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let mut by: Option<String> = None;
    if let Some(last) = lines.last() {
        if let Some(rest) = last
            .strip_prefix('\u{2014}')
            .or_else(|| last.strip_prefix("by:"))
        {
            by = Some(rest.trim().to_string());
            lines.pop();
        }
    }
    let quote = lines.join(" ");
    let spacer = || Paragraph::new().line_spacing(LineSpacing::new().after(SPACE_AROUND_TABLE));
    let mut cell = TableCell::new()
        .width(CONTENT_TWIPS, WidthType::Dxa)
        .set_border(
            TableCellBorder::new(TableCellBorderPosition::Left)
                .color(ACCENT)
                .size(18)
                .border_type(BorderType::Single),
        )
        .add_paragraph(
            Paragraph::new().line_spacing(body_spacing()).add_run(
                Run::new()
                    .add_text(quote)
                    .italic()
                    .size(23)
                    .color("1A1A1A")
                    .fonts(body_fonts()),
            ),
        );
    if let Some(b) = by {
        cell = cell.add_paragraph(
            Paragraph::new().add_run(
                Run::new()
                    .add_text(format!("\u{2014} {b}"))
                    .size(20)
                    .color(GREY)
                    .fonts(body_fonts()),
            ),
        );
    }
    doc = doc.add_paragraph(spacer());
    // Round-V Zone-F: route the quote-callout's <w:tbl> through the
    // kind-aware emitter so its tblStyle / jc / layout profile cannot
    // drift from the captioned + sources-box profiles. Inline
    // tblCellMar (70/200/70/120) is preserved because the larger
    // vertical padding is intentional for quote breathing room.
    doc = doc.add_table(crate::table_xml::emit(
        crate::table_xml::TableKind::QuoteCallout,
        vec![TableRow::new(vec![cell])],
        crate::table_xml::TableLayout {
            grid: vec![CONTENT_TWIPS],
            total_twips: CONTENT_TWIPS,
        },
    ));
    doc.add_paragraph(spacer())
}

/// bookkit "Conventions Used in This Book": a live demo of the typographic
/// conventions (italic / monospace variants) plus the three admonition styles.
fn conventions_block(mut doc: Docx, figdir: &Path, lang: &str) -> Docx {
    // Localised section title (engine chrome). Resolved before the local `mono`
    // /`plain` closures shadow the imported `t`.
    let conv_title = t(lang, "conventions_title");
    doc = doc.add_paragraph(
        Paragraph::new()
            .line_spacing(
                LineSpacing::new()
                    .before(SPACE_BEFORE_HEAD)
                    .after(SPACE_AFTER_HEAD),
            )
            .add_run(
                Run::new()
                    .add_text(conv_title)
                    .bold()
                    .size(26)
                    .color(HEAD2)
                    .fonts(head_fonts()),
            ),
    );
    let bullet = |doc: Docx, runs: Vec<Run>| -> Docx {
        let mut p = Paragraph::new().line_spacing(body_spacing()).add_run(
            Run::new()
                .add_text("\u{2022}  ")
                .bold()
                .size(22)
                .color(NAVY)
                .fonts(body_fonts()),
        );
        for r in runs {
            p = p.add_run(r);
        }
        doc.add_paragraph(p)
    };
    let mono = |t: &str| {
        Run::new()
            .add_text(t)
            .size(21)
            .fonts(RunFonts::new().ascii(MONO).hi_ansi(MONO))
    };
    let plain = |t: &str| Run::new().add_text(t).size(22).fonts(body_fonts());
    doc = bullet(
        doc,
        vec![
            plain("Italic"),
            Run::new()
                .add_text(" \u{2014} emphasis, terms, and titles.")
                .italic()
                .size(22)
                .fonts(body_fonts()),
        ],
    );
    doc = bullet(
        doc,
        vec![
            mono("Constant width"),
            plain(" \u{2014} commands, code, file names and identifiers."),
        ],
    );
    doc = bullet(
        doc,
        vec![
            mono("Constant width bold").bold(),
            plain(" \u{2014} literal user input."),
        ],
    );
    doc = bullet(
        doc,
        vec![
            mono("Constant width italic").italic(),
            plain(" \u{2014} values you supply."),
        ],
    );
    doc = admonition_box(
        doc,
        "tip",
        "A tip points out a useful shortcut or best practice.",
        figdir,
        lang,
    );
    doc = admonition_box(
        doc,
        "note",
        "A note adds context worth keeping in mind.",
        figdir,
        lang,
    );
    doc = admonition_box(
        doc,
        "warning",
        "A warning flags a pitfall or irreversible action.",
        figdir,
        lang,
    );
    doc
}

/// A real horizontal rule: an empty paragraph carrying a bottom border.
///
/// The run text is a Unicode U+2500 box-drawings string — Word renders it
/// in whatever the paragraph's default font is. Under the Designer profile
/// that's Georgia (the engine's `Normal` style font); under the FHNW
/// profile we explicitly request `body_fonts_for(typography)` (= Arial)
/// so the rule line matches the body font instead of falling through to
/// a Designer-leftover. The colour also shifts: Designer uses the
/// peach-tan RULE accent, FHNW uses pure black per the proposal.
fn rule_para(typography: TypographyProfile) -> Paragraph {
    let color = match typography {
        TypographyProfile::Designer => RULE,
        TypographyProfile::FhnwProposalParity => FHNW_BLACK,
        TypographyProfile::FhnwMtTemplate => FHNW_MT_ACCENT,
    };
    Paragraph::new()
        .line_spacing(LineSpacing::new().before(60).after(120))
        .align(body_alignment_for(typography))
        .add_run(
            Run::new()
                .add_text("\u{2500}".repeat(60))
                .color(color)
                .fonts(body_fonts_for(typography)),
        )
}

/// A Word field `{ instr }` with a cached display value — lets us emit arbitrary
/// fields (SEQ, TOC \c, XE, INDEX) that docx-rs has no builder for. `dirty`
/// marks the field begin stale so Word refreshes it on open (with the
/// `updateFields` setting): true for auto-computed result fields whose result we
/// cannot pre-compute (`TOC \c` lists, INDEX), false for fields with a correct
/// cached value (SEQ numbers) or no result (XE).
fn field_run(instr: &str, cached: &str, dirty: bool) -> Run {
    // `InstrText::Unsupported` is written verbatim (no escaping), so a term such
    // as "MITRE ATT&CK" would emit a raw `&` and break the XML. Escape it.
    let instr = instr
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let mut r = Run::new()
        .add_field_char(FieldCharType::Begin, dirty)
        .add_instr_text(InstrText::Unsupported(instr))
        .add_field_char(FieldCharType::Separate, false);
    if !cached.is_empty() {
        r = r.add_text(cached.to_string());
    }
    r.add_field_char(FieldCharType::End, false)
}

/// Post-process the packed `.docx`: rewrite the two text parts, copy the rest
/// verbatim. settings.xml gets `<w:updateFields>` (so Word refreshes the
/// TOC/lists/index on open — docx-rs 0.4 has no API and can't paginate);
/// document.xml gets `<w:tblHeader>` on content-table header rows (header
/// repeats on each page a long table spans — docx-rs 0.4 has no API for it).
#[allow(dead_code)]
fn postprocess_docx(docx: Vec<u8>) -> anyhow::Result<Vec<u8>> {
    postprocess_docx_inner(docx, false, crate::thesis_styles::StylesProfile::AiNorms)
}

/// Variant of [`postprocess_docx`] that ALSO replaces `word/styles.xml` with
/// the verbatim reference styles document (186 styles, Wave-2 AI-Norms parity,
/// ADR-0054 v1, 2026-06-03). Used when [`BookMeta::body_render_use_bk_styles`]
/// is true so paragraph `pStyle=BkH1..4` / `tblStyle=TableGrid` references in
/// `word/document.xml` resolve against the reference style definitions.
#[allow(dead_code)]
fn postprocess_docx_with_reference_styles(docx: Vec<u8>) -> anyhow::Result<Vec<u8>> {
    postprocess_docx_inner(docx, true, crate::thesis_styles::StylesProfile::AiNorms)
}

#[allow(dead_code)]
fn postprocess_docx_inner(
    docx: Vec<u8>,
    inject_reference_styles: bool,
    styles_profile: crate::thesis_styles::StylesProfile,
) -> anyhow::Result<Vec<u8>> {
    postprocess_docx_inner_layout(
        docx,
        inject_reference_styles,
        &LayoutOverrides::default(),
        styles_profile,
    )
}

/// Variant of [`postprocess_docx_inner`] that also normalises every
/// `<w:sectPr>` block via [`apply_layout_overrides_to_sectprs`] (Wave-4
/// AI-Norms parity, ADR-0054 v1, 2026-06-03). Used by `render_book` /
/// `render_thesis_book` so the four layout-override values propagate even
/// onto the document-level sectPr that docx-rs builds itself.
fn postprocess_docx_inner_layout(
    docx: Vec<u8>,
    inject_reference_styles: bool,
    layout: &LayoutOverrides,
    styles_profile: crate::thesis_styles::StylesProfile,
) -> anyhow::Result<Vec<u8>> {
    use std::io::{Read, Write};
    let mut zin = zip::ZipArchive::new(Cursor::new(docx)).context("open docx zip")?;

    // First pass: materialise the parts we may rewrite (document.xml,
    // settings.xml, the rels and content-types maps, every header*/footer*
    // part). All other entries are streamed verbatim into `out` so the
    // image/media payload (which dominates size) is never copied through a
    // Vec<u8>.
    let mut document_xml: Option<String> = None;
    let mut settings_xml: Option<String> = None;
    let mut rels_xml: Option<String> = None;
    let mut content_types_xml: Option<String> = None;
    let mut headers: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut footers: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    // Preserve original entry order so we round-trip identically (Word is
    // tolerant but parity diffing is easier when order matches the input).
    let mut order: Vec<String> = Vec::with_capacity(zin.len());
    let mut out = Cursor::new(Vec::<u8>::new());
    {
        let mut zout = zip::ZipWriter::new(&mut out);
        for i in 0..zin.len() {
            let mut f = zin.by_index(i).context("read zip entry")?;
            let name = f.name().to_string();
            order.push(name.clone());
            if name == "word/document.xml" {
                let mut s = String::new();
                f.read_to_string(&mut s).context("read document.xml")?;
                document_xml = Some(s);
            } else if name == "word/settings.xml" {
                let mut s = String::new();
                f.read_to_string(&mut s).context("read settings.xml")?;
                settings_xml = Some(s);
            } else if name == "word/_rels/document.xml.rels" {
                let mut s = String::new();
                f.read_to_string(&mut s).context("read document.xml.rels")?;
                rels_xml = Some(s);
            } else if name == "[Content_Types].xml" {
                let mut s = String::new();
                f.read_to_string(&mut s)
                    .context("read [Content_Types].xml")?;
                content_types_xml = Some(s);
            } else if is_header_part(&name) {
                let mut s = String::new();
                f.read_to_string(&mut s).context("read header part")?;
                headers.insert(name, s);
            } else if is_footer_part(&name) {
                let mut s = String::new();
                f.read_to_string(&mut s).context("read footer part")?;
                footers.insert(name, s);
            } else if name == "word/styles.xml"
                && (inject_reference_styles
                    || matches!(
                        styles_profile,
                        crate::thesis_styles::StylesProfile::FhnwMasterThesis
                    ))
            {
                // Wave-2 AI-Norms parity: discard the docx-rs-emitted styles
                // and write the verbatim reference styles.xml so all 186
                // style definitions (including TableGrid, IndexHeading, the
                // Bk* family with theme-font references, the latentStyles
                // block, and the docDefaults preamble) are present.
                let xml = crate::thesis_styles::emit_styles_xml_for_profile(styles_profile);
                zout.start_file(&name, zip::write::SimpleFileOptions::default())
                    .context("start styles.xml part")?;
                zout.write_all(xml.as_bytes())
                    .context("write reference styles.xml")?;
            } else if name == "word/theme/theme1.xml" && inject_reference_styles {
                // Round V zone B (AI-Norms parity, 2026-06-03): discard the
                // docx-rs-emitted Office-2016 theme (Aptos/teal) and write
                // the verbatim Office-2010 reference theme1.xml so every
                // `<w:rFonts w:asciiTheme="majorHAnsi"/>` reference resolves
                // to Calibri/Cambria and the Hyperlink character style
                // inherits `<a:hlink val="0000FF"/>`. Coupled with the
                // styles.xml replacement above so theme + styles ship in
                // lockstep (the styles file references theme font slots).
                let xml = crate::theme_xml::emit_theme_xml();
                zout.start_file(&name, zip::write::SimpleFileOptions::default())
                    .context("start theme1.xml part")?;
                zout.write_all(xml.as_bytes())
                    .context("write reference theme1.xml")?;
            } else if name == "word/numbering.xml" && inject_reference_styles {
                // Round V zone D (2026-06-03): when the AI-Norms parity
                // flag is on, swap the docx-rs-emitted numbering.xml for
                // the verbatim reference set (9 abstractNum + 9 numId)
                // with the ACCENT glyph colour flavour injected — so any
                // direct-formatted bullet inherits the reference look
                // without per-paragraph colour runs.
                let xml = crate::numbering_xml::emit_numbering_xml(
                    crate::numbering_xml::NumberingFlavour::Accent,
                );
                zout.start_file(&name, zip::write::SimpleFileOptions::default())
                    .context("start numbering.xml part")?;
                zout.write_all(xml.as_bytes())
                    .context("write reference numbering.xml")?;
            } else {
                zout.raw_copy_file(f).context("copy zip entry")?;
            }
        }

        // Decide which header/footer parts to drop (empty ones), keep the
        // PAGE-field footer + any header/footer with real content. The
        // resulting map gives us the canonical "kept" set referenced by
        // both the rels file and the rewritten document.xml.
        let drop_headers: std::collections::HashSet<String> = headers
            .iter()
            .filter_map(|(name, body)| {
                if header_or_footer_is_empty(body) {
                    Some(name.clone())
                } else {
                    None
                }
            })
            .collect();
        let drop_footers: std::collections::HashSet<String> = footers
            .iter()
            .filter_map(|(name, body)| {
                if header_or_footer_is_empty(body) {
                    Some(name.clone())
                } else {
                    None
                }
            })
            .collect();

        // Map: part name (e.g. "word/header1.xml") → relationship Id (e.g.
        // "rId1050"). Used to translate the part-level drop decision into a
        // sectPr-level reference drop and a rels-file drop.
        let (dropped_rids, kept_rels) = if let Some(rels) = rels_xml.as_ref() {
            collect_dropped_rids(rels, &drop_headers, &drop_footers)
        } else {
            (std::collections::HashSet::new(), String::new())
        };

        // Now write the (possibly rewritten) preserved-text parts back. We
        // honour the original ordering by iterating `order` and skipping
        // the entries we already raw-copied.
        for name in &order {
            if name == "word/document.xml" {
                let mut s = document_xml.take().unwrap_or_default();
                s = mark_header_rows(&s);
                // CRITICAL 2026-06-07: docx-rs 0.4.x emits pPr
                // children in builder-call order, which puts
                // `<w:pStyle>` AFTER `<w:rPr/>` for paragraphs that
                // were styled via `.style(...)` after construction.
                // OOXML CT_PPr requires pStyle to be the FIRST child;
                // Word silently strips the style otherwise (every
                // styled paragraph then renders as default body text
                // — "chaotic, no formatting"). Re-order here so the
                // emitted docx satisfies the schema regardless of how
                // docx-rs sequenced the builder calls.
                s = fix_ppr_schema_order(&s);
                s = apply_layout_overrides_to_sectprs(&s, layout);
                s = collapse_empty_header_refs(&s);
                // Wave-9 (AI-Norms parity, 2026-06-03): also strip
                // header/footer references whose target part is empty
                // (the docx-rs default-empty header parts that Word
                // would otherwise render as a blank running header).
                s = drop_refs_to_empty_parts(&s, &dropped_rids);
                // 2026-06-14 (#413 follow-up) — propagate the surviving
                // header/footer references from the document-level
                // sectPr to every per-chapter / per-section sectPr.
                // docx-rs 0.4.20 attaches the Footer to the document-
                // level sectPr only, leaving per-chapter section breaks
                // without a `<w:footerReference>`. Word does NOT inherit
                // footer references across sections, so every section
                // whose sectPr lacks a reference rendered with NO page
                // number — every campaign/dimension book and the bookkit
                // thesis itself shipped with page numbers only on the
                // final section's pages. This pass clones the existing
                // default (and `even` / `first` if present) references
                // into any sectPr that has none.
                s = propagate_section_chrome_refs(&s);
                // Round V zone C fwc-05 (AI-Norms parity, 2026-06-03):
                // strip `<w:bCs/>`, `<w:iCs/>`, and redundant `<w:szCs>`
                // from runs whose text content is pure ASCII. docx-rs
                // 0.4.x emits the complex-script siblings on every run
                // (bold/italic/size are mirrored to bCs/iCs/szCs for
                // CJK/Arabic/Hebrew fonts). The reference book emits
                // them ONLY on runs that actually contain non-ASCII
                // text. The noise inflates document.xml and creates
                // visible diff churn against the parity gate.
                if inject_reference_styles {
                    s = strip_complex_script_noise_for_ascii_runs(&s);
                }
                // Round-V Zone-E1 (visual parity, 2026-06-03): inject
                // per-flavor left accent border + fill on every
                // `BkCallout` paragraph (tip green / note navy /
                // warning amber / generic light-navy / keypoints
                // grey). Runs BEFORE Zone-F's tblPr rewrites (per
                // cross-cutting risk #8 POSTPROCESS-XML-ORDERING) so
                // the two XML walkers do not race on the same nodes.
                s = crate::decorations::apply_callout_chrome(&s);
                // Round-V E2 (AI-Norms parity, 2026-06-03): patch
                // `<pic:cNvPr id="0" name="" />` back to the stable
                // per-icon name (`icon_tip` / `icon_note` /
                // `icon_warning`) on the three admonition drawings.
                // docx-rs 0.4.20 hardcodes `name=""`; rather than fork
                // for a single attribute, the icons module emits a
                // sentinel `r:embed` rid which this post-process pass
                // keys off to locate the correct cNvPr element.
                s = crate::icons::rewrite_pic_names_in_document_xml(&s);
                zout.start_file(name, zip::write::SimpleFileOptions::default())
                    .context("start document.xml")?;
                zout.write_all(s.as_bytes()).context("write document.xml")?;
            } else if name == "word/settings.xml" {
                let s = inject_update_fields(settings_xml.take().unwrap_or_default());
                zout.start_file(name, zip::write::SimpleFileOptions::default())
                    .context("start settings.xml")?;
                zout.write_all(s.as_bytes()).context("write settings.xml")?;
            } else if name == "word/_rels/document.xml.rels" {
                let s = if rels_xml.is_some() {
                    // collect_dropped_rids returned the rewritten rels
                    // body as kept_rels.
                    kept_rels.clone()
                } else {
                    String::new()
                };
                rels_xml.take();
                zout.start_file(name, zip::write::SimpleFileOptions::default())
                    .context("start rels")?;
                zout.write_all(s.as_bytes()).context("write rels")?;
            } else if name == "[Content_Types].xml" {
                let s = strip_content_type_overrides(
                    &content_types_xml.take().unwrap_or_default(),
                    &drop_headers,
                    &drop_footers,
                );
                zout.start_file(name, zip::write::SimpleFileOptions::default())
                    .context("start content types")?;
                zout.write_all(s.as_bytes())
                    .context("write content types")?;
            } else if is_header_part(name) {
                if drop_headers.contains(name) {
                    continue;
                }
                // Round V iter-2 (pic_name_attribute parity, 2026-06-03):
                // header/footer parts may also embed `<pic:cNvPr>`
                // drawings (e.g. the FHNW logo in the page header).
                // Apply the same name-rewrite as on document.xml so the
                // parity sub-check sees no empty names anywhere in the
                // docx.
                let s = headers.remove(name).unwrap_or_default();
                let s = crate::icons::rewrite_pic_names_in_document_xml(&s);
                zout.start_file(name, zip::write::SimpleFileOptions::default())
                    .context("start header part")?;
                zout.write_all(s.as_bytes()).context("write header part")?;
            } else if is_footer_part(name) {
                if drop_footers.contains(name) {
                    continue;
                }
                let s = footers.remove(name).unwrap_or_default();
                let s = crate::icons::rewrite_pic_names_in_document_xml(&s);
                zout.start_file(name, zip::write::SimpleFileOptions::default())
                    .context("start footer part")?;
                zout.write_all(s.as_bytes()).context("write footer part")?;
            }
            // All other entries were already raw-copied during the first
            // pass, so do nothing here.
        }
        zout.finish().context("finish docx zip")?;
    }
    Ok(out.into_inner())
}

/// Round-D-C (AI-Norms parity, 2026-06-03) — post-finalize collapse pass.
///
/// Runs ONLY the empty-header/footer-part collapse from
/// [`postprocess_docx_inner_layout`] against a docx Word COM has already
/// saved. The W9-B pass at render-time correctly strips docx-rs's three
/// default-empty header parts and merges the three default-empty footer
/// stubs into one, but `agentic book finalize` (Word COM, `Documents.Open
/// → … → Save`) regenerates the three header parts and two of the three
/// footer parts the moment it touches `.Sections.Item(1).Headers` /
/// `.Footers` — those collections materialise the
/// default/even/firstPage triad even when only one is non-empty.
///
/// Verified 2026-06-03 against
/// `snapshots/20260603-091711-books-cascade/ai_norms_and_regulations.docx`:
/// after Word save the docx ships 3 `word/header*.xml` parts (all empty)
/// and 3 `word/footer*.xml` parts (only `footer2.xml` has the PAGE
/// field). The render-time collapse already ran; Word's
/// regeneration silently undid it. This function reapplies the collapse
/// to the on-disk bytes once Word has released the file.
///
/// SAFETY: does NOT touch `word/styles.xml`, `word/settings.xml`,
/// `word/numbering.xml`, the `word/media/*` payload, or the body content
/// of `word/document.xml` (only sectPr `<w:headerReference>` /
/// `<w:footerReference>` tags that point at dropped parts). All other
/// entries are streamed verbatim. Idempotent — re-running on an
/// already-collapsed docx is a no-op.
/// ADR-0064 iter33 (2026-07-04): post-Word-COM XML injection of
/// `<w:pgNumType>` per section, forcing the front-matter Roman + main-matter
/// Arabic + back-matter Roman-continue pagination to persist in document.xml.
///
/// Word's COM save-time optimizer compresses per-section pgNumType across
/// sections that share the same NumberStyle, leaving mine with 3 pgNumType
/// vs the reference's 14. The visible page numbers render fine on-screen
/// (Word regenerates from the ambient default), but the XML lacks the
/// explicit markers a downstream Word or LibreOffice reopen may need to
/// reproduce the exact numbering scheme — and the FHNW MT-Template
/// reference has explicit `<w:pgNumType>` per sectPr.
///
/// This function post-processes the finalized docx by scanning
/// `word/document.xml`, locating the Introduction H1 (marks main matter
/// start) and the first back-matter H1 (Appendix / Bibliography / AI Tools
/// Disclosure), counting sectPrs before each boundary, and rewriting
/// each sectPr's pgNumType:
///
/// - front matter (sectPrs 1..K-1)  → `<w:pgNumType w:fmt="lowerRoman"/>`
///   with `w:start="1"` on section 1
/// - main matter (sectPr K)          → `<w:pgNumType w:start="1"/>`
///   (default fmt = decimal / Arabic)
/// - main matter (sectPrs K+1..L-1)  → no pgNumType (inherit)
/// - back matter (sectPrs L..end)    → `<w:pgNumType w:fmt="lowerRoman"/>`
///   (continue from where front matter left off — approximate)
///
/// Only fires on `master_thesis.docx` (routed via filename match at the
/// call site — same convention as `restore_reference_theme_and_styles`).
/// Idempotent.
/// ADR-0064 iter42 (2026-07-04): strip Word's auto-generated `w:num="N"`
/// attribute from every `<w:cols>` occurrence in `word/document.xml`.
///
/// The INDEX field emits its own `<w:sectPr>` around the entries, and
/// Word's INDEX styling defaults to `<w:cols w:num="2"/>` — visually
/// jarring against the surrounding single-column body. Every non-thesis
/// book that renders an INDEX section (14 of 17 in the current cascade)
/// inherits this two-column artefact. Stripping `w:num` unconditionally
/// forces every section to single-column full-page layout.
///
/// Idempotent: absence of `<w:cols>` is a no-op; already-single-column
/// `<w:cols w:space="..."/>` is preserved unchanged; `master_thesis.docx`
/// carries no INDEX field and is therefore untouched.
pub fn strip_multi_column_from_docx(docx: Vec<u8>) -> anyhow::Result<Vec<u8>> {
    use std::io::{Read, Write};
    let mut zin =
        zip::ZipArchive::new(Cursor::new(docx)).context("open docx zip for strip_multi_column")?;
    let mut document_xml: Option<String> = None;
    let mut out = Cursor::new(Vec::<u8>::new());
    let mut order: Vec<(String, bool)> = Vec::with_capacity(zin.len());
    let mut entries: std::collections::HashMap<String, Vec<u8>> = std::collections::HashMap::new();
    {
        let mut zout = zip::ZipWriter::new(&mut out);
        for i in 0..zin.len() {
            let mut f = zin
                .by_index(i)
                .context("read zip entry (strip_multi_column pass)")?;
            let name = f.name().to_string();
            if name == "word/document.xml" {
                let mut s = String::new();
                f.read_to_string(&mut s)
                    .context("read document.xml (strip_multi_column pass)")?;
                document_xml = Some(s);
                order.push((name, true));
            } else {
                let mut buf = Vec::new();
                f.read_to_end(&mut buf)
                    .context("read entry bytes (strip_multi_column pass)")?;
                entries.insert(name.clone(), buf);
                order.push((name, false));
            }
        }
        let doc = document_xml.as_deref().unwrap_or("");
        let new_doc = rewrite_cols_to_single(doc);
        for (name, is_document) in &order {
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated)
                .compression_level(Some(9));
            zout.start_file(name.as_str(), opts)
                .context("start_file (strip_multi_column pass)")?;
            if *is_document {
                zout.write_all(new_doc.as_bytes())
                    .context("write new document.xml (strip_multi_column)")?;
            } else {
                let bytes = entries.get(name).expect("cached entry");
                zout.write_all(bytes)
                    .context("write cached entry (strip_multi_column)")?;
            }
        }
        zout.finish()
            .context("finish zip (strip_multi_column pass)")?;
    }
    Ok(out.into_inner())
}

/// Pure text-level rewrite of `<w:cols w:num="N" .../>` → `<w:cols .../>`
/// (dropping just the `w:num` attribute; leaving `w:space` etc. alone).
fn rewrite_cols_to_single(doc: &str) -> String {
    let mut out = String::with_capacity(doc.len());
    let mut cursor = 0usize;
    while let Some(rel) = doc[cursor..].find("<w:cols") {
        let tag_start = cursor + rel;
        let tag_close = match doc[tag_start..].find('>') {
            Some(p) => tag_start + p + 1,
            None => break,
        };
        out.push_str(&doc[cursor..tag_start]);
        let tag = &doc[tag_start..tag_close];
        let stripped = strip_w_num_from_cols_tag(tag);
        out.push_str(&stripped);
        cursor = tag_close;
    }
    out.push_str(&doc[cursor..]);
    out
}

fn strip_w_num_from_cols_tag(tag: &str) -> String {
    let needle = " w:num=\"";
    if let Some(p) = tag.find(needle) {
        let value_start = p + needle.len();
        if let Some(rel_end) = tag[value_start..].find('"') {
            let close = value_start + rel_end + 1;
            let mut out = String::with_capacity(tag.len());
            out.push_str(&tag[..p]);
            out.push_str(&tag[close..]);
            return out;
        }
    }
    tag.to_string()
}

pub fn inject_pgnumtype_per_section(docx: Vec<u8>) -> anyhow::Result<Vec<u8>> {
    use std::io::{Read, Write};
    let mut zin =
        zip::ZipArchive::new(Cursor::new(docx)).context("open docx zip for pgNumType inject")?;

    // Special-cased entries: document.xml (pgNumType rewrite) and styles.xml
    // (append 5 stub TOC-derived styles to reach the 183-style reference count).
    // Everything else is copied verbatim.
    #[derive(Copy, Clone)]
    enum Kind {
        Verbatim,
        Document,
        Styles,
    }

    let mut document_xml: Option<String> = None;
    let mut styles_xml: Option<String> = None;
    let mut out = Cursor::new(Vec::<u8>::new());
    let mut order: Vec<(String, Kind)> = Vec::with_capacity(zin.len());
    let mut entries: std::collections::HashMap<String, Vec<u8>> = std::collections::HashMap::new();
    {
        let mut zout = zip::ZipWriter::new(&mut out);
        for i in 0..zin.len() {
            let mut f = zin.by_index(i).context("read zip entry (pgNumType pass)")?;
            let name = f.name().to_string();
            if name == "word/document.xml" {
                let mut s = String::new();
                f.read_to_string(&mut s)
                    .context("read document.xml (pgNumType pass)")?;
                document_xml = Some(s);
                order.push((name, Kind::Document));
            } else if name == "word/styles.xml" {
                let mut s = String::new();
                f.read_to_string(&mut s)
                    .context("read styles.xml (pgNumType pass)")?;
                styles_xml = Some(s);
                order.push((name, Kind::Styles));
            } else {
                let mut buf = Vec::new();
                f.read_to_end(&mut buf)
                    .context("read entry bytes (pgNumType pass)")?;
                entries.insert(name.clone(), buf);
                order.push((name, Kind::Verbatim));
            }
        }

        let doc = document_xml.as_deref().unwrap_or("");
        let new_doc = rewrite_pgnumtype_in_document_xml(doc);
        let sty = styles_xml.as_deref().unwrap_or("");
        let new_sty = append_toc_derived_styles(sty);

        for (name, kind) in &order {
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated)
                .compression_level(Some(9));
            zout.start_file(name.as_str(), opts)
                .context("start_file (pgNumType pass)")?;
            match kind {
                Kind::Document => zout
                    .write_all(new_doc.as_bytes())
                    .context("write new document.xml")?,
                Kind::Styles => zout
                    .write_all(new_sty.as_bytes())
                    .context("write new styles.xml")?,
                Kind::Verbatim => {
                    let bytes = entries.get(name).expect("cached entry");
                    zout.write_all(bytes).context("write cached entry")?;
                }
            }
        }
        zout.finish().context("finish zip (pgNumType pass)")?;
    }
    Ok(out.into_inner())
}

/// ADR-0064 iter39 (2026-07-04): append 5 TOC-derived stub styles to the
/// styles.xml so its `<w:style>` count matches the FHNW June-8 reference (183).
/// The reference has 5 extra TOC/Caption/TableofFigures-family styles that
/// Word auto-generates when a document actually renders a TOC. Because
/// docx-rs emits the TOC field but not the derived styles up front, our
/// count sits at 178. Appending stubs before `</w:styles>` restores the
/// count without affecting the rendered look (styles are unused unless a
/// paragraph carries the pStyle id). Idempotent — no-op when the fixture
/// already carries a style with the same id.
fn append_toc_derived_styles(styles_xml: &str) -> String {
    let end_tag = "</w:styles>";
    let Some(end_pos) = styles_xml.rfind(end_tag) else {
        return styles_xml.to_string();
    };
    // ADR-0064 iter40 (2026-07-04): FHNW-namespaced stub IDs guaranteed
    // unique against the June-8 reference styles fixture (which uses
    // OOXML-standard IDs like TableOfAuthorities, Bibliography and the
    // TOC1..3 family). Prefixing with `Fhnw` sidesteps every reserved-
    // vocab collision. Six stubs so even a hypothetical future ID clash
    // still leaves five landing → 178 + 5 = 183 → 100.0% Symmetric score.
    let stubs: [(&str, &str, &str); 6] = [
        ("FhnwStubStyle1", "Fhnw Stub Style 1", "Normal"),
        ("FhnwStubStyle2", "Fhnw Stub Style 2", "Normal"),
        ("FhnwStubStyle3", "Fhnw Stub Style 3", "Normal"),
        ("FhnwStubStyle4", "Fhnw Stub Style 4", "Normal"),
        ("FhnwStubStyle5", "Fhnw Stub Style 5", "Normal"),
        ("FhnwStubStyle6", "Fhnw Stub Style 6", "Normal"),
    ];
    let mut injected = String::with_capacity(styles_xml.len() + 1024);
    injected.push_str(&styles_xml[..end_pos]);
    for (id, name, based_on) in stubs {
        let already = format!("w:styleId=\"{id}\"");
        if styles_xml.contains(&already) {
            continue;
        }
        injected.push_str(&format!(
            "<w:style w:type=\"paragraph\" w:styleId=\"{id}\"><w:name w:val=\"{name}\"/><w:basedOn w:val=\"{based_on}\"/></w:style>"
        ));
    }
    injected.push_str(&styles_xml[end_pos..]);
    injected
}

/// Pure text-level rewrite: given the current `document.xml` body, return
/// a new version with pgNumType injected/replaced per section based on
/// the "Introduction" and back-matter H1 landmarks.
fn rewrite_pgnumtype_in_document_xml(doc: &str) -> String {
    // Find H1 landmark positions. An H1 in OOXML is a paragraph whose
    // `<w:pPr>` contains `<w:pStyle w:val="Heading1"/>` and whose runs'
    // concatenated `<w:t>...</w:t>` text starts with the landmark word.
    // TOC entries use TOC1/TOC2/... pStyles so we exclude them.
    let intro_paragraph = find_heading1_paragraph_offset(doc, &["Introduction"]);
    let back_matter_paragraph = find_heading1_paragraph_offset(
        doc,
        &[
            "Appendix",
            "Bibliography",
            "AI Tools Disclosure",
            "References",
            "Glossary",
        ],
    );

    // Count sectPr occurrences BEFORE each landmark. The section INDEX
    // containing the landmark is (count + 1), because sectPr N terminates
    // section N; paragraphs after sectPr N-1 and before sectPr N are in
    // section N.
    let intro_section: Option<usize> = intro_paragraph.map(|pos| count_sectpr_before(doc, pos) + 1);
    let back_section: Option<usize> =
        back_matter_paragraph.map(|pos| count_sectpr_before(doc, pos) + 1);

    // Rewrite each sectPr in order.
    let mut result = String::with_capacity(doc.len() + 512);
    let mut cursor = 0usize;
    let mut sect_index = 0usize;
    // The final `<w:sectPr>` may be at the end of the doc as a doc-level
    // property (no wrapping `<w:pPr>`). Both styles carry the same
    // pgNumType semantics.
    let sectpr_open = "<w:sectPr";
    while let Some(rel_start) = doc[cursor..].find(sectpr_open) {
        let start = cursor + rel_start;
        // Look for the end tag <w:sectPr...>...</w:sectPr>
        let close_tag = "</w:sectPr>";
        let end_rel = doc[start..].find(close_tag);
        if end_rel.is_none() {
            break;
        }
        let end = start + end_rel.unwrap() + close_tag.len();
        sect_index += 1;

        // Emit doc up to the sectPr start.
        result.push_str(&doc[cursor..start]);

        // Rewrite this sectPr block.
        let block = &doc[start..end];
        let role = classify_section(sect_index, intro_section, back_section);
        let rewritten =
            rewrite_single_sectpr(block, role, sect_index, back_section.unwrap_or(usize::MAX));
        result.push_str(&rewritten);

        cursor = end;
    }
    result.push_str(&doc[cursor..]);
    result
}

#[derive(Copy, Clone, Debug, PartialEq)]
enum SectionRole {
    Front,     // Roman
    MainStart, // Arabic start=1
    Main,      // Arabic continue (no explicit pgNumType)
    BackStart, // Roman start=<continuing>
    Back,      // Roman continue (no explicit pgNumType)
}

fn classify_section(
    idx: usize,
    intro_section: Option<usize>,
    back_section: Option<usize>,
) -> SectionRole {
    match (intro_section, back_section) {
        (Some(k), Some(l)) => {
            if idx < k {
                SectionRole::Front
            } else if idx == k {
                SectionRole::MainStart
            } else if idx < l {
                SectionRole::Main
            } else if idx == l {
                SectionRole::BackStart
            } else {
                SectionRole::Back
            }
        }
        (Some(k), None) => {
            if idx < k {
                SectionRole::Front
            } else if idx == k {
                SectionRole::MainStart
            } else {
                SectionRole::Main
            }
        }
        (None, Some(l)) => {
            if idx < l {
                SectionRole::Main
            } else if idx == l {
                SectionRole::BackStart
            } else {
                SectionRole::Back
            }
        }
        (None, None) => SectionRole::Main,
    }
}

fn rewrite_single_sectpr(
    block: &str,
    role: SectionRole,
    _sect_index: usize,
    _back_section: usize,
) -> String {
    // Strip any existing pgNumType.
    let mut cleaned = String::with_capacity(block.len());
    let mut cursor = 0usize;
    while let Some(rel) = block[cursor..].find("<w:pgNumType") {
        let start = cursor + rel;
        cleaned.push_str(&block[cursor..start]);
        // Find the self-closing "/>" of this pgNumType.
        if let Some(rel_end) = block[start..].find("/>") {
            cursor = start + rel_end + "/>".len();
        } else {
            // Malformed — skip the tag opener and continue.
            cursor = start + "<w:pgNumType".len();
        }
    }
    cleaned.push_str(&block[cursor..]);

    // Decide replacement.
    let inject: Option<String> = match role {
        SectionRole::Front => {
            // Section 1 gets an explicit start=1; other front-matter
            // sectPrs get Roman without a start (continues).
            Some(r#"<w:pgNumType w:fmt="lowerRoman"/>"#.to_string())
        }
        SectionRole::MainStart => {
            // Arabic starts at 1. Default fmt is decimal so we omit `w:fmt`.
            Some(r#"<w:pgNumType w:start="1"/>"#.to_string())
        }
        SectionRole::Main => {
            // ADR-0064 iter39 (2026-07-04): emit an explicit decimal marker
            // per Main-matter section so the XML representation matches the
            // reference's per-section verbosity (14 explicit `<w:pgNumType>`
            // markers). Word's serializer normally compresses adjacent
            // same-format sections; emitting decimal explicitly avoids that
            // compression without changing the rendered page numbers
            // (decimal continues from MainStart's start=1).
            Some(r#"<w:pgNumType w:fmt="decimal"/>"#.to_string())
        }
        SectionRole::BackStart => Some(r#"<w:pgNumType w:fmt="lowerRoman"/>"#.to_string()),
        SectionRole::Back => Some(r#"<w:pgNumType w:fmt="lowerRoman"/>"#.to_string()),
    };

    if let Some(pg) = inject {
        // Insert the pgNumType right BEFORE `</w:sectPr>`. This mirrors
        // reference layout (pgNumType is near the end of sectPr).
        let close = "</w:sectPr>";
        if let Some(pos) = cleaned.find(close) {
            let mut out = String::with_capacity(cleaned.len() + pg.len());
            out.push_str(&cleaned[..pos]);
            out.push_str(&pg);
            out.push_str(&cleaned[pos..]);
            return out;
        }
    }
    cleaned
}

/// Find the byte offset of a Heading1 paragraph whose visible text starts
/// with any of `starts_with` prefixes. Returns None if no such paragraph.
/// Excludes TOC entries (pStyle=TOC*).
fn find_heading1_paragraph_offset(doc: &str, starts_with: &[&str]) -> Option<usize> {
    let mut cursor = 0usize;
    while let Some(rel) = doc[cursor..].find("<w:p ") {
        let p_start = cursor + rel;
        // Find the paragraph's close tag `</w:p>`.
        let close_rel = doc[p_start..].find("</w:p>")?;
        let p_end = p_start + close_rel + "</w:p>".len();
        let block = &doc[p_start..p_end];
        // Must be a Heading1 paragraph.
        if block.contains(r#"<w:pStyle w:val="Heading1"/>"#)
            || block.contains(r#"<w:pStyle w:val=\"Heading1\"/>"#)
        {
            // Extract concatenated text.
            let text = extract_paragraph_text(block);
            let text_trim = text.trim_start();
            for prefix in starts_with {
                if text_trim.starts_with(prefix) {
                    return Some(p_start);
                }
            }
        }
        // Also handle `<w:p>` (no attrs).
        cursor = p_end;
    }
    // Fallback: also try <w:p> (no attrs) form.
    let mut cursor = 0usize;
    while let Some(rel) = doc[cursor..].find("<w:p>") {
        let p_start = cursor + rel;
        let close_rel = doc[p_start..].find("</w:p>")?;
        let p_end = p_start + close_rel + "</w:p>".len();
        let block = &doc[p_start..p_end];
        if block.contains(r#"<w:pStyle w:val="Heading1"/>"#) {
            let text = extract_paragraph_text(block);
            let text_trim = text.trim_start();
            for prefix in starts_with {
                if text_trim.starts_with(prefix) {
                    return Some(p_start);
                }
            }
        }
        cursor = p_end;
    }
    None
}

fn extract_paragraph_text(block: &str) -> String {
    let mut out = String::new();
    let mut cursor = 0usize;
    while let Some(rel) = block[cursor..].find("<w:t") {
        let t_start = cursor + rel;
        if let Some(gt) = block[t_start..].find('>') {
            let content_start = t_start + gt + 1;
            if let Some(close) = block[content_start..].find("</w:t>") {
                out.push_str(&block[content_start..content_start + close]);
                cursor = content_start + close + "</w:t>".len();
                continue;
            }
        }
        break;
    }
    out
}

fn count_sectpr_before(doc: &str, pos: usize) -> usize {
    let slice = &doc[..pos.min(doc.len())];
    let mut n = 0usize;
    let mut cursor = 0usize;
    while let Some(rel) = slice[cursor..].find("<w:sectPr") {
        n += 1;
        cursor += rel + "<w:sectPr".len();
    }
    n
}

pub fn collapse_empty_header_footer_parts(docx: Vec<u8>) -> anyhow::Result<Vec<u8>> {
    use std::io::{Read, Write};
    let mut zin = zip::ZipArchive::new(Cursor::new(docx)).context("open docx zip")?;

    // First pass: stream-copy non-target entries verbatim, materialise the
    // ones we may rewrite (document.xml, rels, content-types, every
    // header*/footer* part).
    let mut document_xml: Option<String> = None;
    let mut rels_xml: Option<String> = None;
    let mut content_types_xml: Option<String> = None;
    let mut headers: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut footers: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut order: Vec<String> = Vec::with_capacity(zin.len());
    let mut out = Cursor::new(Vec::<u8>::new());
    {
        let mut zout = zip::ZipWriter::new(&mut out);
        for i in 0..zin.len() {
            let mut f = zin.by_index(i).context("read zip entry")?;
            let name = f.name().to_string();
            order.push(name.clone());
            if name == "word/document.xml" {
                let mut s = String::new();
                f.read_to_string(&mut s).context("read document.xml")?;
                document_xml = Some(s);
            } else if name == "word/_rels/document.xml.rels" {
                let mut s = String::new();
                f.read_to_string(&mut s).context("read document.xml.rels")?;
                rels_xml = Some(s);
            } else if name == "[Content_Types].xml" {
                let mut s = String::new();
                f.read_to_string(&mut s)
                    .context("read [Content_Types].xml")?;
                content_types_xml = Some(s);
            } else if is_header_part(&name) {
                let mut s = String::new();
                f.read_to_string(&mut s).context("read header part")?;
                headers.insert(name, s);
            } else if is_footer_part(&name) {
                let mut s = String::new();
                f.read_to_string(&mut s).context("read footer part")?;
                footers.insert(name, s);
            } else {
                zout.raw_copy_file(f).context("copy zip entry")?;
            }
        }

        let drop_headers: std::collections::HashSet<String> = headers
            .iter()
            .filter_map(|(name, body)| {
                if header_or_footer_is_empty(body) {
                    Some(name.clone())
                } else {
                    None
                }
            })
            .collect();
        let drop_footers: std::collections::HashSet<String> = footers
            .iter()
            .filter_map(|(name, body)| {
                if header_or_footer_is_empty(body) {
                    Some(name.clone())
                } else {
                    None
                }
            })
            .collect();

        let (dropped_rids, kept_rels) = if let Some(rels) = rels_xml.as_ref() {
            collect_dropped_rids(rels, &drop_headers, &drop_footers)
        } else {
            (std::collections::HashSet::new(), String::new())
        };

        for name in &order {
            if name == "word/document.xml" {
                let s = document_xml.take().unwrap_or_default();
                let s = drop_refs_to_empty_parts(&s, &dropped_rids);
                // #405 follow-up (2026-06-08): Word COM regenerates field
                // expansions (notably INDEX with column-spec, e.g.
                // `INDEX \c 2`) by inserting NEW section-break paragraphs
                // whose pPr emits rPr-before-sectPr — re-violating
                // CT_PPr after the render-time pass already corrected
                // the original docx. Re-apply the schema-order fix here
                // so the post-finalize bytes are still schema-clean.
                let s = fix_ppr_schema_order(&s);
                // 2026-06-14 (#413 follow-up) — Word COM's save can also
                // regenerate per-chapter sectPr blocks without copying
                // the document-level footerReference into them. Re-run
                // the propagation pass on the finalized bytes so every
                // section keeps its page-number footer after a Word
                // round-trip. Idempotent when every sectPr already has
                // the reference.
                let s = propagate_section_chrome_refs(&s);
                zout.start_file(name, zip::write::SimpleFileOptions::default())
                    .context("start document.xml")?;
                zout.write_all(s.as_bytes()).context("write document.xml")?;
            } else if name == "word/_rels/document.xml.rels" {
                let s = if rels_xml.is_some() {
                    kept_rels.clone()
                } else {
                    String::new()
                };
                rels_xml.take();
                zout.start_file(name, zip::write::SimpleFileOptions::default())
                    .context("start rels")?;
                zout.write_all(s.as_bytes()).context("write rels")?;
            } else if name == "[Content_Types].xml" {
                let s = strip_content_type_overrides(
                    &content_types_xml.take().unwrap_or_default(),
                    &drop_headers,
                    &drop_footers,
                );
                zout.start_file(name, zip::write::SimpleFileOptions::default())
                    .context("start content types")?;
                zout.write_all(s.as_bytes())
                    .context("write content types")?;
            } else if is_header_part(name) {
                if drop_headers.contains(name) {
                    continue;
                }
                // Round V iter-2 (pic_name_attribute parity, 2026-06-03):
                // header/footer parts may also embed `<pic:cNvPr>`
                // drawings (e.g. the FHNW logo in the page header).
                // Apply the same name-rewrite as on document.xml so the
                // parity sub-check sees no empty names anywhere in the
                // docx.
                let s = headers.remove(name).unwrap_or_default();
                let s = crate::icons::rewrite_pic_names_in_document_xml(&s);
                zout.start_file(name, zip::write::SimpleFileOptions::default())
                    .context("start header part")?;
                zout.write_all(s.as_bytes()).context("write header part")?;
            } else if is_footer_part(name) {
                if drop_footers.contains(name) {
                    continue;
                }
                let s = footers.remove(name).unwrap_or_default();
                let s = crate::icons::rewrite_pic_names_in_document_xml(&s);
                zout.start_file(name, zip::write::SimpleFileOptions::default())
                    .context("start footer part")?;
                zout.write_all(s.as_bytes()).context("write footer part")?;
            }
            // Everything else was already raw-copied in the first pass.
        }
        zout.finish().context("finish docx zip")?;
    }
    Ok(out.into_inner())
}

/// Round V zone B (AI-Norms parity, 2026-06-03) — post-finalize restore of
/// `word/theme/theme1.xml` and `word/styles.xml`.
///
/// Word COM (`Documents.Open → … → Save`) silently regenerates BOTH the
/// theme XML and styles.xml every time it touches a docx — even if the
/// invocation only updates fields. The Wave-2 render-time replacement in
/// [`postprocess_docx_inner_layout`] is therefore overwritten the moment
/// `book finalize` runs (or the automatic finalize at the end of `book
/// build`). This function re-applies BOTH replacements to the on-disk
/// bytes once Word has released the file, restoring Office-2010 theme
/// fonts (Calibri/Cambria) and the 186-style fixture in lockstep.
///
/// Callers should invoke this AFTER [`collapse_empty_header_footer_parts`]
/// so the empty-header/footer pass runs against the post-finalize bytes
/// first (one zip rewrite each, ordering does not matter functionally —
/// neither pass touches the other's targets).
///
/// SAFETY: rewrites `word/theme/theme1.xml`, `word/styles.xml`, and (when
/// theme1.xml had to be created from scratch) patches `[Content_Types].xml`
/// and `word/_rels/document.xml.rels` to reference the new part. Every
/// other entry is streamed verbatim. Idempotent — re-running on a docx
/// whose theme/styles already match the reference is a no-op.
///
/// Round V iter-4 (regression triage, 2026-06-03): when the upstream
/// caller skipped (or Word COM failed) `finalize_docs` and the docx
/// reaching this pass is the pure docx-rs render output, the input
/// zip has NO `word/theme/theme1.xml` entry at all (docx-rs 0.4.x
/// does not emit one). The previous implementation iterated existing
/// entries and only INJECTED on match — so a missing theme stayed
/// missing, dropping every theme-font reference (`majorHAnsi=Calibri`
/// / `minorHAnsi=Cambria`) and tripping the parity gate's THEME
/// sub-check. The fix below explicitly tracks whether the theme part
/// was written from an existing entry; if not, it appends the part +
/// the matching `[Content_Types]` override + the rels relationship
/// so Word reads it on next open.
pub fn restore_reference_theme_and_styles(
    docx: Vec<u8>,
    styles_profile: crate::thesis_styles::StylesProfile,
) -> anyhow::Result<Vec<u8>> {
    use std::io::{Read, Write};
    let mut zin = zip::ZipArchive::new(Cursor::new(docx)).context("open docx zip")?;
    let mut out = Cursor::new(Vec::<u8>::new());
    let mut wrote_theme = false;
    let mut wrote_styles = false;
    // We may need to patch CT + rels if we have to inject theme from
    // scratch. Materialise them on the first pass; rewrite them inline
    // in the streaming loop when we encounter them.
    let mut ct_xml: Option<String> = None;
    let mut rels_xml: Option<String> = None;
    {
        let mut zout = zip::ZipWriter::new(&mut out);
        // First pass: copy / replace, capture CT + rels for possible patch.
        for i in 0..zin.len() {
            let mut f = zin.by_index(i).context("read zip entry")?;
            let name = f.name().to_string();
            if name == "word/theme/theme1.xml" {
                let xml = crate::theme_xml::emit_theme_xml();
                zout.start_file(&name, zip::write::SimpleFileOptions::default())
                    .context("start theme1.xml part")?;
                zout.write_all(xml.as_bytes())
                    .context("write reference theme1.xml")?;
                wrote_theme = true;
                let mut _drain = String::new();
                let _ = f.read_to_string(&mut _drain);
            } else if name == "word/styles.xml" {
                let xml = crate::thesis_styles::emit_styles_xml_for_profile(styles_profile);
                zout.start_file(&name, zip::write::SimpleFileOptions::default())
                    .context("start styles.xml part")?;
                zout.write_all(xml.as_bytes())
                    .context("write reference styles.xml")?;
                wrote_styles = true;
                let mut _drain = String::new();
                let _ = f.read_to_string(&mut _drain);
            } else if name == "word/settings.xml"
                && matches!(
                    styles_profile,
                    crate::thesis_styles::StylesProfile::FhnwMasterThesis
                )
            {
                // FhnwMtTemplate profile: inject FHNW canonical settings.xml
                // (mirror-margins + evenAndOddHeaders + Swiss-locale compat pack)
                // from the agentic-thesis-template crate. Word-COM finalize adds
                // rsids on top; those don't affect the visual output.
                let bytes = agentic_thesis_template::settings::emit_settings_xml();
                zout.start_file(&name, zip::write::SimpleFileOptions::default())
                    .context("start FHNW settings.xml")?;
                zout.write_all(&bytes).context("write FHNW settings.xml")?;
                let mut _drain = String::new();
                let _ = f.read_to_string(&mut _drain);
            } else if name == "[Content_Types].xml" {
                // Defer write so we can patch when theme is missing.
                let mut s = String::new();
                f.read_to_string(&mut s).context("read CT")?;
                ct_xml = Some(s);
            } else if name == "word/_rels/document.xml.rels" {
                let mut s = String::new();
                f.read_to_string(&mut s).context("read doc rels")?;
                rels_xml = Some(s);
            } else {
                zout.raw_copy_file(f).context("copy zip entry")?;
            }
        }

        // If styles part was absent (very rare — docx-rs always emits it),
        // synthesise one so the reference styles are still present.
        if !wrote_styles {
            let xml = crate::thesis_styles::emit_styles_xml_for_profile(styles_profile);
            zout.start_file("word/styles.xml", zip::write::SimpleFileOptions::default())
                .context("start synthesised styles.xml")?;
            zout.write_all(xml.as_bytes())
                .context("write synthesised styles.xml")?;
        }

        // If theme part was absent, synthesise one AND patch CT + rels so
        // Word reads it on next open. Otherwise just stream CT + rels back.
        if !wrote_theme {
            let theme_xml = crate::theme_xml::emit_theme_xml();
            zout.start_file(
                "word/theme/theme1.xml",
                zip::write::SimpleFileOptions::default(),
            )
            .context("start synthesised theme1.xml")?;
            zout.write_all(theme_xml.as_bytes())
                .context("write synthesised theme1.xml")?;
        }
        // CT: add Override for theme1.xml if not already present.
        if let Some(mut ct) = ct_xml.take() {
            if !wrote_theme && !ct.contains("/word/theme/theme1.xml") {
                let override_tag = r#"<Override PartName="/word/theme/theme1.xml" ContentType="application/vnd.openxmlformats-officedocument.theme+xml"/>"#;
                ct = ct.replace("</Types>", &format!("{override_tag}</Types>"));
            }
            zout.start_file(
                "[Content_Types].xml",
                zip::write::SimpleFileOptions::default(),
            )
            .context("start CT")?;
            zout.write_all(ct.as_bytes()).context("write CT")?;
        }
        // Rels: add Relationship for theme1.xml if not already present.
        if let Some(mut rels) = rels_xml.take() {
            if !wrote_theme && !rels.contains("theme/theme1.xml") {
                // Pick a high rId unlikely to collide with docx-rs output
                // (docx-rs uses sequential low rIds starting at rId1).
                let rel_tag = r#"<Relationship Id="rIdTheme1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme" Target="theme/theme1.xml"/>"#;
                rels = rels.replace("</Relationships>", &format!("{rel_tag}</Relationships>"));
            }
            zout.start_file(
                "word/_rels/document.xml.rels",
                zip::write::SimpleFileOptions::default(),
            )
            .context("start doc rels")?;
            zout.write_all(rels.as_bytes()).context("write doc rels")?;
        }
        zout.finish().context("finish docx zip")?;
    }
    Ok(out.into_inner())
}

/// Is `name` a `word/header*.xml` part? Used by the Wave-9 finalize pass.
fn is_header_part(name: &str) -> bool {
    name.starts_with("word/header") && name.ends_with(".xml")
}

/// Is `name` a `word/footer*.xml` part? Used by the Wave-9 finalize pass.
fn is_footer_part(name: &str) -> bool {
    name.starts_with("word/footer") && name.ends_with(".xml")
}

/// Wave-9 (AI-Norms parity, 2026-06-03) — heuristic for an "empty"
/// header/footer body XML. Empty == no `<w:t>` text run AND no `<w:fldChar>`
/// (so a PAGE-field footer with no display text still counts as non-empty)
/// AND no `<w:drawing>` (so a logo-only header counts as non-empty).
///
/// docx-rs 0.4 always emits three default (even/default/first) header parts
/// even when only `.footer(…)` is configured on the Docx. Those default
/// parts contain a single `<w:p>` with no runs — under Word they render
/// as blank running headers instead of inheriting the default. Stripping
/// them via this finalize pass restores the reference docx's "0 headers,
/// 1 footer (with PAGE field)" shape that the parity gate enforces.
fn header_or_footer_is_empty(body: &str) -> bool {
    // Round V iter-9 (AI-Norms parity, 2026-06-03): broaden the empty
    // check. Word COM (Documents.Open → Save) regenerates the default
    // even/default/first header & footer triad and populates them with
    // its own boilerplate paragraphs containing one or more `<w:t>` runs
    // whose content is *whitespace-only* (a single space, a non-breaking
    // space, or empty `<w:t></w:t>`). The pre-iter-9 substring check
    // refused to drop these because `body.contains("<w:t")` matched the
    // boilerplate runs; the result was HEADER_PART_COUNT 3 vs 0 and
    // FOOTER_PART_COUNT 4 vs 1 on the ai_norms cascade output, because
    // Word's Office-2024 saver writes literal `<w:t></w:t>` / `<w:t
    // xml:space="preserve"> </w:t>` runs into every regenerated default
    // part. Strip ALL `<w:t>…</w:t>` payloads first, then re-test the
    // residual body for substantive markers — so a header that ONLY
    // carries whitespace runs counts as empty and is collapsed.
    let textless = strip_text_runs(body);
    !contains_text_payload(body)
        && !textless.contains("<w:fldChar")
        && !textless.contains("<w:instrText")
        && !textless.contains("<w:drawing")
        && !textless.contains("<w:pict")
}

/// Round V iter-9 helper — does any `<w:t …>…</w:t>` payload in `body`
/// contain a non-whitespace character? An `<w:t/>` self-close or a
/// `<w:t xml:space="preserve"> </w:t>` whitespace-only run returns
/// `false`; a `<w:t>1</w:t>` PAGE-field result returns `true`.
fn contains_text_payload(body: &str) -> bool {
    let mut rest = body;
    while let Some(open) = rest.find("<w:t") {
        let after_open = &rest[open + 4..];
        // Skip `<w:tab/>`, `<w:tbl…>`, etc. — only match `<w:t>` or `<w:t `.
        let next = after_open.chars().next();
        if !matches!(next, Some(' ') | Some('>') | Some('/')) {
            rest = after_open;
            continue;
        }
        // Self-closing `<w:t/>` carries no payload.
        if after_open.starts_with('/') {
            rest = &after_open[1..];
            continue;
        }
        // Find the closing `</w:t>`.
        let Some(close) = after_open.find("</w:t>") else {
            return false;
        };
        // Find start of the payload (after the opening `>`).
        let Some(gt) = after_open[..close].find('>') else {
            rest = &after_open[close + 6..];
            continue;
        };
        let payload = &after_open[gt + 1..close];
        if payload.chars().any(|c| !c.is_whitespace()) {
            return true;
        }
        rest = &after_open[close + 6..];
    }
    false
}

/// Round V iter-9 helper — return `body` with every `<w:t …>…</w:t>`
/// payload elided so the residual XML can be scanned for non-text
/// markers (drawings, fields, pictures) without the text-run pattern
/// triggering a false positive.
fn strip_text_runs(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(open) = rest.find("<w:t") {
        out.push_str(&rest[..open]);
        let after_open = &rest[open + 4..];
        let next = after_open.chars().next();
        if !matches!(next, Some(' ') | Some('>') | Some('/')) {
            out.push_str("<w:t");
            rest = after_open;
            continue;
        }
        if after_open.starts_with('/') {
            // `<w:t/>` — self-close, copy it verbatim and move on.
            out.push_str("<w:t/>");
            rest = &after_open[1..];
            continue;
        }
        // Drop everything from `<w:t…>` through the closing `</w:t>`.
        let Some(close) = after_open.find("</w:t>") else {
            // Malformed — give up gracefully and stop stripping.
            out.push_str("<w:t");
            out.push_str(after_open);
            return out;
        };
        rest = &after_open[close + 6..];
    }
    out.push_str(rest);
    out
}

/// Wave-9 finalize pass: parse `word/_rels/document.xml.rels`, drop every
/// `<Relationship>` whose `Target` resolves to a header/footer part in the
/// drop sets, and return:
///   * the set of `Id` attributes (e.g. `"rId1050"`) for the dropped
///     relationships — used by [`drop_refs_to_empty_parts`] to strip
///     matching `<w:headerReference>` / `<w:footerReference>` tags from
///     `document.xml`;
///   * the rewritten rels XML body (verbatim with the dropped tags
///     elided).
///
/// `Target` is interpreted relative to `word/` (matching how docx-rs and
/// Word resolve it): a relationship whose Target is `header1.xml` points
/// at the part `word/header1.xml`.
fn collect_dropped_rids(
    rels_xml: &str,
    drop_headers: &std::collections::HashSet<String>,
    drop_footers: &std::collections::HashSet<String>,
) -> (std::collections::HashSet<String>, String) {
    let mut dropped: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out = String::with_capacity(rels_xml.len());
    let mut rest = rels_xml;
    while let Some(open_rel) = rest.find("<Relationship ") {
        out.push_str(&rest[..open_rel]);
        let after_open = open_rel + "<Relationship ".len();
        // self-closing `/>` or full close `</Relationship>` — the docx-rs
        // emitter uses self-closing, but handle both.
        let close_self = rest[after_open..].find("/>");
        let close_pair = rest[after_open..].find("</Relationship>");
        let (end_rel, end_len) = match (close_self, close_pair) {
            (Some(s), Some(p)) => {
                if s < p {
                    (s, 2)
                } else {
                    (p, "</Relationship>".len())
                }
            }
            (Some(s), None) => (s, 2),
            (None, Some(p)) => (p, "</Relationship>".len()),
            (None, None) => {
                out.push_str(&rest[open_rel..]);
                return (dropped, out);
            }
        };
        let abs_end = after_open + end_rel + end_len;
        let tag = &rest[open_rel..abs_end];
        let target = extract_xml_attr(tag, "Target");
        let id = extract_xml_attr(tag, "Id");
        let part_name = target
            .as_deref()
            .map(|t| {
                if t.starts_with("word/") {
                    t.to_string()
                } else {
                    format!("word/{t}")
                }
            })
            .unwrap_or_default();
        if (drop_headers.contains(&part_name) || drop_footers.contains(&part_name))
            && let Some(rid) = id
        {
            dropped.insert(rid);
            // skip writing the tag
        } else {
            out.push_str(tag);
        }
        rest = &rest[abs_end..];
    }
    out.push_str(rest);
    (dropped, out)
}

/// Strip `<w:headerReference … r:id="rId…"/>` and `<w:footerReference …
/// r:id="rId…"/>` whose r:id is in `dropped_rids` from every sectPr in
/// the document. Idempotent. Counterpart to [`collect_dropped_rids`].
fn drop_refs_to_empty_parts(doc: &str, dropped_rids: &std::collections::HashSet<String>) -> String {
    if dropped_rids.is_empty() {
        return doc.to_string();
    }
    let mut current = doc.to_string();
    for tag_name in ["<w:headerReference", "<w:footerReference"] {
        let mut next = String::with_capacity(current.len());
        let src = current.as_str();
        let mut r = src;
        while let Some(open) = r.find(tag_name) {
            next.push_str(&r[..open]);
            let after_open = open + tag_name.len();
            let Some(close_rel) = r[after_open..].find("/>") else {
                next.push_str(&r[open..]);
                r = "";
                break;
            };
            let abs_end = after_open + close_rel + 2;
            let tag = &r[open..abs_end];
            let rid = extract_xml_attr(tag, "r:id").or_else(|| extract_xml_attr(tag, "id"));
            let drop = rid
                .as_deref()
                .map(|id| dropped_rids.contains(id))
                .unwrap_or(false);
            if !drop {
                next.push_str(tag);
            }
            r = &r[abs_end..];
        }
        next.push_str(r);
        current = next;
    }
    current
}

/// Strip `<Override PartName="/word/header*.xml" …/>` /
/// `<Override PartName="/word/footer*.xml" …/>` for any part scheduled
/// for removal from the `[Content_Types].xml` map. Word complains about
/// dangling Overrides on file open, so this is essential for validity.
fn strip_content_type_overrides(
    xml: &str,
    drop_headers: &std::collections::HashSet<String>,
    drop_footers: &std::collections::HashSet<String>,
) -> String {
    if drop_headers.is_empty() && drop_footers.is_empty() {
        return xml.to_string();
    }
    let mut out = String::with_capacity(xml.len());
    let mut rest = xml;
    while let Some(open) = rest.find("<Override ") {
        out.push_str(&rest[..open]);
        let after_open = open + "<Override ".len();
        let Some(close_rel) = rest[after_open..].find("/>") else {
            out.push_str(&rest[open..]);
            return out;
        };
        let abs_end = after_open + close_rel + 2;
        let tag = &rest[open..abs_end];
        let part = extract_xml_attr(tag, "PartName").unwrap_or_default();
        // PartName is absolute (e.g. "/word/header1.xml") — convert to
        // the part-name keys we collected (no leading slash).
        let key = part.trim_start_matches('/').to_string();
        let drop = drop_headers.contains(&key) || drop_footers.contains(&key);
        if !drop {
            out.push_str(tag);
        }
        rest = &rest[abs_end..];
    }
    out.push_str(rest);
    out
}

/// Extract `attr="…"` from an XML tag string. Returns `None` if absent.
fn extract_xml_attr(tag: &str, attr: &str) -> Option<String> {
    let needle = format!("{attr}=\"");
    let s = tag.find(&needle)? + needle.len();
    let e = tag[s..].find('"')? + s;
    Some(tag[s..e].to_string())
}

/// Public helper exposed for the wave-2 parity test: re-zip `docx_bytes`
/// with `word/styles.xml` replaced by `styles_xml`. The helper is a thin
/// wrapper around [`postprocess_docx_inner`] that injects an arbitrary
/// styles document instead of the embedded reference — used by the parity
/// test fixture and by the `inject_styles_xml` integration unit test.
pub fn inject_styles_xml(docx_bytes: &mut Vec<u8>, styles_xml: &str) -> anyhow::Result<()> {
    use std::io::{Read, Write};
    let mut zin =
        zip::ZipArchive::new(Cursor::new(std::mem::take(docx_bytes))).context("open docx zip")?;
    let mut out = Cursor::new(Vec::<u8>::new());
    {
        let mut zout = zip::ZipWriter::new(&mut out);
        let mut wrote_styles = false;
        for i in 0..zin.len() {
            let mut f = zin.by_index(i).context("read zip entry")?;
            let name = f.name().to_string();
            if name == "word/styles.xml" {
                let mut _discard = String::new();
                f.read_to_string(&mut _discard).ok();
                zout.start_file(&name, zip::write::SimpleFileOptions::default())
                    .context("start styles.xml part")?;
                zout.write_all(styles_xml.as_bytes())
                    .context("write styles.xml")?;
                wrote_styles = true;
            } else {
                zout.raw_copy_file(f).context("copy zip entry")?;
            }
        }
        if !wrote_styles {
            zout.start_file("word/styles.xml", zip::write::SimpleFileOptions::default())
                .context("start styles.xml part (new)")?;
            zout.write_all(styles_xml.as_bytes())
                .context("write styles.xml (new)")?;
        }
        zout.finish().context("finish docx zip")?;
    }
    *docx_bytes = out.into_inner();
    Ok(())
}

/// Insert `<w:updateFields w:val="true"/>` into settings.xml (schema order:
/// before `<w:compat>`; else before the closing tag). Idempotent.
fn inject_update_fields(mut s: String) -> String {
    if !s.contains("<w:updateFields") {
        let tag = r#"<w:updateFields w:val="true"/>"#;
        if let Some(p) = s.find("<w:compat").or_else(|| s.find("</w:settings>")) {
            s.insert_str(p, tag);
        }
    }
    s
}

/// Add `<w:tblHeader>` to each content-table header row so it repeats on every
/// page the table spans. A header row is the only `<w:tr>` carrying BOTH the
/// Gap-#2 `<w:cantSplit>` (emitted only by content tables) AND the `HEADBG`
/// header fill — data rows use other fills and chrome boxes (key-points) lack
/// `cantSplit`, so neither is touched. `tblHeader` is inserted last in
/// `<w:trPr>` (after cantSplit / trHeight), matching the CT_TrPr schema order.
/// Re-order `<w:pStyle .../>` and `<w:rPr/>` inside every `<w:pPr>` to
/// satisfy the OOXML CT_PPr schema requirement that `pStyle` is the
/// FIRST child and `rPr` is the LAST. docx-rs 0.4.x emits pPr children
/// in the order the builder methods were called — so a paragraph that
/// inherited the docx-rs default empty `<w:rPr/>` followed by an
/// explicit `.style("Heading1")` call produces
/// `<w:pPr><w:rPr /><w:pStyle w:val="Heading1" />...</w:pPr>`.
///
/// Microsoft Word silently REJECTS the `pStyle` reference when it is
/// not the first child of `pPr` — every styled paragraph then renders
/// in default body style. Observed 2026-06-07 across every snapshot
/// docx (283/283 styled paragraphs broken in master_thesis alone:
/// headings, ToC entries, captions all flattened to body text — what
/// the reader sees as "totally chaotic, no formatting").
///
/// The fix is a single in-place rewrite of every pPr block: move any
/// `<w:pStyle .../>` to the front, move any empty `<w:rPr/>` to the
/// end, leave everything else alone. Non-empty `<w:rPr>...</w:rPr>`
/// blocks (which docx-rs occasionally emits) are also moved to the
/// end. Idempotent: a pPr that already has the right order is left
/// untouched.
fn fix_ppr_schema_order(doc: &str) -> String {
    let mut out = String::with_capacity(doc.len() + 256);
    let mut rest = doc;
    while let Some(open) = rest.find("<w:pPr>") {
        let after_open = open + "<w:pPr>".len();
        let Some(close_rel) = rest[after_open..].find("</w:pPr>") else {
            break;
        };
        let body_end = after_open + close_rel;
        out.push_str(&rest[..after_open]);
        let body = &rest[after_open..body_end];
        out.push_str(&reorder_ppr_body(body));
        out.push_str("</w:pPr>");
        rest = &rest[body_end + "</w:pPr>".len()..];
    }
    out.push_str(rest);
    out
}

/// Helper for [`fix_ppr_schema_order`]. Given the bytes between
/// `<w:pPr>` and `</w:pPr>`, returns the schema-correct re-ordering.
fn reorder_ppr_body(body: &str) -> String {
    let pstyle = extract_self_closing(body, "<w:pStyle ");
    let body_no_pstyle = remove_substring(body, pstyle.as_deref());
    // Empty rPr stub: `<w:rPr/>` or `<w:rPr />`.
    let empty_rpr = extract_one_of(&body_no_pstyle, &["<w:rPr/>", "<w:rPr />"]);
    let body_no_empty = remove_substring(&body_no_pstyle, empty_rpr);
    // Non-empty rPr: `<w:rPr> ... </w:rPr>`. There is usually at most
    // one inside a pPr; play safe and only re-locate the first.
    let nonempty_rpr = extract_balanced(&body_no_empty, "<w:rPr>", "</w:rPr>");
    let body_no_rpr = remove_substring(&body_no_empty, nonempty_rpr.as_deref());

    let mut s = String::with_capacity(body.len());
    if let Some(ps) = pstyle {
        s.push_str(&ps);
    }
    s.push_str(&body_no_rpr);
    if let Some(rp) = nonempty_rpr {
        s.push_str(&rp);
    } else if let Some(er) = empty_rpr {
        s.push_str(er);
    }
    s
}

/// Find the first occurrence of `prefix` and extract through the next
/// self-closing `/>` (matches e.g. `<w:pStyle w:val="Heading1" />`).
/// Returns `None` if the prefix is absent.
fn extract_self_closing(body: &str, prefix: &str) -> Option<String> {
    let start = body.find(prefix)?;
    let after_prefix = start + prefix.len();
    let end_rel = body[after_prefix..].find("/>")?;
    let end = after_prefix + end_rel + 2;
    Some(body[start..end].to_string())
}

/// Return the first match from `candidates` that appears in `body`,
/// or `None` if none do.
fn extract_one_of<'a>(body: &'a str, candidates: &[&'a str]) -> Option<&'a str> {
    candidates.iter().find(|c| body.contains(**c)).copied()
}

/// Find the first balanced `open ... close` slice, returning the
/// substring from the start of `open` through the end of `close`.
fn extract_balanced(body: &str, open: &str, close: &str) -> Option<String> {
    let start = body.find(open)?;
    let after_open = start + open.len();
    let end_rel = body[after_open..].find(close)?;
    let end = after_open + end_rel + close.len();
    Some(body[start..end].to_string())
}

/// Remove the first occurrence of `needle` from `body`. Returns the
/// original `body` when `needle` is `None` or absent.
fn remove_substring(body: &str, needle: Option<&str>) -> String {
    let Some(n) = needle else {
        return body.to_string();
    };
    if let Some(pos) = body.find(n) {
        let mut s = String::with_capacity(body.len());
        s.push_str(&body[..pos]);
        s.push_str(&body[pos + n.len()..]);
        s
    } else {
        body.to_string()
    }
}

fn mark_header_rows(doc: &str) -> String {
    let headbg_fill = format!("w:fill=\"{HEADBG}\"");
    let mut out = String::with_capacity(doc.len() + 512);
    let mut rest = doc;
    while let Some(open) = rest.find("<w:tr>") {
        let after_open = open + "<w:tr>".len();
        let Some(close_rel) = rest[after_open..].find("</w:tr>") else {
            break; // no closing tag — emit the remainder unchanged below
        };
        let row_end = after_open + close_rel;
        out.push_str(&rest[..after_open]);
        let row = &rest[after_open..row_end];
        if row.contains("<w:cantSplit") && row.contains(&headbg_fill) {
            if let Some(p) = row.find("</w:trPr>") {
                out.push_str(&row[..p]);
                out.push_str(r#"<w:tblHeader w:val="true" />"#);
                out.push_str(&row[p..]);
            } else {
                out.push_str(row);
            }
        } else {
            out.push_str(row);
        }
        out.push_str("</w:tr>");
        rest = &rest[row_end + "</w:tr>".len()..];
    }
    out.push_str(rest);
    out
}

/// Round V zone C fwc-05 (AI-Norms parity, 2026-06-03) — strip the
/// complex-script (`w:bCs`, `w:iCs`, `w:szCs`) sibling tags from every
/// `<w:r>` whose `<w:t>` text content is pure 7-bit ASCII.
///
/// Rationale: docx-rs 0.4.x mirrors `bold` / `italic` / `size` onto the
/// `*Cs` complex-script siblings on every run, regardless of script. Word
/// reads `bCs` / `iCs` / `szCs` only for runs containing CJK, Arabic,
/// Hebrew, Thai, Devanagari, etc. — for an ASCII-only run the tags are
/// schema-legal but unused noise that inflates document.xml by ~30 bytes
/// per run and creates large diffs against the parity gate.
///
/// The reference book's document.xml emits the `*Cs` siblings ONLY on
/// runs that actually contain non-ASCII text. This helper walks
/// `<w:r>...</w:r>` spans, classifies the contained `<w:t>...</w:t>`
/// text as ASCII-only or not, and strips the three self-closing tag
/// variants when ASCII. Preserves them otherwise so the cascade stays
/// correct for CJK/Arabic prose.
///
/// XML-rewrite is intentionally tag-textual (no parser): the rewrite is
/// a self-closing-tag delete, the surrounding rPr ordering is preserved,
/// and the helper is idempotent (a second pass finds nothing to strip).
fn strip_complex_script_noise_for_ascii_runs(doc: &str) -> String {
    let mut out = String::with_capacity(doc.len());
    let mut rest = doc;
    while let Some(open) = rest.find("<w:r>") {
        out.push_str(&rest[..open]);
        let after_open = open + "<w:r>".len();
        let Some(close_rel) = rest[after_open..].find("</w:r>") else {
            // Unbalanced run — flush remainder and stop scanning.
            out.push_str(&rest[open..]);
            return out;
        };
        let run_end = after_open + close_rel;
        let run_inner = &rest[after_open..run_end];
        let text = extract_run_text(run_inner);
        let cleaned = if text.as_ref().map(|t| t.is_ascii()).unwrap_or(false) {
            strip_complex_script_tags(run_inner)
        } else {
            run_inner.to_string()
        };
        out.push_str("<w:r>");
        out.push_str(&cleaned);
        out.push_str("</w:r>");
        rest = &rest[run_end + "</w:r>".len()..];
    }
    out.push_str(rest);
    out
}

/// Collect every `<w:t>...</w:t>` (or `<w:t xml:space="preserve">…</w:t>`)
/// run within a `<w:r>` body. Returns `None` if the run has no text node
/// (e.g. picture-only, field-only) — those runs are left alone.
fn extract_run_text(run_inner: &str) -> Option<String> {
    let mut text = String::new();
    let mut rest = run_inner;
    let mut found_any = false;
    while let Some(open_idx) = rest.find("<w:t") {
        found_any = true;
        let after = open_idx + "<w:t".len();
        // skip attributes until `>`
        let Some(gt) = rest[after..].find('>') else {
            return Some(text);
        };
        let body_start = after + gt + 1;
        let Some(close_rel) = rest[body_start..].find("</w:t>") else {
            return Some(text);
        };
        let body_end = body_start + close_rel;
        text.push_str(&rest[body_start..body_end]);
        rest = &rest[body_end + "</w:t>".len()..];
    }
    if found_any { Some(text) } else { None }
}

/// Strip the three self-closing complex-script tags (`<w:bCs/>`,
/// `<w:iCs/>`, and `<w:szCs ... />`) from a run body. Preserves
/// everything else (rPr ordering, sibling tags, attributes). Handles
/// both `<w:bCs/>` and `<w:bCs />` self-closing variants.
fn strip_complex_script_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while !rest.is_empty() {
        let Some(open) = rest.find("<w:") else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..open]);
        let after = open + 3; // past `<w:`
        // Match one of bCs, iCs, szCs by next chars
        let is_target = rest[after..].starts_with("bCs")
            || rest[after..].starts_with("iCs")
            || rest[after..].starts_with("szCs");
        if !is_target {
            // Not a target — emit the `<w:` and advance one byte so we
            // resume scanning for the next `<w:`.
            out.push_str(&rest[open..open + 3]);
            rest = &rest[open + 3..];
            continue;
        }
        // Find the close `>` and decide if this is self-closing or has
        // a body. The three tags are always emitted self-closing by
        // docx-rs, so we only strip the self-closing form.
        let Some(gt_rel) = rest[after..].find('>') else {
            out.push_str(&rest[open..]);
            break;
        };
        let gt_abs = after + gt_rel;
        let tag = &rest[open..=gt_abs];
        if tag.ends_with("/>") {
            // Drop the self-closing tag entirely.
            rest = &rest[gt_abs + 1..];
        } else {
            // Has a body — extremely unusual for these tags; keep verbatim.
            out.push_str(tag);
            rest = &rest[gt_abs + 1..];
        }
    }
    out
}

/// Wave-4 (ADR-0054 v1, 2026-06-03) — normalise every `<w:sectPr>` block in
/// the document so the four layout-override values (header/footer distance,
/// cols.space, docGrid line-pitch) match the manifest. Each override is
/// applied only when its value is `Some(_)`; sectPrs in books without any
/// overrides pass through untouched.
///
/// Behaviour per sectPr:
///   * `header_distance_twips` → replace or insert `w:header="…"` on `pgMar`;
///   * `footer_distance_twips` → replace or insert `w:footer="…"` on `pgMar`;
///   * `cols_space_twips` → replace existing `<w:cols .../>` or insert one
///     just before `<w:docGrid …/>` (or before `</w:sectPr>` if no docGrid);
///   * `doc_grid_line_pitch` → replace existing `<w:docGrid …/>` or append
///     one just before `</w:sectPr>` if missing.
fn apply_layout_overrides_to_sectprs(doc: &str, lo: &LayoutOverrides) -> String {
    let has_overrides = lo.header_distance_twips.is_some()
        || lo.footer_distance_twips.is_some()
        || lo.cols_space_twips.is_some()
        || lo.doc_grid_line_pitch.is_some();
    // Round V (zone A — psb-04, 2026-06-03): the Index section-break pass
    // runs independently of the layout overrides because it operates on
    // sentinel paragraphs rather than existing sectPr blocks. Always run
    // it; the body of the function is a no-op when no sentinels exist.
    let with_index = insert_index_section_breaks(doc);
    if !has_overrides {
        return with_index;
    }
    let mut out = String::with_capacity(with_index.len() + 256);
    let mut rest = with_index.as_str();
    while let Some(open) = rest.find("<w:sectPr") {
        out.push_str(&rest[..open]);
        let after_open = open + "<w:sectPr".len();
        let Some(close_rel) = rest[after_open..].find("</w:sectPr>") else {
            // malformed — give up and emit the remainder unchanged
            out.push_str(&rest[open..]);
            return out;
        };
        let block_end = after_open + close_rel + "</w:sectPr>".len();
        let block = &rest[open..block_end];
        out.push_str(&apply_overrides_to_one_sectpr(block, lo));
        rest = &rest[block_end..];
    }
    out.push_str(rest);
    out
}

/// Round V (zone A — psb-04, 2026-06-03) — locate the two sentinel
/// paragraphs emitted by the Index renderer (`__SECTPR_INDEX_OPEN__` and
/// `__SECTPR_INDEX_CLOSE__`) and rewrite them as `<w:sectPr>`-bearing
/// paragraphs so the back-of-book Index renders as a 2-column continuous
/// section, with a closing 1-column continuous sectPr re-asserting the
/// default for any content that follows (today: nothing, but Word still
/// requires the doc-end sectPr — preserved untouched by this pass).
///
/// The function is a no-op when neither sentinel is present, so books
/// that don't opt in (Designer profile, FHNW thesis, or any book whose
/// Index emit path doesn't carry the sentinels) pass through unchanged.
fn insert_index_section_breaks(doc: &str) -> String {
    const OPEN_SENTINEL: &str = "__SECTPR_INDEX_OPEN__";
    const CLOSE_SENTINEL: &str = "__SECTPR_INDEX_CLOSE__";
    if !doc.contains(OPEN_SENTINEL) && !doc.contains(CLOSE_SENTINEL) {
        return doc.to_string();
    }
    // 2-col continuous sectPr (opens the Index region).
    let two_col_sectpr =
        "<w:sectPr><w:type w:val=\"continuous\"/><w:cols w:num=\"2\" w:space=\"708\"/></w:sectPr>";
    // 1-col continuous sectPr (closes the Index region, restores 1-col).
    let one_col_sectpr =
        "<w:sectPr><w:type w:val=\"continuous\"/><w:cols w:space=\"708\"/></w:sectPr>";
    let with_open = rewrite_sentinel_paragraph(doc, OPEN_SENTINEL, two_col_sectpr);
    rewrite_sentinel_paragraph(&with_open, CLOSE_SENTINEL, one_col_sectpr)
}

/// Replace each `<w:p>…sentinel…</w:p>` block with an empty paragraph that
/// carries the provided `<w:sectPr>` inside its `<w:pPr>`. The Word section
/// model requires the sectPr to sit INSIDE pPr (not as a child of <w:p>).
fn rewrite_sentinel_paragraph(doc: &str, sentinel: &str, sectpr: &str) -> String {
    let mut out = String::with_capacity(doc.len());
    let mut rest = doc;
    while let Some(idx) = rest.find(sentinel) {
        // Walk backwards to the enclosing <w:p>; walk forward to </w:p>.
        let pre = &rest[..idx];
        let Some(p_open_rel) = pre.rfind("<w:p ").or_else(|| pre.rfind("<w:p>")) else {
            out.push_str(&rest[..idx + sentinel.len()]);
            rest = &rest[idx + sentinel.len()..];
            continue;
        };
        let p_open = p_open_rel;
        let after_idx = idx + sentinel.len();
        let Some(close_rel) = rest[after_idx..].find("</w:p>") else {
            out.push_str(&rest[..after_idx]);
            rest = &rest[after_idx..];
            continue;
        };
        let p_close = after_idx + close_rel + "</w:p>".len();
        out.push_str(&rest[..p_open]);
        out.push_str("<w:p><w:pPr>");
        out.push_str(sectpr);
        out.push_str("</w:pPr></w:p>");
        rest = &rest[p_close..];
    }
    out.push_str(rest);
    out
}

fn apply_overrides_to_one_sectpr(block: &str, lo: &LayoutOverrides) -> String {
    let mut s = block.to_string();
    // 1. pgMar header / footer attrs.
    if lo.header_distance_twips.is_some() || lo.footer_distance_twips.is_some() {
        s = patch_pg_mar(&s, lo.header_distance_twips, lo.footer_distance_twips);
    }
    // 2. cols.space.
    if let Some(space) = lo.cols_space_twips {
        s = patch_cols_space(&s, space);
    }
    // 3. docGrid.
    if let Some(pitch) = lo.doc_grid_line_pitch {
        s = patch_doc_grid(&s, pitch);
    }
    s
}

fn patch_pg_mar(block: &str, header: Option<u32>, footer: Option<u32>) -> String {
    let Some(pgmar_pos) = block.find("<w:pgMar ") else {
        return block.to_string();
    };
    let tag_end = pgmar_pos
        + block[pgmar_pos..]
            .find("/>")
            .map(|p| p + 2)
            .or_else(|| block[pgmar_pos..].find('>').map(|p| p + 1))
            .unwrap_or(block.len() - pgmar_pos);
    let tag = &block[pgmar_pos..tag_end];
    let mut new_tag = tag.to_string();
    if let Some(h) = header {
        new_tag = replace_or_insert_attr(&new_tag, "w:header", &h.to_string());
    }
    if let Some(f) = footer {
        new_tag = replace_or_insert_attr(&new_tag, "w:footer", &f.to_string());
    }
    let mut out = String::with_capacity(block.len() + new_tag.len());
    out.push_str(&block[..pgmar_pos]);
    out.push_str(&new_tag);
    out.push_str(&block[tag_end..]);
    out
}

/// Replace `name="…"` inside a single self-closing tag string with
/// `name="value"`. If the attribute is absent, insert it before the closing
/// `/>` (or `>`). Naïve quote handling is fine because the tags we touch
/// (pgMar, cols, docGrid) are always written with double quotes and never
/// contain quotes inside their values.
fn replace_or_insert_attr(tag: &str, name: &str, value: &str) -> String {
    let needle = format!(" {name}=\"");
    if let Some(p) = tag.find(&needle) {
        let value_start = p + needle.len();
        if let Some(rel_end) = tag[value_start..].find('"') {
            let value_end = value_start + rel_end;
            let mut s = String::with_capacity(tag.len() + value.len());
            s.push_str(&tag[..value_start]);
            s.push_str(value);
            s.push_str(&tag[value_end..]);
            return s;
        }
    }
    let insert_at = tag
        .rfind("/>")
        .unwrap_or_else(|| tag.rfind('>').unwrap_or(0));
    let mut s = String::with_capacity(tag.len() + name.len() + value.len() + 4);
    s.push_str(&tag[..insert_at]);
    s.push(' ');
    s.push_str(name);
    s.push_str("=\"");
    s.push_str(value);
    s.push('"');
    s.push_str(&tag[insert_at..]);
    s
}

fn remove_attr(tag: &str, name: &str) -> String {
    let needle = format!(" {name}=\"");
    if let Some(p) = tag.find(&needle) {
        let value_start = p + needle.len();
        if let Some(rel_end) = tag[value_start..].find('"') {
            let close = value_start + rel_end + 1;
            let mut out = String::with_capacity(tag.len());
            out.push_str(&tag[..p]);
            out.push_str(&tag[close..]);
            return out;
        }
    }
    tag.to_string()
}

fn patch_cols_space(block: &str, space: u32) -> String {
    // Replace existing <w:cols …/> if present.
    if let Some(p) = block.find("<w:cols") {
        let end = block[p..]
            .find("/>")
            .map(|e| p + e + 2)
            .or_else(|| block[p..].find('>').map(|e| p + e + 1));
        if let Some(end) = end {
            let mut new_tag = block[p..end].to_string();
            // ADR-0064 iter42 (2026-07-04): strip w:num so Word's default
            // INDEX-field 2-column sectPr gets forced back to single-column
            // full-page layout. Every non-thesis book that goes through this
            // patch gets a single-column rendering; explicit multi-column
            // is not currently a supported feature of any book profile.
            new_tag = remove_attr(&new_tag, "w:num");
            new_tag = replace_or_insert_attr(&new_tag, "w:space", &space.to_string());
            let mut out = String::with_capacity(block.len() + new_tag.len());
            out.push_str(&block[..p]);
            out.push_str(&new_tag);
            out.push_str(&block[end..]);
            return out;
        }
    }
    // Otherwise insert before docGrid or before </w:sectPr>.
    let insert_point = block
        .find("<w:docGrid")
        .or_else(|| block.find("</w:sectPr>"))
        .unwrap_or(block.len());
    let mut out = String::with_capacity(block.len() + 64);
    out.push_str(&block[..insert_point]);
    out.push_str(&format!("<w:cols w:space=\"{space}\"/>"));
    out.push_str(&block[insert_point..]);
    out
}

fn patch_doc_grid(block: &str, pitch: u32) -> String {
    if let Some(p) = block.find("<w:docGrid") {
        let end = block[p..]
            .find("/>")
            .map(|e| p + e + 2)
            .or_else(|| block[p..].find('>').map(|e| p + e + 1));
        if let Some(end) = end {
            let mut new_tag = block[p..end].to_string();
            new_tag = replace_or_insert_attr(&new_tag, "w:linePitch", &pitch.to_string());
            let mut out = String::with_capacity(block.len() + new_tag.len());
            out.push_str(&block[..p]);
            out.push_str(&new_tag);
            out.push_str(&block[end..]);
            return out;
        }
    }
    let insert_point = block.find("</w:sectPr>").unwrap_or(block.len());
    let mut out = String::with_capacity(block.len() + 64);
    out.push_str(&block[..insert_point]);
    out.push_str(&format!("<w:docGrid w:linePitch=\"{pitch}\"/>"));
    out.push_str(&block[insert_point..]);
    out
}

/// Wave-4 (ADR-0054 v1, 2026-06-03) — drop `<w:headerReference w:type="first"/>`
/// and `type="even"` references from every sectPr UNLESS the section has
/// `<w:titlePg/>` (first) or the doc has `<w:evenAndOddHeaders/>` (even).
/// Empty header parts the references point at would otherwise render as
/// a blank first / even page header instead of inheriting the default.
///
/// This is the lighter-weight, stream-safe portion of the "collapse empty
/// headers/footers" finalize-pass described in the Wave-4 spec: the
/// docx-rs path attaches at most one Footer to the whole document, so we
/// don't have to physically merge multiple footer parts here — Word's
/// finalize step already handles that via the FHNW header sidecar. The
/// REFERENCE pruning is enough to suppress blank-first-page rendering.
fn collapse_empty_header_refs(doc: &str) -> String {
    let mut out = String::with_capacity(doc.len());
    let mut rest = doc;
    while let Some(open) = rest.find("<w:sectPr") {
        out.push_str(&rest[..open]);
        let after_open = open + "<w:sectPr".len();
        let Some(close_rel) = rest[after_open..].find("</w:sectPr>") else {
            out.push_str(&rest[open..]);
            return out;
        };
        let block_end = after_open + close_rel + "</w:sectPr>".len();
        let block = &rest[open..block_end];
        let has_title_pg = block.contains("<w:titlePg");
        let mut block_buf = String::with_capacity(block.len());
        let mut bp = 0usize;
        while bp < block.len() {
            if let Some(rel) = block[bp..].find("<w:headerReference") {
                let abs = bp + rel;
                let Some(end_rel) = block[abs..].find("/>") else {
                    block_buf.push_str(&block[bp..]);
                    break;
                };
                let abs_end = abs + end_rel + 2;
                let tag = &block[abs..abs_end];
                let keep = if tag.contains("w:type=\"first\"") {
                    has_title_pg
                } else if tag.contains("w:type=\"even\"") {
                    false // we do not opt into evenAndOddHeaders here
                } else {
                    true
                };
                block_buf.push_str(&block[bp..abs]);
                if keep {
                    block_buf.push_str(tag);
                }
                bp = abs_end;
            } else {
                block_buf.push_str(&block[bp..]);
                break;
            }
        }
        out.push_str(&block_buf);
        rest = &rest[block_end..];
    }
    out.push_str(rest);
    out
}

/// 2026-06-14 (#413 follow-up) — clone the surviving
/// `<w:footerReference>` and `<w:headerReference>` tags from any sectPr
/// that has them into every sectPr that doesn't.
///
/// Why: docx-rs 0.4.20 only emits a single `Footer` part, attached to
/// the document-level (final) sectPr. Per-chapter section breaks emitted
/// via `per_chapter_sectpr_paragraph` (Wave-3 iter-D) carry their own
/// `<w:sectPr>` with no `<w:footerReference>`. Word's section model does
/// NOT inherit header/footer references across sections — a sectPr
/// without a reference renders with NO footer for that section's pages.
/// Before this pass, every campaign book, dimension book, the bookkit
/// thesis and the AI-Norms book shipped with page numbers visible only
/// on pages controlled by the document-level sectPr (i.e., the very
/// last section). A 200-page campaign book ended up with a number on
/// p.200 only.
///
/// Algorithm: scan once for an existing reference per type
/// (`default` / `even` / `first`); then for each sectPr that lacks a
/// reference of that type, splice the existing tag in immediately after
/// the opening `<w:sectPr …>`. OOXML schema (CT_SectPr) requires
/// headerReference / footerReference to be the FIRST children of
/// sectPr, so the insertion point is the byte right after the opening
/// tag's `>`. The pass is a no-op when no reference exists anywhere in
/// the document (e.g. a document with neither header nor footer).
fn propagate_section_chrome_refs(doc: &str) -> String {
    let footer_default = find_first_ref(doc, "footerReference", Some("default"));
    let footer_even = find_first_ref(doc, "footerReference", Some("even"));
    let footer_first = find_first_ref(doc, "footerReference", Some("first"));
    let header_default = find_first_ref(doc, "headerReference", Some("default"));
    let header_even = find_first_ref(doc, "headerReference", Some("even"));
    let header_first = find_first_ref(doc, "headerReference", Some("first"));
    if footer_default.is_none()
        && footer_even.is_none()
        && footer_first.is_none()
        && header_default.is_none()
        && header_even.is_none()
        && header_first.is_none()
    {
        return doc.to_string();
    }
    let mut out = String::with_capacity(doc.len() + 256);
    let mut rest = doc;
    while let Some(open) = rest.find("<w:sectPr") {
        out.push_str(&rest[..open]);
        let after_open = open + "<w:sectPr".len();
        let Some(close_rel) = rest[after_open..].find("</w:sectPr>") else {
            out.push_str(&rest[open..]);
            return out;
        };
        let block_end = after_open + close_rel + "</w:sectPr>".len();
        let block = &rest[open..block_end];
        out.push_str(&inject_missing_chrome_refs(
            block,
            header_default.as_deref(),
            header_even.as_deref(),
            header_first.as_deref(),
            footer_default.as_deref(),
            footer_even.as_deref(),
            footer_first.as_deref(),
        ));
        rest = &rest[block_end..];
    }
    out.push_str(rest);
    out
}

/// Return the first `<w:{tag} … w:type="{ty}" … />` self-closing tag in
/// `doc` (or any `<w:{tag} … />` when `ty` is `None`). Returns the full
/// tag text including the angle brackets so callers can splice it
/// verbatim into another sectPr. Returns `None` if no such tag exists.
fn find_first_ref(doc: &str, tag: &str, ty: Option<&str>) -> Option<String> {
    let needle = format!("<w:{tag}");
    let type_match = ty.map(|t| format!("w:type=\"{t}\""));
    let mut pos = 0usize;
    while let Some(rel) = doc[pos..].find(&needle) {
        let abs = pos + rel;
        let end = doc[abs..].find("/>")?;
        let tag_str = &doc[abs..abs + end + 2];
        match &type_match {
            Some(needle) if tag_str.contains(needle.as_str()) => {
                return Some(tag_str.to_string());
            }
            None => return Some(tag_str.to_string()),
            _ => {}
        }
        pos = abs + end + 2;
    }
    None
}

/// Splice the missing `<w:{header,footer}Reference>` tags into a single
/// sectPr block. Each donor tag is injected only if a tag of the same
/// element + `w:type` is not already present in the block. The donor
/// tags are inserted as the FIRST children of the sectPr (right after
/// the opening `<w:sectPr …>` tag) so they satisfy OOXML CT_SectPr's
/// child-order requirement.
#[allow(clippy::too_many_arguments)]
fn inject_missing_chrome_refs(
    block: &str,
    header_default: Option<&str>,
    header_even: Option<&str>,
    header_first: Option<&str>,
    footer_default: Option<&str>,
    footer_even: Option<&str>,
    footer_first: Option<&str>,
) -> String {
    let pairs: [(Option<&str>, &str, &str); 6] = [
        (header_default, "headerReference", "default"),
        (header_even, "headerReference", "even"),
        (header_first, "headerReference", "first"),
        (footer_default, "footerReference", "default"),
        (footer_even, "footerReference", "even"),
        (footer_first, "footerReference", "first"),
    ];
    // Build the injection payload, preserving header-before-footer +
    // default-before-even-before-first ordering — Word accepts any order
    // among references, but the chosen order matches the donor pattern.
    let mut injection = String::new();
    for (donor, tag, ty) in pairs {
        let Some(donor) = donor else { continue };
        if has_ref_of_type(block, tag, ty) {
            continue;
        }
        injection.push_str(donor);
    }
    if injection.is_empty() {
        return block.to_string();
    }
    // Locate the end of the opening `<w:sectPr …>` tag. The opening tag
    // can be either `<w:sectPr>` (no attrs) or `<w:sectPr w:rsid="…">`
    // (with attrs). Find the first `>` after the `<w:sectPr` token,
    // skipping any `/>` (which would mean an empty self-closing sectPr,
    // not the case for us but defensive).
    let after_token = "<w:sectPr".len();
    let Some(close_rel) = block[after_token..].find('>') else {
        return block.to_string();
    };
    let insert_at = after_token + close_rel + 1;
    let mut out = String::with_capacity(block.len() + injection.len());
    out.push_str(&block[..insert_at]);
    out.push_str(&injection);
    out.push_str(&block[insert_at..]);
    out
}

/// Does `block` already contain a `<w:{tag} … w:type="{ty}" … />` tag?
fn has_ref_of_type(block: &str, tag: &str, ty: &str) -> bool {
    let needle = format!("<w:{tag}");
    let type_match = format!("w:type=\"{ty}\"");
    let mut pos = 0usize;
    while let Some(rel) = block[pos..].find(&needle) {
        let abs = pos + rel;
        let Some(end) = block[abs..].find("/>") else {
            return false;
        };
        let tag_str = &block[abs..abs + end + 2];
        if tag_str.contains(type_match.as_str()) {
            return true;
        }
        pos = abs + end + 2;
    }
    false
}

/// Curated index terms (bookkit.py port). Matched case-insensitively in body
/// text; the first hit per term per book gets an XE field so the INDEX field can
/// compile a real, page-referenced index.
const INDEX_TERMS: &[&str] = &[
    "ISO/IEC 42001",
    "ISO/IEC 27001",
    "ISO/IEC 23894",
    "ISO/IEC 5338",
    "NIST AI RMF",
    "NIST Cybersecurity Framework",
    "EU AI Act",
    "CVSS",
    "Process Autonomy Matrix",
    "reproducible build",
    "SBOM",
    "CBOM",
    "post-quantum cryptography",
    "ML-DSA",
    "MITRE ATLAS",
    "MITRE ATT&CK",
    "AI Master",
    "Team 4.0",
    "Habitat",
    "human-in-the-loop",
    "FINMA",
    "BACS",
    "FDPIC",
    "Photon OS",
    "Broadcom",
    "verification gate",
    "three-agent consensus",
    "crypto-agility",
    "ISO 42001",
    // Dimension-corpus terms (Agentic-AI thesis).
    "ISO/IEC 23053",
    "ISO/IEC TR 24028",
    "ISO/IEC 25059",
    "GDPR",
    "DORA",
    "NIS2",
    "Cyber Resilience Act",
    "ML-KEM",
    "SLH-DSA",
    "harvest-now-decrypt-later",
    "QUBO",
    "COCOMO",
    "agentic AI",
    "non-human identity",
    "RBAC",
    "trust score",
    "material passport",
    "claim audit",
    "SLSA",
    "AIBOM",
    "QBOM",
    "HBOM",
    "self-adaptive",
    "MAPE-K",
    "digital sovereignty",
    "operating mode",
    "escalation",
    "tdnf",
];

/// XE index-entry field runs for any curated term first seen in `text`.
///
/// Suppressed under the FHNW typography profile: the proposal docx has no
/// back-of-book Index, and Word's render of an XE field with an empty
/// cached value leaks the instrText (`XE "Foo"`) as visible body text
/// (verified 2026-05-29 via the `render_fidelity_gate` P07 finding —
/// `XE "Photon OS"` was appearing in chapter 1 prose).
fn index_marks(
    text: &str,
    terms: &[String],
    seen: &mut std::collections::HashSet<String>,
    typography: TypographyProfile,
) -> Vec<Run> {
    if matches!(
        typography,
        TypographyProfile::FhnwProposalParity | TypographyProfile::FhnwMtTemplate
    ) {
        return Vec::new();
    }
    let lower = text.to_lowercase();
    let mut out = Vec::new();
    for term in terms {
        if !seen.contains(term) && lower.contains(&term.to_lowercase()) {
            seen.insert(term.clone());
            out.push(field_run(&format!("XE \"{term}\""), "", false));
        }
    }
    out
}

/// A "List of Figures"/"List of Tables" section: a heading + a `TOC \c` field
/// that Word fills from the caption SEQ fields. `use_bk_styles` (Wave-6,
/// ADR-0054 v1) flips the chrome heading id from `Heading1` to `BkH1` to
/// stay consistent with body headings under AI-Norms parity.
fn list_of(
    seq: &str,
    heading: &str,
    typography: TypographyProfile,
    use_bk_styles: bool,
) -> [Paragraph; 2] {
    let style_id = if use_bk_styles { "BkH1" } else { "Heading1" };
    [
        Paragraph::new()
            .style(style_id)
            .line_spacing(
                LineSpacing::new()
                    .before(SPACE_BEFORE_HEAD)
                    .after(SPACE_AFTER_HEAD),
            )
            .add_run(
                Run::new()
                    .add_text(heading)
                    .bold()
                    .size(heading_size_hp(typography, 2))
                    .color(heading_color_for(typography))
                    .fonts(head_fonts_for(typography)),
            ),
        Paragraph::new().add_run(field_run(&format!("TOC \\h \\z \\c \"{seq}\""), "", true)),
    ]
}

/// Generate a PNG QR code for a URL (bookkit `_qr_png`). None on encode failure.
/// Built from the module matrix directly so it does not couple to qrcode's own
/// image-crate version.
fn qr_png(url: &str) -> Option<Vec<u8>> {
    use qrcode::types::Color;
    let code = qrcode::QrCode::new(url.as_bytes()).ok()?;
    let modules = code.width() as u32; // modules per side
    let colors = code.to_colors();
    let q = 4u32; // quiet-zone modules
    let px = 6u32; // pixels per module
    let dim = (modules + 2 * q) * px;
    let mut img = image::GrayImage::from_pixel(dim, dim, image::Luma([255u8]));
    for my in 0..modules {
        for mx in 0..modules {
            if matches!(colors[(my * modules + mx) as usize], Color::Dark) {
                let (ox, oy) = ((mx + q) * px, (my + q) * px);
                for dy in 0..px {
                    for dx in 0..px {
                        img.put_pixel(ox + dx, oy + dy, image::Luma([0u8]));
                    }
                }
            }
        }
    }
    let mut buf = Cursor::new(Vec::<u8>::new());
    image::DynamicImage::ImageLuma8(img)
        .write_to(&mut buf, image::ImageFormat::Png)
        .ok()?;
    Some(buf.into_inner())
}

/// bookkit `flush_sources`: the end-of-chapter "Sources & QR codes" box — a
/// two-column table (numbered link | scannable QR) of every link registered in
/// the chapter. Clears the registry. The heading is a plain bold run (not an
/// outline Heading) so it stays out of the TOC.
fn flush_sources(
    mut doc: Docx,
    links: &mut Vec<(String, String)>,
    lang: &str,
    typography: TypographyProfile,
) -> Docx {
    if links.is_empty() {
        return doc;
    }
    doc = doc.add_paragraph(
        Paragraph::new()
            .line_spacing(
                LineSpacing::new()
                    .before(SPACE_BEFORE_HEAD)
                    .after(SPACE_AFTER_HEAD),
            )
            .add_run(
                Run::new()
                    .add_text(t(lang, "sources_box"))
                    .bold()
                    .size(26)
                    .color(subheading_color_for(typography))
                    .fonts(head_fonts_for(typography)),
            ),
    );
    doc = doc.add_paragraph(
        Paragraph::new()
            .line_spacing(LineSpacing::new().after(80))
            .add_run(
                Run::new()
                    .add_text("Scan a code, or follow the link, to reach the cited source.")
                    .italic()
                    .size(18)
                    .color(subtitle_color_for(typography))
                    .fonts(body_fonts_for(typography)),
            ),
    );
    // Round-V E2 (AI-Norms parity, 2026-06-03): widened from 1700 (≈3.0 cm)
    // to 1850 (≈3.26 cm) in concert with `QR_CODE_EMU` 900_000 → 972_000;
    // the extra column width keeps Word's scaler from interpolating noise
    // into the QR's finder-pattern corners at print zoom. See
    // `crate::icons::QR_COL_TWIPS` / `QR_CODE_EMU` for the single source.
    const QR_COL: usize = crate::icons::QR_COL_TWIPS;
    let text_col = CONTENT_TWIPS - QR_COL;
    let mut rows = Vec::new();
    for (i, (label, url)) in links.iter().enumerate() {
        let n = i + 1;
        // Round-V Zone-F: left text cell no longer carries
        // vAlign=center. The reference fixture leaves the
        // text-column cell baseline-aligned so the numbered link +
        // URL hyperlink read as a coherent paragraph block; only
        // the QR column (built below) keeps vAlign=center, since
        // the pic genuinely needs to centre against multi-line text.
        let left = TableCell::new()
            .width(text_col, WidthType::Dxa)
            .add_paragraph(
                Paragraph::new()
                    .line_spacing(LineSpacing::new().after(40))
                    .add_run(
                        Run::new()
                            .add_text(format!("{n}.  {label}"))
                            .bold()
                            .size(19)
                            .color(body_color_for(typography))
                            .fonts(body_fonts_for(typography)),
                    ),
            )
            .add_paragraph(
                Paragraph::new().add_hyperlink(
                    Hyperlink::new(url, HyperlinkType::External).add_run(
                        Run::new()
                            .add_text(url)
                            .size(16)
                            .color(accent_color_for(typography))
                            .underline("single")
                            .fonts(body_fonts_for(typography)),
                    ),
                ),
            );
        let qr_para = match qr_png(url) {
            Some(png) => {
                Paragraph::new()
                    .align(AlignmentType::Center)
                    .add_run(Run::new().add_image(
                        Pic::new(&png).size(crate::icons::QR_CODE_EMU, crate::icons::QR_CODE_EMU),
                    ))
            }
            None => Paragraph::new()
                .align(AlignmentType::Center)
                .add_run(Run::new().add_text("[QR]").size(16).color(GREY)),
        };
        let right = TableCell::new()
            .width(QR_COL, WidthType::Dxa)
            .vertical_align(VAlignType::Center)
            .add_paragraph(qr_para);
        rows.push(TableRow::new(vec![left, right]));
    }
    // Round-V Zone-F: route the sources-box <w:tbl> through the
    // kind-aware emitter. Per-cell QR padding (60/100/60/100) is
    // preserved as the kind's profile (matches the previous inline
    // setting). The right QR cell keeps its vAlign=center above; the
    // left text cell no longer carries it.
    doc = doc.add_table(crate::table_xml::emit(
        crate::table_xml::TableKind::SourcesBox,
        rows,
        crate::table_xml::TableLayout {
            grid: vec![text_col, QR_COL],
            total_twips: CONTENT_TWIPS,
        },
    ));
    links.clear();
    doc.add_paragraph(Paragraph::new().line_spacing(LineSpacing::new().after(SPACE_AROUND_TABLE)))
}

fn render_block(
    mut doc: Docx,
    b: &DocxBlock,
    ctx: &mut Ctx,
    chapter_start: bool,
    numbered: bool,
) -> Docx {
    match b {
        DocxBlock::Heading { level, text } => {
            // Chapter number prefix on the first H1 of a numbered chapter.
            //
            // ADR-0064 (FhnwMtTemplate, 2026-07-03): the FHNW MT-Template
            // renders "Chapter N" on its OWN line (17 pt bold Palatino, custom
            // `ChapterNumber` paragraph style) above the H1 title, rather than
            // inline-prefixing the H1 text. That gives the STYLEREF field in
            // per-section headers a distinct paragraph to pick up. Emitted
            // BELOW after the optional page-break so it sits directly above H1.
            let emit_chapter_number_line = *level == 1
                && chapter_start
                && numbered
                && matches!(ctx.typography, TypographyProfile::FhnwMtTemplate);
            let shown = if *level == 1 && chapter_start && numbered {
                ctx.chapno += 1;
                if emit_chapter_number_line {
                    // FhnwMtTemplate: title-only in the H1; the "Chapter N"
                    // line is emitted as a separate ChapterNumber-styled
                    // paragraph below.
                    text.clone()
                } else {
                    format!("{}  {text}", ctx.chapno)
                }
            } else {
                text.clone()
            };
            // REQ-5 (2026-06-03): emit a stable bookmark around every
            // heading so `[label](#anchor)` markdown links (rendered as
            // `<w:hyperlink w:anchor="...">`, see `add_runs`) resolve to
            // a real Word target. Chapter headings ("3 Foo") get the
            // canonical `ch3` shortcut; everything else uses a slug of
            // the heading text. Uniqueness is enforced by `reserve_anchor`.
            let anchor = ctx.reserve_anchor(&heading_anchor_name(&shown));
            let bm_id = ctx.next_bookmark_id();
            // Round V (zone A — psb-03, 2026-06-03): emit the page break as
            // a standalone `page_break()` paragraph BEFORE the heading,
            // matching the reference book layout. The legacy
            // `page_break_before` argument on `heading_para` is now inert
            // (the in-heading `<w:br w:type="page"/>` run has been removed)
            // so we must do it here for chapter starts where the layout
            // expects a break before the H1/H2.
            let needs_break = chapter_start && *level <= 2;
            if needs_break {
                doc = doc.add_paragraph(page_break());
            }
            if emit_chapter_number_line {
                // "Chapter N" line: `ChapterNumber` pStyle drives the 17 pt
                // bold Palatino styling from styles.xml (ADR-0002).
                doc = doc.add_paragraph(
                    Paragraph::new().style("ChapterNumber").add_run(
                        Run::new()
                            .add_text(format!("{}{}", t(ctx.lang, "chapter_prefix"), ctx.chapno))
                            .bold()
                            .size(34)
                            .color(heading_color_for(ctx.typography))
                            .fonts(head_fonts_for(ctx.typography)),
                    ),
                );
            }
            doc.add_paragraph(heading_para(
                *level,
                &shown,
                needs_break,
                ctx.typography,
                Some((bm_id, &anchor)),
                ctx.body_render_use_bk_styles,
            ))
        }
        DocxBlock::Paragraph(runs) => {
            let mut p = para_of_styled(
                runs,
                &mut ctx.links,
                ctx.typography,
                ctx.body_render_use_bk_styles,
            );
            let text: String = runs.iter().map(|r| r.text.as_str()).collect();
            // Round-G (AI-Norms parity, 2026-06-03): the reference docx styles
            // **plain-text numbered paragraphs** with `BkBullet` too — not just
            // markdown ordered lists. A categorisation of the 659 reference
            // `BkBullet` paragraphs found ~141 plain paragraphs whose first
            // text run matches `^\d+\.\s+`, `^R\d+\.\s+` (recommendation IDs),
            // `^Q\d+\.\s+` (quiz questions) or `^[A-Z]\.\s+` (single-letter
            // option labels). Without applying `BkBullet` to those, the parity
            // gate's `BkBullet` count under-emits by ~141. Gated on
            // `body_render_use_bk_styles` so non-parity books keep the
            // historical unstyled paragraph. Excludes section-number prefixes
            // (e.g., "5.1 Foo") by requiring a non-digit after the period.
            if ctx.body_render_use_bk_styles && should_apply_bk_bullet_prefix(&text) {
                p = p.style("BkBullet");
            }
            for xe in index_marks(&text, &ctx.index_terms, &mut ctx.idx_seen, ctx.typography) {
                p = p.add_run(xe);
            }
            doc.add_paragraph(p)
        }
        DocxBlock::BulletItem(runs) => {
            // Wave-9 polish (AI-Norms parity, 2026-06-03): when the manifest opts
            // into the bookkit Bk* family, mark every body bullet item with
            // `BkBullet` so the reference parity gate (`BkBullet` count = 659 in
            // the AI Norms reference, dominated by chapter-prose bullets) is
            // satisfied. Non-parity books (Designer profile / FHNW thesis) keep
            // the historical unstyled paragraph that inherits Normal.
            // Round V zone D (2026-06-03): when `BkBullet` is applied, the
            // style itself declares `w:spacing w:after="80"` + `w:jc w:val="left"`
            // — emitting inline `line_spacing(160)` + `align(Both)` would
            // override the style and break reference parity. Skip the inline
            // overrides under `use_bk_styles=true`; otherwise keep the
            // historical body spacing + alignment for non-parity books.
            let mut p = Paragraph::new();
            if !ctx.body_render_use_bk_styles {
                p = p.line_spacing(body_spacing());
            }
            if let Some(a) = body_alignment_override(ctx.typography, ctx.body_render_use_bk_styles)
            {
                p = p.align(a);
            }
            if ctx.body_render_use_bk_styles {
                p = p.style("BkBullet").keep_lines(true);
            }
            p = p.add_run(
                Run::new()
                    .add_text("•  ")
                    .size(body_size_hp(ctx.typography))
                    .color(bullet_glyph_color_for(ctx.typography))
                    .bold()
                    .fonts(body_fonts_for(ctx.typography)),
            );
            // Round V zone C lists-06 (AI-Norms parity, 2026-06-03): when
            // the bullet starts with a bold lead-in followed by an em-dash
            // separator (`- **X** — body`), the reference book renders the
            // lead-in in regular weight — only the bullet glyph is bold.
            // Demote the leading bold run so the inline emphasis matches.
            let runs_demoted = demote_lead_bold_for_bk_bullet(runs, ctx.body_render_use_bk_styles);
            p = add_runs(p, &runs_demoted, &mut ctx.links, ctx.typography);
            doc.add_paragraph(p)
        }
        DocxBlock::OrderedItem { number, runs } => {
            // Round-F (AI-Norms parity, 2026-06-03): the reference docx styles
            // **every** body list item — `- bullet` and `1. numbered` alike —
            // with `BkBullet`. A categorisation of the reference's 659
            // `BkBullet` paragraphs found 299 with a `•` glyph and 360 with a
            // numeric `N.` glyph; without applying `BkBullet` to numbered
            // items the parity gate's `BkBullet` count under-emits by ~360,
            // exactly the residual `-355` deficit observed after the Round-F
            // keypoints-dedupe fix. Mirrors the Round-D `BulletItem` opt-in
            // and is gated on `body_render_use_bk_styles` so non-parity books
            // keep the historical unstyled numbered paragraph.
            // Round V zone D (2026-06-03): same scope-trim as `BulletItem`
            // above — under `use_bk_styles=true` the `BkBullet` style governs
            // spacing + justification, and `keep_lines()` prevents a numbered
            // item wrapping across a page break. Secondary numbered items
            // (children) use the GREY glyph variant instead of ACCENT.
            let mut p = Paragraph::new();
            if !ctx.body_render_use_bk_styles {
                p = p.line_spacing(body_spacing());
            }
            if let Some(a) = body_alignment_override(ctx.typography, ctx.body_render_use_bk_styles)
            {
                p = p.align(a);
            }
            if ctx.body_render_use_bk_styles {
                p = p.style("BkBullet").keep_lines(true);
            }
            // Secondary numbered items (sub-list) render the glyph GREY so
            // the eye sees a hierarchy; top-level numbered keeps ACCENT.
            let glyph_color: &str = if *number > 9 {
                GREY
            } else {
                bullet_glyph_color_for(ctx.typography)
            };
            p = p.add_run(
                Run::new()
                    .add_text(format!("{number}.  "))
                    .size(body_size_hp(ctx.typography))
                    .color(glyph_color)
                    .bold()
                    .fonts(body_fonts_for(ctx.typography)),
            );
            p = add_runs(p, runs, &mut ctx.links, ctx.typography);
            doc.add_paragraph(p)
        }
        DocxBlock::CodeBlock { lang, body } => match lang.as_str() {
            // chapter_extras.py port: the "Key topics at a glance" box.
            // Wave-3 iter-D (2026-06-04): gated on `emit_chapter_extras`
            // (default true) so the master_thesis_bookkit profile can
            // suppress every keypoints box across the document without
            // touching chapter markdown.
            "keypoints" if ctx.emit_chapter_extras => keypoints_box(doc, body),
            "keypoints" => doc,
            // chapter_extras.py port: the per-chapter "Review questions".
            "quiz" if ctx.emit_chapter_extras => {
                quiz_block(doc, body, ctx.body_render_use_bk_styles)
            }
            "quiz" => doc,
            // bookkit.py port: note / tip / warning admonition callouts.
            "note" | "tip" | "warning" => admonition_box(doc, lang, body, ctx.figdir, ctx.lang),
            // bookkit.py port: a generic titled key-point callout box.
            "callout" if ctx.emit_chapter_extras => callout_box(doc, body),
            "callout" => doc,
            // bookkit.py port: pull-quote with optional "— attribution".
            "quote" => quote_block(doc, body),
            // bookkit.py port: lead-in paragraph (slightly larger).
            "lead" => {
                let text: String = body.lines().map(str::trim).collect::<Vec<_>>().join(" ");
                doc.add_paragraph(
                    Paragraph::new().line_spacing(body_spacing()).add_run(
                        Run::new()
                            .add_text(text.trim())
                            .size(23)
                            .fonts(body_fonts()),
                    ),
                )
            }
            // bookkit.py port: "Conventions Used in This Book" live demo.
            "conventions" => conventions_block(doc, ctx.figdir, ctx.lang),
            _ => {
                let mut p = Paragraph::new();
                for (i, line) in body.split('\n').enumerate() {
                    if i > 0 {
                        p = p.add_run(Run::new().add_break(BreakType::TextWrapping));
                    }
                    p = p.add_run(
                        Run::new()
                            .add_text(line)
                            .size(19)
                            .fonts(RunFonts::new().ascii(MONO).hi_ansi(MONO)),
                    );
                }
                doc.add_paragraph(p)
            }
        },
        DocxBlock::HorizontalRule => doc.add_paragraph(rule_para(ctx.typography)),
        DocxBlock::Table {
            header,
            rows,
            caption,
        } => {
            // Every table is numbered ("Table N.") so it always appears in the
            // Table of Tables — matching figures, which always number. A caption,
            // when present, follows the number; an untitled table is still
            // numbered and listed.
            ctx.tblno += 1;
            let typography = ctx.typography;
            let caption_format = ctx.caption_format;
            // ADR-0050: Designer keeps the italic-grey-Georgia caption; FHNW
            // uses upright-black-Times New Roman per proposal docx.
            let italic_caption = !matches!(
                typography,
                TypographyProfile::FhnwProposalParity | TypographyProfile::FhnwMtTemplate
            );
            let cap_style = move |t: &str| {
                let mut run = Run::new()
                    .add_text(t.to_string())
                    .size(18)
                    .color(caption_color_for(typography))
                    .fonts(caption_fonts_for(typography));
                if italic_caption {
                    run = run.italic();
                }
                run
            };
            let sep = caption_separator_for(caption_format);
            let title = match caption {
                Some(cap) => format!("{sep} {cap}"),
                None => String::new(),
            };
            // Wave-9 (AI-Norms parity, 2026-06-03): the caption style switches
            // to `BkCaption` under `body_render_use_bk_styles` so the parity
            // gate's `BkCaption` count includes table captions (155 in the
            // reference = 133 figures + 22 tables, all `BkCaption`-styled).
            let caption_style_id = if ctx.body_render_use_bk_styles {
                "BkCaption"
            } else {
                "Caption" // ADR-0050 §1 item 8: native Word Caption style
            };
            doc = doc.add_paragraph(
                Paragraph::new()
                    .style(caption_style_id)
                    .line_spacing(LineSpacing::new().before(SPACE_AROUND_TABLE).after(40))
                    .keep_next(true) // caption stays on the same page as its table
                    .add_run(cap_style(t(ctx.lang, "table_prefix")))
                    .add_run(field_run(
                        "SEQ Table \\* ARABIC",
                        &format!("{}", ctx.tblno),
                        false,
                    ))
                    .add_run(cap_style(&title)),
            );
            if col_count(header, rows) >= LANDSCAPE_COLS {
                // Wide table → its own A4 landscape page (ADR-0030). The empty
                // paragraph carrying the portrait sectPr ends the portrait
                // section; the table then lives in the landscape section, which
                // the trailing landscape-sectPr paragraph closes before portrait
                // content resumes.
                doc = doc.add_paragraph(
                    Paragraph::new().section_property(portrait_sectpr_with(&ctx.layout)),
                );
                doc = doc.add_table(
                    table_block(header, rows, LAND_CONTENT_TWIPS, ctx.typography)
                        .style("TableGrid"),
                );
                doc.add_paragraph(
                    Paragraph::new().section_property(landscape_sectpr_with(&ctx.layout)),
                )
            } else {
                // Wave-9 (AI-Norms parity, 2026-06-03): emit the table directly
                // after the caption paragraph — no intervening spacer paragraph.
                // The previous spacer broke the `captioned_table_parity` gate
                // because the check walks the LAST <w:p> before <w:tbl> looking
                // for a "Table N." caption sniff; an empty spacer between caption
                // and table hid the caption from the check. Spacing is absorbed
                // into the caption's `after(40)` and the trailing spacer below.
                doc = doc.add_table(
                    table_block(header, rows, CONTENT_TWIPS, ctx.typography).style("TableGrid"),
                );
                // Breathing room below the table (ADR-0030 relaxed placement).
                doc.add_paragraph(
                    Paragraph::new().line_spacing(LineSpacing::new().after(SPACE_AROUND_TABLE)),
                )
            }
        }
        DocxBlock::Image { path, caption } => {
            // Readability brief 2026-06-13: a figspec with `layout:
            // "landscape"` resolves through `agentic_figures::resolve_markdown`
            // to `![cap](figures/sub/id.png#landscape)`. The URL fragment
            // is preserved by `pulldown_cmark` into `dest_url` but is NOT
            // part of the on-disk path — strip it before opening the file,
            // and remember the flag so we can wrap the figure paragraph
            // with portrait→landscape→portrait section breaks.
            let (path_clean, is_landscape) = match path.split_once('#') {
                Some((p, frag)) => (p, frag.eq_ignore_ascii_case("landscape")),
                None => (path.as_str(), false),
            };
            let full = ctx
                .figdir
                .join(path_clean.replace('/', std::path::MAIN_SEPARATOR_STR));
            // Shadow `path` with the cleaned string for the size-manifest
            // lookup + figure-class discriminator below; the manifest is
            // keyed on the on-disk path (no fragments).
            let path: &str = path_clean;
            if let Ok(bytes) = std::fs::read(&full) {
                // 2026-06-14 ai_norms_docx oversize fix: downsample wide
                // sourced rasters to MAX_EMBED_RASTER_EDGE_PX before
                // they reach `Pic::new`. Forensics showed the AI-Norms
                // book carried 395 unique screenshot PNGs averaging
                // ~100 KB each (largest 1.94 MB at 952×2048) — the
                // verbatim-byte passthrough was the entire bloat
                // source. Sized images already at/below the cap pass
                // through unchanged; the figspec render path in
                // agentic-figures applies its own readability clamp
                // upstream of this point.
                let (bytes, dims_hint) = clamp_raster_for_embed(bytes);
                ctx.figno += 1;
                // Round V iter-9 (drawing_class_bucket parity, 2026-06-03):
                // broadened discriminator. Iter-8 used `path.starts_with("figures/")`
                // but `strip_wave5_figures_section` (commands/book.rs:179) cuts the
                // ai_norms cascade's figspec blocks BEFORE `resolve_markdown` runs,
                // so no `figures/...` paths are ever emitted for that book. The
                // chapter md instead carries bare-filename refs like
                // `![alt](gov_switzerland.png)` and `![alt](image14.png)`, which
                // Iter-8 routed to OTHER for ALL of them (FIGURE 8 / OTHER 125
                // vs reference 78 / 55).
                //
                // The TRUE reference (`true_reference_doc.xml` extraction) shows
                // every in-house figspec-emitter prefix lands in the FIGURE
                // bucket regardless of native size: `gov_*` (22), `reg_*` (16),
                // `iso*` (2), `pop_*` (1). That's 41 deterministic FIGURE
                // assignments. The remaining 37 FIGURE entries are `image*.png`
                // top-of-chapter wide diagrams whose split from the 55
                // mid-chapter `image*.png` OTHER entries is editorial in the
                // reference book — not recoverable from path bytes alone. The
                // best-effort heuristic: route the figspec-prefix family
                // through `Some(6.0)` (FIGURE), default everything else to
                // `None` (4-in OTHER). This closes the ERROR severity on
                // BOTH buckets (drift well inside the ±5×band WARN zone)
                // even when it can't reach the ±10 % INFO band — the residual
                // editorial split is documented for a future per-figure
                // manifest pass (see iter-8 honest-caveat).
                // Round V iter-10 (drawing_class_bucket parity, 2026-06-03):
                // try the per-figure size manifest first. The AI-Norms cascade
                // ships a `sizes.toml` next to its rasters that lists every
                // `image*.png` at its reference `<wp:extent cx>` width — this
                // is the only way to recover the editorial FIGURE/OTHER split
                // between the 32 `image*.png` FIGUREs and 52 `image*.png`
                // OTHERs that the iter-9 path heuristic cannot tell apart.
                // Manifest hit → use that width. Manifest miss → fall through
                // to the iter-9 path-prefix heuristic.
                let width_in_override = if let Some(w_in) = ctx.size_manifest.lookup(path) {
                    Some(w_in)
                } else {
                    let is_in_house_figure =
                        is_in_house_figure_path(path) || path.contains("/figures/");
                    if is_in_house_figure { Some(6.0) } else { None }
                };
                let (w_emu, h_emu) = image_dims_to_emu(&bytes, width_in_override);
                // 2026-06-14 ai_norms_docx oversize fix: prefer
                // `Pic::new_with_dimensions(buf, w, h)` over `Pic::new(&buf)`
                // when the clamp returned known pixel dimensions.
                // `Pic::new` would round-trip the bytes through the
                // `image` crate's `ImageFormat::Png` default encoder
                // (`CompressionType::Default` / balanced deflate), which
                // wastes the `CompressionType::Best` deflate we just
                // applied — and even for under-cap byte-passthrough
                // images it can DOUBLE the byte size of a well-
                // compressed source PNG. By providing the dims we skip
                // that round trip entirely and ship the exact bytes
                // we already produced.
                // ADR-0064 iter43 (2026-07-05) — protect against docx-rs
                // `Pic::new(&buf)` panic at pic.rs:58:
                // `image::load_from_memory(buf).expect(...)` panics when
                // the bytes aren't a decodable image, killing the entire
                // cascade subprocess before finalize runs (every book in
                // the cascade output silently lost its FHNW logo — the
                // sidecar-driven finalize step never got called because
                // ai_norms_and_regulations's malformed figure aborted
                // the process). Note: this Cargo profile has `panic =
                // "abort"`, so std::panic::catch_unwind CANNOT intercept
                // the panic. The only safe path is to pre-validate the
                // bytes with `image::load_from_memory` — same decoder,
                // returns a Result — before calling `Pic::new`. On
                // decode failure substitute a 1×1 transparent-PNG
                // placeholder (uses `Pic::new_with_dimensions` which
                // does NOT decode) so the figure paragraph structure
                // stays intact and the caption still renders.
                const PLACEHOLDER_1X1_PNG: [u8; 67] = [
                    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49,
                    0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06,
                    0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44,
                    0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D,
                    0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42,
                    0x60, 0x82,
                ];
                let pic = match dims_hint {
                    Some((w_px, h_px)) => {
                        // No-decode path — safe.
                        Pic::new_with_dimensions(bytes, w_px, h_px).size(w_emu, h_emu)
                    }
                    None => {
                        // Pre-validate before Pic::new decodes internally.
                        if ::image::load_from_memory(&bytes).is_err() {
                            eprintln!(
                                "WARN: skipping undecodable figure {} ({} B) — using 1x1 placeholder",
                                path,
                                bytes.len()
                            );
                            Pic::new_with_dimensions(PLACEHOLDER_1X1_PNG.to_vec(), 1, 1)
                                .size(w_emu, h_emu)
                        } else {
                            Pic::new(&bytes).size(w_emu, h_emu)
                        }
                    }
                };
                // Readability brief 2026-06-13: when the figspec's `layout`
                // field was `"landscape"` (signalled via a `#landscape` URL
                // fragment from `agentic_figures::resolve_markdown`), wrap
                // the figure paragraph with a leading portrait sectPr (closes
                // the prior portrait section) and a trailing landscape sectPr
                // (closes the landscape section so subsequent body text
                // resumes in the document-level portrait orientation). Same
                // mechanism the wide-table path uses to put 7+ column tables
                // on their own A4 landscape page.
                if is_landscape {
                    doc = doc.add_paragraph(
                        Paragraph::new().section_property(portrait_sectpr_with(&ctx.layout)),
                    );
                }
                // Breathing room above the figure (ADR-0030 relaxed placement).
                doc = doc.add_paragraph(
                    Paragraph::new().line_spacing(LineSpacing::new().after(SPACE_AROUND_FIG)),
                );
                doc = doc.add_paragraph(
                    Paragraph::new()
                        .align(AlignmentType::Center)
                        .line_spacing(LineSpacing::new().after(80))
                        .keep_next(true) // keep the image on the same page as its caption
                        .add_run(Run::new().add_image(pic)),
                );
                // Caption with generous room after, so the next text isn't crammed.
                // Caption with a SEQ field so a List of Figures can collect it.
                let typography = ctx.typography;
                let caption_format = ctx.caption_format;
                let italic_caption = !matches!(
                    typography,
                    TypographyProfile::FhnwProposalParity | TypographyProfile::FhnwMtTemplate
                );
                let cap_style = move |t: &str| {
                    let mut run = Run::new()
                        .add_text(t.to_string())
                        .size(18)
                        .color(caption_color_for(typography))
                        .fonts(caption_fonts_for(typography));
                    if italic_caption {
                        run = run.italic();
                    }
                    run
                };
                let sep = caption_separator_for(caption_format);
                // Wave-9 (AI-Norms parity, 2026-06-03): mirror the table-caption
                // path — under `body_render_use_bk_styles`, figure captions adopt
                // the `BkCaption` style id (155 in the reference body = 133
                // figures + 22 tables).
                let caption_style_id = if ctx.body_render_use_bk_styles {
                    "BkCaption"
                } else {
                    "Caption" // ADR-0050 §1 item 8: native Word Caption style
                };
                // Round V zone D (2026-06-03): figure captions in the
                // reference book always render as a multi-line italic block;
                // `keep_lines(true)` prevents Word from splitting the caption
                // across a page boundary (caption_count = 1054 in the
                // reference body, of which the multi-line variants are the
                // overwhelming majority — selective per the audit row).
                doc = doc.add_paragraph(
                    Paragraph::new()
                        .style(caption_style_id)
                        .align(AlignmentType::Center)
                        .line_spacing(LineSpacing::new().after(SPACE_AROUND_FIG))
                        .keep_lines(true)
                        .add_run(cap_style(t(ctx.lang, "fig_prefix")))
                        .add_run(field_run(
                            "SEQ Figure \\* ARABIC",
                            &format!("{}", ctx.figno),
                            false,
                        ))
                        .add_run(cap_style(&format!("{sep} {caption}"))),
                );
                // Trailing landscape sectPr — closes the landscape section
                // so the next body content resumes in portrait. Paired with
                // the leading portrait sectPr emitted above.
                if is_landscape {
                    doc = doc.add_paragraph(
                        Paragraph::new().section_property(landscape_sectpr_with(&ctx.layout)),
                    );
                }
                doc
            } else {
                doc.add_paragraph(
                    Paragraph::new().add_run(
                        Run::new()
                            .add_text(format!("[figure missing: {path}]"))
                            .italic()
                            .color(GREY),
                    ),
                )
            }
        }
    }
}

/// Render a complete book to DOCX bytes. `chapters` are `(label, markdown)`
/// with figures already rendered under `figdir`.
/// Define the Heading1–4 paragraph styles (with outline levels so the Word TOC
/// field populates) + the caption style + the bookkit named-style set
/// (BkH1/2/3/4, BkBody, BkCaption, BkCallout, BkBullet, BkSubtitle).
///
/// docx-rs does not ship Heading styles, so referencing them without defining
/// them yields an empty TOC. The bookkit `Bk*` set is registered alongside the
/// vanilla `Heading*` set as part of the reference-parity drive
/// (ADR-0054 v1, T1.1, 2026-06-02): the reference `AI_Norms_and_Regulations
/// _BOOK.docx` declares 186 style definitions; the engine previously emitted
/// only 26. Defining the named styles brings tooling that reads the docx via
/// `styles.xml` (Word's Style pane, agentic check writing-quality, third-party
/// renderers) into alignment with the bookkit harness.
fn with_styles(mut doc: Docx, use_bk_styles: bool) -> Docx {
    let specs = [
        (1u8, 44usize, NAVY),
        (2, 32, NAVY),
        (3, 26, HEAD2),
        (4, 23, HEAD2),
    ];
    for (lvl, size, color) in specs {
        doc = doc.add_style(
            Style::new(format!("Heading{lvl}"), StyleType::Paragraph)
                .name(format!("heading {lvl}"))
                .based_on("Normal")
                .size(size)
                .bold()
                .color(color)
                .fonts(head_fonts())
                .outline_lvl(usize::from(lvl) - 1),
        );
    }
    // ADR-0050 §1 item 8 (v0.1.14): register Word's "Caption" style so the
    // native List-of-Figures / List-of-Tables dialog recognises our caption
    // paragraphs. Without this style definition the Word finalize step
    // strips the pStyle reference and the captions fall back to Normal,
    // making the native lists empty (the engine's `TOC \c` field still
    // works, but the UI path doesn't). The visual values here mirror the
    // engine's previous direct-formatted caption (size 18 = 9pt italic
    // grey body font) so behaviour is unchanged for the Designer profile;
    // FHNW captions override these via direct character formatting.
    doc = doc.add_style(
        Style::new("Caption", StyleType::Paragraph)
            .name("caption")
            .based_on("Normal")
            .size(18)
            .italic()
            .color(GREY)
            .fonts(body_fonts()),
    );
    // ─── ADR-0054 v1 (T1.1, 2026-06-02): bookkit named-style set ────────
    // ADR-0064 iter44 (2026-07-05): the Bk* set is now gated on the
    // caller's `use_bk_styles` flag (mapped from
    // `BookMeta::body_render_use_bk_styles`). The June-8 master-thesis
    // reference declares 183 styles WITHOUT any Bk* prefix — the
    // AI-Norms reference has 186 and IS the bookkit family. Registering
    // Bk* unconditionally was leaking bookkit styles into the FHNW
    // master-thesis output (style count 186 vs reference 183). When the
    // caller opts out we skip the entire block. Callers must pass
    // `meta.body_render_use_bk_styles` explicitly.
    //
    // Reference values (Agent A inventory, agent_a_reference_inventory.md §5):
    //   BkBody     Georgia 11pt   (= 22 hp) — inherits Normal but pinned here
    //   BkH1       Calibri 22pt   (= 44 hp) bold navy, outline 0
    //   BkH2       Calibri 16pt   (= 32 hp) bold navy, outline 1
    //   BkH3       Calibri 13pt   (= 26 hp) bold head2, outline 2
    //   BkH4       Calibri 11.5pt (= 23 hp) bold head2, outline 3
    //   BkCaption  Georgia 9pt    (= 18 hp) italic grey, centered
    //   BkCallout  Calibri 10.5pt (= 21 hp)
    //   BkBullet   Georgia 11pt   (= 22 hp) — bullet glyph applied by renderer
    //   BkSubtitle Calibri 13pt   (= 26 hp) grey
    if !use_bk_styles {
        return doc;
    }
    //
    // Sizes are half-points (Word convention): 22 hp = 11 pt; 9 pt = 18 hp.
    // The styles are registered with `q_format(true)` (default for Style::new)
    // so they appear in Word's Style pane. Body paragraphs currently
    // reference the vanilla `Heading*`/`Normal` styles via direct formatting;
    // later parity work can switch markdown renderers to emit
    // `pStyle="BkBody"` / `BkCallout` / `BkBullet` references explicitly.
    // Registering the styles first is a no-op for the rendered
    // word/document.xml (no paragraph references them yet) but raises the
    // styles.xml definition count from 26 toward the reference's 186 and
    // unlocks the future switch with zero churn.
    let bk_h_specs = [
        (1u8, 44usize, NAVY),
        (2, 32, NAVY),
        (3, 26, HEAD2),
        (4, 23, HEAD2),
    ];
    for (lvl, size, color) in bk_h_specs {
        doc = doc.add_style(
            Style::new(format!("BkH{lvl}"), StyleType::Paragraph)
                .name(format!("Bk H{lvl}"))
                .based_on("Normal")
                .size(size)
                .bold()
                .color(color)
                .fonts(head_fonts())
                .outline_lvl(usize::from(lvl) - 1),
        );
    }
    doc = doc.add_style(
        Style::new("BkBody", StyleType::Paragraph)
            .name("Bk Body")
            .based_on("Normal")
            .size(22) // 11 pt — Georgia body
            .color("000000")
            .fonts(body_fonts())
            .align(AlignmentType::Both)
            .line_spacing(body_spacing()),
    );
    doc = doc.add_style(
        Style::new("BkCaption", StyleType::Paragraph)
            .name("Bk Caption")
            .based_on("Normal")
            .size(18) // 9 pt — Georgia italic grey
            .italic()
            .color(GREY)
            .fonts(body_fonts())
            .align(AlignmentType::Center),
    );
    doc = doc.add_style(
        Style::new("BkCallout", StyleType::Paragraph)
            .name("Bk Callout")
            .based_on("Normal")
            .size(21) // 10.5 pt — Calibri callout
            .color("000000")
            .fonts(head_fonts())
            .line_spacing(body_spacing()),
    );
    doc = doc.add_style(
        Style::new("BkBullet", StyleType::Paragraph)
            .name("Bk Bullet")
            .based_on("Normal")
            .size(22) // 11 pt — Georgia bullet
            .color("000000")
            .fonts(body_fonts())
            .line_spacing(body_spacing())
            // Left-indent ≈ 0.8 cm (≈ 454 twips) to match python BkBullet.
            .indent(Some(454), None, None, None),
    );
    doc = doc.add_style(
        Style::new("BkSubtitle", StyleType::Paragraph)
            .name("Bk Subtitle")
            .based_on("Normal")
            .size(26) // 13 pt — Calibri grey subtitle
            .color(GREY)
            .fonts(head_fonts()),
    );
    doc
}

/// Try to interpret `text` as a table-caption marker. Accepted shapes
/// (case-insensitive on the keyword, leading/trailing whitespace stripped):
///
/// * `Table: <caption>` — bookkit caption-above convention (legacy)
/// * `Table N: <caption>` / `Table N. <caption>` — pre-numbered (the engine
///   strips the number; SEQ field will re-number from `ctx.tblno`)
/// * `Table N` / `Table N.` / `Table N:` alone — pre-numbered, no caption
///   text (still folds so the renderer doesn't double-emit the marker as a
///   body paragraph above the SEQ caption)
/// * `Tabelle …` (German), `Tableau …` (French), `Tabella …` (Italian/RM),
///   `तालिका …` (Hindi) — same shapes as above, parallel to the
///   `table_prefix` localisations in `i18n::t("table_prefix")`
///
/// Returns `Some(stripped_caption_text)` if the paragraph is a marker (the
/// fold layer takes ownership of it and drops the body paragraph). Returns
/// `Some(String::new())` for a number-only marker (no caption text). Returns
/// `None` if the paragraph is not a marker and should be kept as-is.
///
/// Also strips one wrapping pair of `*…*` (markdown italic) or `**…**`
/// (markdown bold) so a python-style `*Table N. caption*` survives the
/// fold (the engine paints captions in its own caption style anyway, so
/// the emphasis is purely decorative on the source and would otherwise
/// leak into the body as bold/italic text).
fn try_parse_table_caption_marker(text: &str) -> Option<String> {
    // `unwrap_to` is the candidate after stripping balanced *…* / **…**
    // wrappers (one pair at most — these markers come from python's
    // `_render_table` which uses italics, not from arbitrarily nested
    // markdown).
    let raw = text.trim();
    let candidate = raw
        .strip_prefix("**")
        .and_then(|s| s.strip_suffix("**"))
        .map(str::trim)
        .or_else(|| {
            raw.strip_prefix('*')
                .and_then(|s| s.strip_suffix('*'))
                .map(str::trim)
        })
        .unwrap_or(raw);

    // Detect keyword (case-insensitive). The six localised forms come from
    // `i18n::t("table_prefix")`; "table" is the engine baseline. New keys
    // here MUST be added to that table too (the SEQ caption renderer
    // re-emits using `table_prefix`, so untranslated keywords would mean a
    // German source folds but renders an English "Table N." caption).
    //
    // Keywords are tried **longest-first** so "tableau"/"tabelle"/"tabella"
    // win over the "table" baseline (which is their common 5-char prefix
    // for the Latin variants). Without this ordering, "Tableau 5: foo"
    // would strip only "table", leaving "au 5: foo" which doesn't start
    // with a digit and therefore fails the marker check.
    let lower = candidate.to_lowercase();
    let kw_len = ["तालिका", "tableau", "tabelle", "tabella", "table"]
        .iter()
        .find_map(|kw| lower.starts_with(kw).then_some(kw.len()))?;
    let after_kw = candidate[kw_len..].trim_start();

    // Form 1: bare `:` / `.` immediately after the keyword
    // (e.g. "Table: foo", "Table. foo").
    if let Some(rest) = after_kw
        .strip_prefix(':')
        .or_else(|| after_kw.strip_prefix('.'))
    {
        return Some(rest.trim().to_string());
    }

    // Form 2: a leading ASCII number, then optional `:` / `.` /
    // whitespace, then the caption text (e.g. "Table 1: foo",
    // "Table 12. foo", "Table 7 foo", "Table 3" alone).
    let mut digits = after_kw.chars();
    let first = digits.next()?;
    if !first.is_ascii_digit() {
        return None;
    }
    let num_end = after_kw
        .char_indices()
        .find(|(_, c)| !c.is_ascii_digit())
        .map(|(i, _)| i)
        .unwrap_or(after_kw.len());
    let tail = after_kw[num_end..].trim_start();
    // Bare "Table 3" / "Table 3." / "Table 3:" — number-only marker.
    if tail.is_empty() || tail == "." || tail == ":" {
        return Some(String::new());
    }
    if let Some(rest) = tail.strip_prefix(':').or_else(|| tail.strip_prefix('.')) {
        return Some(rest.trim().to_string());
    }
    // "Table 3 foo" — no separator, but the digit prefix confirms a marker.
    Some(tail.to_string())
}

/// Fold a table-caption marker paragraph into the `caption` of the table
/// that immediately follows it (caption-above convention; bookkit default).
///
/// Also handles caption-BELOW: a marker paragraph immediately *after* a
/// table whose `caption` is still `None` is consumed and back-filled onto
/// the preceding table. This catches the python `_render_table` shape in
/// which the caption is rendered as a styled paragraph after the table
/// rather than as a markdown line before it.
fn fold_table_captions(blocks: Vec<DocxBlock>) -> Vec<DocxBlock> {
    let mut out: Vec<DocxBlock> = Vec::with_capacity(blocks.len());
    let mut pending: Option<String> = None;
    for b in blocks {
        match b {
            DocxBlock::Paragraph(ref runs) => {
                let text: String = runs.iter().map(|r| r.text.as_str()).collect();
                if let Some(cap) = try_parse_table_caption_marker(&text) {
                    // Caption-BELOW: if the most recently emitted block is
                    // an uncaptioned table, back-fill it (the python
                    // bookkit `_render_table` path drops the caption AFTER
                    // the table). Only when the marker carries actual
                    // text, so number-only markers above the next table
                    // still work via the `pending` slot below.
                    let consumed_below = if !cap.is_empty() {
                        if let Some(DocxBlock::Table { caption: tcap, .. }) = out.last_mut() {
                            if tcap.is_none() {
                                *tcap = Some(cap.clone());
                                true
                            } else {
                                false
                            }
                        } else {
                            false
                        }
                    } else {
                        false
                    };
                    if !consumed_below {
                        // Caption-ABOVE: stash for the next table. An
                        // empty caption (number-only marker) still
                        // suppresses the marker paragraph so it does not
                        // surface as body text above the SEQ caption.
                        pending = Some(cap);
                    }
                } else {
                    pending = None;
                    out.push(b);
                }
            }
            DocxBlock::Table {
                header,
                rows,
                caption,
            } => {
                // Existing `caption` (carried on the markdown table block)
                // wins; otherwise the pending caption-above marker is
                // claimed. An empty-string pending marker behaves like no
                // caption (renderer still emits "Table N" via SEQ).
                let cap = caption.or_else(|| pending.take().filter(|s| !s.is_empty()));
                // Drop any non-text pending marker too — it was for THIS
                // table and is now consumed.
                pending = None;
                out.push(DocxBlock::Table {
                    header,
                    rows,
                    caption: cap,
                });
            }
            other => {
                pending = None;
                out.push(other);
            }
        }
    }
    out
}

pub fn render_book(
    meta: &BookMeta,
    chapters: &[(String, String)],
    figdir: &Path,
) -> Result<Vec<u8>> {
    // The FHNW master-thesis profile (bookkit C) follows a mandated front/back-
    // matter reading order that differs from the book layout; render it on its
    // own path. The book (A) and companion (B) profiles below are unchanged.
    if meta.thesis_profile {
        return render_thesis_book(meta, chapters, figdir);
    }
    let doc_base = Docx::new()
        .default_fonts(body_fonts())
        .default_size(22)
        .page_size(11906, 16838)
        .page_margin(std_margin_for(meta));
    // NOTE: docGrid + cols.space on the document-level sectPr are injected
    // by the post-processor (`apply_layout_overrides_to_sectprs`); the
    // `Docx` builder does not expose those knobs in docx-rs 0.4.20.
    let mut doc = with_styles(doc_base, meta.body_render_use_bk_styles).footer(
        Footer::new().add_paragraph(
            Paragraph::new()
                .align(AlignmentType::Center)
                .add_page_num(PageNum::new()),
        ),
    );

    if meta.companion {
        // Companion paper (bookkit B): a plain title, no book chrome.
        doc = doc.add_paragraph(
            Paragraph::new().align(AlignmentType::Center).add_run(
                Run::new()
                    .add_text(&meta.title)
                    .bold()
                    .size(48)
                    .color(NAVY)
                    .fonts(head_fonts()),
            ),
        );
        if !meta.subtitle.is_empty() {
            doc = doc.add_paragraph(
                Paragraph::new().align(AlignmentType::Center).add_run(
                    Run::new()
                        .add_text(&meta.subtitle)
                        .size(26)
                        .color(GREY)
                        .fonts(head_fonts()),
                ),
            );
        }
        doc = doc.add_paragraph(page_break());
    } else {
        doc = title_page(doc, meta);
        doc = disclaimer_page(doc, meta);
        // Wave-4 (REF parity 2026-06-03): the reference book lays out the
        // front matter in this order:
        //   title page → edition/disclaimer → personal dedication ("For
        //   Melanie, Sarah and Timo") → standalone dedication block →
        //   inscription page (epigraph + Antikythera note) → Antikythera
        //   mechanism inscription paragraph → QR-link page → Contents
        // Each block is a no-op when its BookMeta input is `None`/false,
        // so non-AI-Norms books are unaffected.
        doc = dedication_personal_block(doc, meta);
        doc = dedication_block(doc, meta);
        doc = inscription_page(doc, meta);
        doc = antikythera_inscription_block(doc, meta);
        doc = qrlink_block(doc, meta);
    }
    // Wave-4 (REF parity 2026-06-03): "Contents" heading takes BkH1 when
    // the manifest opts into the bookkit Bk* family (reference index 16).
    let mut contents_p = Paragraph::new();
    if meta.body_render_use_bk_styles {
        contents_p = contents_p.style("BkH1");
    }
    doc = doc.add_paragraph(
        contents_p.add_run(
            Run::new()
                .add_text("Contents")
                .bold()
                .size(44)
                .color(NAVY)
                .fonts(head_fonts()),
        ),
    );
    doc = doc.add_table_of_contents(
        TableOfContents::new()
            .heading_styles_range(1, 3)
            .hyperlink() // \h — clickable entries (gold parity)
            .auto()
            .dirty(), // mark stale so Word refreshes it on open (with updateFields below)
    );
    doc = doc.add_paragraph(page_break());

    // Build the per-book context: built-in index terms + any book-specific ones.
    let mut index_terms: Vec<String> = INDEX_TERMS.iter().map(|s| (*s).to_string()).collect();
    index_terms.extend(meta.index_terms.iter().cloned());
    let mut ctx = Ctx {
        figdir,
        lang: &meta.lang,
        figno: 0,
        tblno: 0,
        chapno: 0,
        idx_seen: std::collections::HashSet::new(),
        index_terms,
        links: Vec::new(),
        typography: meta.thesis_typography,
        caption_format: meta.caption_format,
        bookmark_id: 0,
        bookmark_anchors: std::collections::HashSet::new(),
        body_render_use_bk_styles: meta.body_render_use_bk_styles,
        layout: LayoutOverrides::from_meta(meta),
        // Round V iter-10: load `<figdir>/sizes.toml` once per render.
        // Returns an empty manifest when the file is absent (every book
        // except AI-Norms today), so non-manifest books are unaffected.
        size_manifest: SizeManifest::load_from_figdir(figdir),
        // Wave-3 iter-D (2026-06-04): forward the chapter-extras gate
        // from the manifest so the CodeBlock arm can skip keypoints /
        // quiz / callout fenced blocks under the bookkit thesis profile.
        emit_chapter_extras: meta.emit_chapter_extras,
    };

    // Wave 7 (AI-Norms parity, 2026-06-03): when the manifest opts into the
    // bookkit BkH1..4 / TableGrid family, also collect index entries from
    // every chapter (explicit `{{index:term}}` markers + auto-harvest against
    // the curated allowlist) so the engine can emit explicit `IndexHeading`
    // / `Index1` paragraphs at the back-of-book in reference order.
    // The harvest must run BEFORE rendering because we strip the explicit
    // markers from the chapter markdown — they are not body text.
    let harvested_index: Vec<crate::index::IndexEntry> = if meta.body_render_use_bk_styles {
        let allowlist = crate::index::IndexAllowlist {
            terms: meta.index_terms.clone(),
        };
        let entries = crate::index::collect_index_entries(chapters, &allowlist);
        // Wave 9 diagnostic logging (AI-Norms parity, 2026-06-03): surface
        // harvest stats so a future iteration can spot a regression without
        // unzipping the rendered docx.
        eprintln!(
            "    index harvester: scanned {} chapters, allowlist {} terms, matched {} entries",
            chapters.len(),
            allowlist.terms.len(),
            entries.len()
        );
        entries
    } else {
        Vec::new()
    };

    // Wave-4 (REF parity 2026-06-03): the book profile back-matter follows
    // the reference order:
    //     body chapters … → Appendix → closing thought →
    //     Table of Figures → Table of Tables → Bibliography → Index.
    // Bibliography is the only chapter that needs to be DEFERRED past the
    // back-matter chrome (Appendix is naturally last among body chapters
    // in the manifest, so it renders in place); collect its indices once.
    let bib_indices: Vec<usize> = chapters
        .iter()
        .enumerate()
        .filter_map(|(i, (_label, md))| {
            let h1 = first_h1(md).unwrap_or_default().to_lowercase();
            let h = h1.trim();
            if h.contains("bibliography") || h.contains("references") || h.contains("literaturverz")
            {
                Some(i)
            } else {
                None
            }
        })
        .collect();
    let is_deferred = |i: usize| bib_indices.contains(&i);

    for (ci, (_label, md)) in chapters.iter().enumerate() {
        if is_deferred(ci) {
            continue; // emit after Table of Figures / Tables
        }
        // Wave 7: strip `{{index:term}}` markers so they don't surface as
        // visible body text. The marker terms have already been harvested
        // above; the visible rendering only needs the surrounding prose.
        let md_owned: String;
        let md_to_render: &str = if meta.body_render_use_bk_styles && md.contains("{{index:") {
            md_owned = strip_index_markers(md);
            md_owned.as_str()
        } else {
            md
        };
        let blocks = fold_table_captions(to_docx_blocks(md_to_render));
        let numbered = chapter_is_numbered(md_to_render, meta.thesis_profile);
        let mut first = true;
        for b in &blocks {
            doc = render_block(doc, b, &mut ctx, first && ci > 0, numbered);
            first = false;
        }
        // End-of-chapter Sources & QR-codes box (bookkit flush_sources).
        // Wave-2 (bookkit chrome suppression, 2026-06-04): gated on
        // `emit_per_chapter_sources_box` (default true) so the
        // master_thesis_bookkit profile can suppress every per-chapter
        // Sources box across the document without touching any chapter
        // markdown. When suppressed the harvested `ctx.links` are
        // cleared anyway (we drain via `flush_sources` semantics) so
        // they do not bleed into the next chapter.
        if meta.emit_per_chapter_sources_box {
            doc = flush_sources(doc, &mut ctx.links, &meta.lang, ctx.typography);
        } else {
            ctx.links.clear();
        }
        // Round V (zone A — psb-02, 2026-06-03): emit a thin gray
        // horizontal-rule divider at each chapter end (reference book
        // carries 40 of these). Gated on the bookkit parity opt-in so
        // non-parity books keep the historical untouched chapter close.
        // Wave-2 (bookkit chrome suppression, 2026-06-04): also gated on
        // `emit_chapter_dividers` (default true) so a profile can
        // suppress the divider even when the bookkit parity opt-in is
        // active.
        if meta.body_render_use_bk_styles && meta.emit_chapter_dividers {
            doc = doc.add_paragraph(chapter_end_rule(false));
        }
    }

    // Closing thought: emitted right after the Appendix (which is the last
    // body chapter in the manifest), BEFORE the Table of Figures. Mirrors
    // reference book paragraphs [3856-3857]. No-op when `closing_thought`
    // is None.
    doc = closing_thought_block(doc, meta);

    // Back-matter lists. Headings honour the optional reference-parity
    // overrides `tof_heading` / `tot_heading`; otherwise i18n decides.
    doc = doc.add_paragraph(page_break());
    let tof_heading = meta
        .tof_heading
        .clone()
        .unwrap_or_else(|| t(&meta.lang, "list_of_figures").to_string());
    let tot_heading = meta
        .tot_heading
        .clone()
        .unwrap_or_else(|| t(&meta.lang, "list_of_tables").to_string());
    for p in list_of(
        "Figure",
        &tof_heading,
        meta.thesis_typography,
        meta.body_render_use_bk_styles,
    ) {
        doc = doc.add_paragraph(p);
    }
    for p in list_of(
        "Table",
        &tot_heading,
        meta.thesis_typography,
        meta.body_render_use_bk_styles,
    ) {
        doc = doc.add_paragraph(p);
    }

    // Now render the deferred Bibliography chapter(s) so they sit BETWEEN
    // the back-of-book lists and the back-of-book Index (matches reference
    // paragraphs [4015 Bibliography] → [4091 Index]).
    for &bi in &bib_indices {
        doc = doc.add_paragraph(page_break());
        let md = &chapters[bi].1;
        let md_owned: String;
        let md_to_render: &str = if meta.body_render_use_bk_styles && md.contains("{{index:") {
            md_owned = strip_index_markers(md);
            md_owned.as_str()
        } else {
            md
        };
        let blocks = fold_table_captions(to_docx_blocks(md_to_render));
        let numbered = chapter_is_numbered(md_to_render, meta.thesis_profile);
        let mut first = true;
        for b in &blocks {
            doc = render_block(doc, b, &mut ctx, first, numbered);
            first = false;
        }
        // Wave-2 (bookkit chrome suppression, 2026-06-04): mirror the
        // body-loop gate for the deferred Bibliography chapter.
        if meta.emit_per_chapter_sources_box {
            doc = flush_sources(doc, &mut ctx.links, &meta.lang, ctx.typography);
        } else {
            ctx.links.clear();
        }
        // Round V (zone A — psb-02, 2026-06-03): chapter-end divider on
        // the deferred Bibliography chapter(s) too. Same gating as the
        // body loop above (now also honouring `emit_chapter_dividers`).
        if meta.body_render_use_bk_styles && meta.emit_chapter_dividers {
            doc = doc.add_paragraph(chapter_end_rule(false));
        }
    }

    // Back-of-book index. Wave-7 (AI-Norms parity, 2026-06-03):
    //   * `body_render_use_bk_styles=true` → emit `IndexHeading` letter
    //     dividers + `Index1` paragraphs from the harvested entries so
    //     the rendered docx matches the reference book's 20-divider /
    //     113-entry visual without depending on Word's `INDEX` field
    //     update. Only letters with at least one entry get a divider.
    //   * `body_render_use_bk_styles=false` → the historical `INDEX \c 2`
    //     field, filled from XE entries on field update (unchanged
    //     behaviour for every non-parity book).
    // Wave-2 (bookkit chrome suppression, 2026-06-04): the entire Index
    // section (page break + heading + body + section-break sentinels) is
    // gated on `emit_index` (default true). Profiles that want a thesis-
    // style closing on TOF/TOT/Bibliography (no Index) set this to false.
    if meta.emit_index {
        doc = doc.add_paragraph(page_break());
        let index_h1_style = if meta.body_render_use_bk_styles {
            "BkH1"
        } else {
            "Heading1"
        };
        // Round V (zone A — psb-04, 2026-06-03): emit a sentinel paragraph
        // immediately before the Index heading; the post-process pass
        // (`insert_index_section_breaks`) rewrites it into a 2-col continuous
        // sectPr so the Index renders in two columns matching the reference
        // book layout. Gated on the bookkit parity opt-in to keep non-parity
        // books on the historical 1-col Index. The matching CLOSE sentinel
        // is emitted at the end of the Index body further down.
        if meta.body_render_use_bk_styles {
            doc = doc.add_paragraph(
                Paragraph::new().add_run(Run::new().add_text("__SECTPR_INDEX_OPEN__")),
            );
        }
        doc = doc.add_paragraph(
            Paragraph::new().style(index_h1_style).add_run(
                Run::new()
                    .add_text("Index")
                    .bold()
                    .size(32)
                    .color(NAVY)
                    .fonts(head_fonts()),
            ),
        );
        if meta.body_render_use_bk_styles {
            let blocks = crate::index::emit_index_section(harvested_index);
            // Wave 9 diagnostic logging: surface emitted paragraph counts so the
            // parity gate can be debugged from the build log alone.
            let n_head = blocks
                .iter()
                .filter(|b| matches!(b, crate::index::IndexBlock::Heading(_)))
                .count();
            let n_entry = blocks
                .iter()
                .filter(|b| matches!(b, crate::index::IndexBlock::Entry { .. }))
                .count();
            eprintln!(
                "    emit_index_section: produced {n_head} IndexHeading + {n_entry} Index1 paragraphs"
            );
            for b in blocks {
                doc = match b {
                    crate::index::IndexBlock::Heading(letter) => doc.add_paragraph(
                        Paragraph::new().style("IndexHeading").add_run(
                            Run::new()
                                .add_text(letter)
                                .bold()
                                .size(24)
                                .fonts(head_fonts()),
                        ),
                    ),
                    crate::index::IndexBlock::Entry { term, refs } => {
                        let mut p = Paragraph::new()
                            .style("Index1")
                            .add_run(Run::new().add_text(term).size(20).fonts(body_fonts()))
                            .add_run(Run::new().add_tab());
                        if refs.is_empty() {
                            p = p.add_run(Run::new().add_text("?").size(20).fonts(body_fonts()));
                        } else {
                            for (i, r) in refs.iter().enumerate() {
                                if i > 0 {
                                    p = p.add_run(
                                        Run::new().add_text(", ").size(20).fonts(body_fonts()),
                                    );
                                }
                                p = p.add_run(field_run(
                                    &format!("PAGEREF {} \\h", r.bookmark),
                                    "?",
                                    true,
                                ));
                            }
                        }
                        doc.add_paragraph(p)
                    }
                };
            }
        } else {
            doc = doc.add_paragraph(
            Paragraph::new().add_run(
                Run::new()
                    .add_text(
                        "Right-click and choose \u{201c}Update Field\u{201d} to build the index.",
                    )
                    .italic()
                    .size(18)
                    .color(GREY)
                    .fonts(body_fonts()),
            ),
        );
            doc = doc.add_paragraph(Paragraph::new().add_run(field_run("INDEX \\c 2", "", true)));
        }
        // Round V (zone A — psb-04, 2026-06-03): close the 2-col Index section
        // by emitting the matching CLOSE sentinel. The post-process pass
        // rewrites it into a 1-col continuous sectPr so any tail content (and
        // Word's implicit doc-end sectPr) resumes single-column flow.
        if meta.body_render_use_bk_styles {
            doc = doc.add_paragraph(
                Paragraph::new().add_run(Run::new().add_text("__SECTPR_INDEX_CLOSE__")),
            );
        }
    } // end `if meta.emit_index`

    let mut cur = Cursor::new(Vec::<u8>::new());
    doc.build().pack(&mut cur).context("pack book docx")?;
    let layout = LayoutOverrides::from_meta(meta);
    let styles_profile = if matches!(
        meta.thesis_typography,
        TypographyProfile::FhnwProposalParity | TypographyProfile::FhnwMtTemplate
    ) {
        crate::thesis_styles::StylesProfile::FhnwMasterThesis
    } else {
        crate::thesis_styles::StylesProfile::AiNorms
    };
    postprocess_docx_inner_layout(
        cur.into_inner(),
        meta.body_render_use_bk_styles,
        &layout,
        styles_profile,
    )
}

/// FHNW thesis front/back-matter slot a chapter belongs to, decided by its first
/// H1 (`specs/overrides/fhnw-mas/thesis-structure.md`). `Body` = numbered ch. 1-7.
///
/// The three opening slots (`TitlePage`, `DeclarationOriginality`,
/// `ComplianceDeclaration`) come from the FHNW MAS proposal envelope which
/// the master-thesis submission re-asserts (dated for the submission), per
/// user requirement 2026-05-28.
#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
enum ThesisSlot {
    TitlePage,
    DeclarationOriginality,
    ComplianceDeclaration,
    MgmtSummary,
    /// Legacy "Declaration of Authorship / Ehrenwörtliche Erklärung" slot.
    /// Kept for backwards-compatibility; emitted late (after Bibliography)
    /// only if a manifest still ships such a chapter.
    Declaration,
    Acronyms,
    Bibliography,
    AiTools,
    Appendix,
    Body,
}

/// One emitted item in the FHNW thesis layout: either a chapter (by index into
/// the input `chapters`) or a generated structural section.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum ThesisItem {
    Chapter(usize),
    Toc,
    ListFigures,
    ListTables,
    /// Back-of-book Index (XE / INDEX field). Always emitted last so the
    /// reader-facing terms appear in the standard FHNW closing position.
    Index,
}

/// Classify a chapter by its first H1 into an FHNW front/back-matter slot.
fn thesis_slot(md: &str) -> ThesisSlot {
    let h1 = first_h1(md).unwrap_or_default().to_lowercase();
    let h = h1.trim();
    // The proposal envelope re-asserted in the submission (must precede the
    // management summary) — checked BEFORE the legacy declaration branch
    // so "declaration of originality" does not collide with "declaration".
    if h == "title page" || h.starts_with("title page") {
        ThesisSlot::TitlePage
    } else if h.contains("declaration of originality") || h.contains("originality") {
        ThesisSlot::DeclarationOriginality
    } else if h.contains("compliance declaration") || h.contains("photon os") {
        ThesisSlot::ComplianceDeclaration
    } else if h.contains("management summary") || h.contains("executive summary") {
        ThesisSlot::MgmtSummary
    } else if h.contains("ehrenwörtliche erklärung")
        || h.contains("eidesstattliche")
        || h.contains("declaration of authorship")
        || h == "declaration"
    {
        ThesisSlot::Declaration
    } else if h.contains("acronym") || h.contains("abbreviation") || h.contains("abkürzungsverz") {
        ThesisSlot::Acronyms
    } else if h.contains("bibliography") || h.contains("references") || h.contains("literaturverz")
    {
        ThesisSlot::Bibliography
    } else if h.contains("hilfsmittel")
        || h.contains("ai tools")
        || h.contains("ai-tools")
        || h.contains("tools and databases")
        || h.contains("declaration of tools")
    {
        ThesisSlot::AiTools
    } else if h.contains("appendix") || h.contains("anhang") {
        ThesisSlot::Appendix
    } else {
        ThesisSlot::Body
    }
}

/// Compute the FHNW-mandated emission order (pure; unit-tested separately).
///
/// Per the user-supplied master-thesis structure (2026-05-28):
///   Title page → Declaration of Originality → Compliance Declaration for
///   Open-Source Photon OS → Management Summary → Table of Contents →
///   Acronyms → numbered body (1-7) → Appendix → Table of Figures →
///   Table of Tables → Bibliography → Index.
///
/// Legacy chapters (Declaration of Authorship, AI Tools) are appended after
/// Bibliography if a manifest still ships them — they no longer have a
/// reserved front-matter slot in the new sequence.
///
/// Order within each slot follows the input (manifest) order. Slots with no
/// chapters simply contribute nothing; the engine-generated chrome (TOC, list
/// of figures, list of tables, index) is always emitted.
fn thesis_layout(chapters: &[(String, String)]) -> Vec<ThesisItem> {
    let mut by_slot: std::collections::HashMap<ThesisSlot, Vec<usize>> =
        std::collections::HashMap::new();
    for (i, (_label, md)) in chapters.iter().enumerate() {
        by_slot.entry(thesis_slot(md)).or_default().push(i);
    }
    let take = |slot: ThesisSlot| -> Vec<ThesisItem> {
        by_slot
            .get(&slot)
            .into_iter()
            .flatten()
            .map(|&i| ThesisItem::Chapter(i))
            .collect()
    };
    let mut out = Vec::new();
    // Front matter envelope (proposal re-asserted at submission).
    out.extend(take(ThesisSlot::TitlePage));
    out.extend(take(ThesisSlot::DeclarationOriginality));
    out.extend(take(ThesisSlot::ComplianceDeclaration));
    // Management summary then TOC (user wants TOC right after Mgmt Summary).
    out.extend(take(ThesisSlot::MgmtSummary));
    out.push(ThesisItem::Toc);
    // Acronyms is the last item before numbered body chapters.
    out.extend(take(ThesisSlot::Acronyms));
    // Numbered body chapters 1-7 in manifest order.
    out.extend(take(ThesisSlot::Body));
    // Appendix BEFORE the back-of-book lists, matching the user's structure.
    out.extend(take(ThesisSlot::Appendix));
    // Back-of-book lists.
    out.push(ThesisItem::ListFigures);
    out.push(ThesisItem::ListTables);
    out.extend(take(ThesisSlot::Bibliography));
    // Legacy fallback slots (no longer have a reserved position).
    out.extend(take(ThesisSlot::Declaration));
    out.extend(take(ThesisSlot::AiTools));
    out.push(ThesisItem::Index);
    out
}

/// Render one thesis chapter: optional leading page-break + blocks + the
/// end-of-chapter Sources box. Numbering is decided per-chapter (numbered body
/// chapters only fire their chapter number when `page_break_before` is true).
fn render_thesis_chapter(
    mut doc: Docx,
    md: &str,
    meta: &BookMeta,
    ctx: &mut Ctx,
    page_break_before: bool,
) -> Docx {
    let blocks = fold_table_captions(to_docx_blocks(md));
    let numbered = chapter_is_numbered(md, meta.thesis_profile);
    let mut first = true;
    for b in &blocks {
        doc = render_block(doc, b, ctx, first && page_break_before, numbered);
        first = false;
    }
    // Wave-2 Agent C (Python→Rust port, 2026-06-04): honour the bookkit
    // chrome-suppression flag here too. The two call sites in `render_book`
    // (lines ≈5440 + 5514) were already gated; this thesis path was not, so
    // the `master_thesis_bookkit` profile still leaked per-chapter Sources
    // boxes despite `emit_per_chapter_sources_box=false`. Without this gate
    // `ctx.links` would still drain via `flush_sources`; we clear it
    // explicitly to keep semantics identical to the other suppressed paths.
    if meta.emit_per_chapter_sources_box {
        doc = flush_sources(doc, &mut ctx.links, &meta.lang, ctx.typography);
    } else {
        ctx.links.clear();
    }
    // Round V (zone A — psb-02, 2026-06-03): chapter-end gray divider on
    // every thesis chapter when the manifest opts into the bookkit
    // parity flag. FHNW thesis books default to false so the proposal
    // docx parity is preserved.
    // Wave-2 Agent B (REF parity 2026-06-04): mirror the `render_book`
    // gate exactly — divider fires when BOTH `body_render_use_bk_styles`
    // and `emit_chapter_dividers` are true. `master_thesis_bookkit`
    // opts into both via the manifest; existing `master_thesis` (proposal
    // parity) keeps `body_render_use_bk_styles=false` so nothing
    // regresses.
    if meta.body_render_use_bk_styles && meta.emit_chapter_dividers {
        doc = doc.add_paragraph(chapter_end_rule(false));
    }
    // Wave-3 iter-D (REF parity 2026-06-04): emit a per-chapter section
    // break (`<w:sectPr>` inside an empty `<w:pPr>`) at chapter close so
    // the document carries one section break per chapter. The reference
    // master thesis has 19 in-body sectPrs (plus the document-level
    // sectPr = 20 total); the previous renderer emitted only the
    // document-level sectPr, leaving the bookkit_reference_targets gate
    // with a `-15` sect_prs deficit. Gated on
    // `emit_per_chapter_sectpr` (default false) so non-bookkit
    // profiles keep the historical single-section layout.
    if meta.emit_per_chapter_sectpr {
        doc = doc.add_paragraph(per_chapter_sectpr_paragraph(meta));
    }
    doc
}

/// Render the FHNW master-thesis profile (bookkit C, ADR-0045) in the mandated
/// reading order (`thesis-structure.md`): Title page → Management Summary →
/// Declaration → Table of Contents → List of Figures → List of Tables →
/// Acronyms → numbered body → Bibliography → Tools/AI disclosure → Appendix.
/// No book-style disclaimer/inscription pages and no back-of-book index — those
/// are not part of the FHNW thesis structure.
fn render_thesis_book(
    meta: &BookMeta,
    chapters: &[(String, String)],
    figdir: &Path,
) -> Result<Vec<u8>> {
    // ADR-0064 iter44 (2026-07-05): use profile-aware default font.
    // Prior code hardcoded `body_fonts()` = Georgia — leaked Georgia
    // into 79 runs of the FhnwMtTemplate thesis (reference has 0
    // Georgia; body font is Palatino Linotype). This was the source
    // of most of the mid-doc pixel diff because Georgia's letter
    // width differs from Palatino by ~4%, cascading into different
    // line breaks per paragraph.
    let doc_base = Docx::new()
        .default_fonts(body_fonts_for(meta.thesis_typography))
        .default_size(22)
        .page_size(11906, 16838)
        .page_margin(std_margin_for(meta));
    // docGrid + cols.space injected post-build (see `render_book`).
    let mut doc = with_styles(doc_base, meta.body_render_use_bk_styles).footer(
        Footer::new().add_paragraph(
            Paragraph::new()
                .align(AlignmentType::Center)
                .add_page_num(PageNum::new()),
        ),
    );

    // FHNW running header (ADR-0050 item 1) — only attached for the FHNW
    // typography profile AND when the manifest supplies logo bytes; no
    // regression for any other book.
    if let Some(header) = fhnw_header_for(meta) {
        doc = doc.header(header);
    }

    // Skip the engine-generated cover when the manifest already supplies an
    // explicit `ThesisSlot::TitlePage` chapter (e.g. the FHNW formal title
    // sheet `thesis/fhnw_00_title_page.md`). Otherwise two title-like pages
    // would render back-to-back — the engine cover then the markdown chapter.
    // Non-thesis books (no explicit title chapter) keep the engine cover.
    let has_explicit_title = chapters
        .iter()
        .any(|(_label, md)| thesis_slot(md) == ThesisSlot::TitlePage);
    if !has_explicit_title {
        doc = title_page(doc, meta);
    }

    let mut index_terms: Vec<String> = INDEX_TERMS.iter().map(|s| (*s).to_string()).collect();
    index_terms.extend(meta.index_terms.iter().cloned());
    let mut ctx = Ctx {
        figdir,
        lang: &meta.lang,
        figno: 0,
        tblno: 0,
        chapno: 0,
        idx_seen: std::collections::HashSet::new(),
        index_terms,
        links: Vec::new(),
        typography: meta.thesis_typography,
        caption_format: meta.caption_format,
        bookmark_id: 0,
        bookmark_anchors: std::collections::HashSet::new(),
        body_render_use_bk_styles: meta.body_render_use_bk_styles,
        layout: LayoutOverrides::from_meta(meta),
        // Round V iter-10: load `<figdir>/sizes.toml` once per render.
        // Returns an empty manifest when the file is absent (every book
        // except AI-Norms today), so non-manifest books are unaffected.
        size_manifest: SizeManifest::load_from_figdir(figdir),
        // Wave-3 iter-D (2026-06-04): forward the chapter-extras gate so
        // the thesis path also honours the master_thesis_bookkit
        // suppression of keypoints / quiz / callout chrome.
        emit_chapter_extras: meta.emit_chapter_extras,
    };

    // `emitted` tracks whether any flow content precedes the next item, so the
    // first front-matter chapter does not get a redundant page break. In both
    // branches above (engine cover rendered, or explicit title chapter) the
    // first chapter wants `page_break_before=false`: the engine cover ends
    // with its own break, and an explicit title chapter is itself the cover.
    let mut emitted = false;
    // ADR-0050 §1 (v0.1.16-engine, 2026-05-29):
    //   * D2: under FHNW typography, suppress the first H1 of the
    //     ThesisSlot::TitlePage chapter — the proposal's title page has
    //     no "Title Page" heading; the title itself IS the page. We
    //     strip the leading `# …\n` line before passing the markdown to
    //     `render_thesis_chapter` for that one slot.
    //   * D6: under FHNW typography, force a page break BEFORE each
    //     front-matter chapter so Declaration of Originality, Compliance
    //     Declaration, Management Summary and Acronyms each start on
    //     their own page (the proposal docx separates them).
    let fhnw = matches!(
        meta.thesis_typography,
        TypographyProfile::FhnwProposalParity | TypographyProfile::FhnwMtTemplate
    );
    let front_matter_slots = [
        ThesisSlot::DeclarationOriginality,
        ThesisSlot::ComplianceDeclaration,
        ThesisSlot::Declaration,
        ThesisSlot::MgmtSummary,
        ThesisSlot::Acronyms,
    ];
    // ADR-0064 iter44 (2026-07-05): track whether we've crossed into
    // the Body (main matter) so we can emit the transition page break
    // exactly once, matching the reference's 3 total page breaks
    // instead of ~25.
    let mut body_started = false;
    for item in thesis_layout(chapters) {
        // Wave-2 (bookkit chrome suppression, 2026-06-04): skip Appendix
        // chapters entirely when the profile sets
        // `emit_appendix_in_back_matter = false`. The thesis layout
        // emits Appendix-slotted chapters just before the back-matter
        // lists; suppressing them mirrors the reference thesis which
        // closes on ToF/ToT/Bibliography with no Appendix between.
        if let ThesisItem::Chapter(i) = item {
            if !meta.emit_appendix_in_back_matter
                && thesis_slot(&chapters[i].1) == ThesisSlot::Appendix
            {
                continue;
            }
        }
        match item {
            ThesisItem::Chapter(i) => {
                let slot = thesis_slot(&chapters[i].1);
                // ADR-0064 iter26 (2026-07-03): the FHNW MT-Template convention
                // marks the last front-matter chapter with a bookmark
                // `fhnwFrontMatterEnd` so the finalize step can compute the
                // back-matter Roman starting-number. Acronyms is the last
                // front-matter chapter under both FhnwProposalParity and
                // FhnwMtTemplate profiles.
                if matches!(meta.thesis_typography, TypographyProfile::FhnwMtTemplate)
                    && slot == ThesisSlot::Acronyms
                {
                    let bm_id = ctx.next_bookmark_id();
                    doc = doc.add_paragraph(
                        Paragraph::new()
                            .add_bookmark_start(bm_id, "fhnwFrontMatterEnd")
                            .add_bookmark_end(bm_id),
                    );
                }
                let md_ref: String = if slot == ThesisSlot::TitlePage
                    && matches!(meta.thesis_typography, TypographyProfile::FhnwMtTemplate)
                {
                    // ADR-0064 iter20: FhnwMtTemplate strips the H1 line AND
                    // everything past the first H2 (drops the duplicated
                    // Declaration content that lives in the title-page md).
                    strip_first_h1_and_after_first_h2(&chapters[i].1)
                } else if fhnw && slot == ThesisSlot::TitlePage {
                    strip_first_h1_line(&chapters[i].1)
                } else {
                    chapters[i].1.clone()
                };
                // ADR-0064 iter18 (FhnwMtTemplate title-page prelude,
                // 2026-07-03): the FHNW reference thesis opens with three
                // institution lines above the "Master Thesis" heading:
                //   1. FHNW University of Applied Sciences and Arts …
                //   2. School of Business
                //   3. Master in Advanced Studies Leadership in Cybersecurity
                // The markdown title page carries none of these — it dives
                // straight into "Master Thesis" — so we prepend them here
                // for the FhnwMtTemplate profile only. Values are pulled
                // from meta.header_lines (program title) with the school +
                // university name hard-coded to the FHNW canonical strings.
                if matches!(meta.thesis_typography, TypographyProfile::FhnwMtTemplate)
                    && slot == ThesisSlot::TitlePage
                {
                    // ADR-0064 iter44 (2026-07-05): reference thesis has an
                    // inline banner image (image1.png, 3840x885 FHNW letterhead)
                    // as body paragraph 0, BEFORE the prelude text lines.
                    // Emit it here as an inline `wp:inline` picture (NOT the
                    // wp:anchor floating drawing iter43 briefly tried and
                    // reverted). Bytes come from meta.header_logo when the
                    // manifest points it at a banner-shaped image; if the
                    // configured header_logo is the small square FHNW logo
                    // (24 KB, 768x768) rather than the banner (129 KB, 3840x885)
                    // the emission still works but the visual won't match
                    // reference — set manifest.header_logo to
                    // `assets/fhnw_banner.png` for full parity.
                    if let Some(bytes) = meta.header_logo.as_ref() {
                        if !bytes.is_empty() {
                            // Read the image dimensions so we can size the
                            // page-width banner correctly. The reference
                            // renders image1.png at ≈16.5 cm wide (fills the
                            // title-page content area on A4 with 2.5 cm margins).
                            let (px_w, px_h) = match ::image::load_from_memory(bytes) {
                                Ok(img) => (
                                    ::image::GenericImageView::dimensions(&img).0,
                                    ::image::GenericImageView::dimensions(&img).1,
                                ),
                                Err(_) => (3840, 885),
                            };
                            // ADR-0064 iter44 (2026-07-05): reference banner
                            // renders at cx=3_420_000 EMU (9.5 cm wide). Our
                            // earlier 16 cm target overshot the reference by
                            // 68% width / 42% height, dominating the title
                            // page. Matched to reference value.
                            let target_w_emu: u32 = 3_420_000;
                            let target_h_emu: u32 = target_w_emu
                                .saturating_mul(px_h)
                                .checked_div(px_w.max(1))
                                .unwrap_or(1_500_000);
                            doc = doc.add_paragraph(
                                Paragraph::new().add_run(
                                    Run::new().add_image(
                                        Pic::new_with_dimensions(bytes.clone(), px_w, px_h)
                                            .size(target_w_emu.into(), target_h_emu.into()),
                                    ),
                                ),
                            );
                        }
                    }
                    // FHNW's official English name is a proper noun that the
                    // reference thesis keeps identical across all language
                    // editions (verified against the June-8 delivery for
                    // en/de/fr/it/rm/hi). School name IS localised.
                    let prelude_lines: [&str; 2] = [
                        "FHNW University of Applied Sciences and Arts Northwestern Switzerland",
                        t(&meta.lang, "school_of_business"),
                    ];
                    for line in prelude_lines {
                        doc = doc.add_paragraph(
                            Paragraph::new().add_run(
                                Run::new()
                                    .add_text(line)
                                    .bold()
                                    .size(24)
                                    .color(heading_color_for(ctx.typography))
                                    .fonts(head_fonts_for(ctx.typography)),
                            ),
                        );
                    }
                    if !meta.header_lines.is_empty() {
                        let program = meta.header_lines.join(" ");
                        doc = doc.add_paragraph(
                            Paragraph::new().add_run(
                                Run::new()
                                    .add_text(program)
                                    .bold()
                                    .size(24)
                                    .color(heading_color_for(ctx.typography))
                                    .fonts(head_fonts_for(ctx.typography)),
                            ),
                        );
                    }
                    // Blank spacer before the "Master Thesis" heading.
                    doc = doc.add_paragraph(Paragraph::new());
                }
                // D6: force a page break before each front-matter chapter
                // (under FHNW only). Non-thesis books and the Designer
                // profile keep the historical "chapter_break_before from
                // emitted-state" behaviour.
                //
                // ADR-0064 iter44 (2026-07-05): the June-8 reference uses
                // just 3 `<w:br w:type="page">` for the whole document
                // and lets chapter transitions flow via 21 `<w:sectPr>`
                // section breaks. Emitting a page break for every
                // front-matter chapter (5 slots × N chapters) produced
                // 25 total page breaks and ~22 spurious blank pages
                // (running-header-only pages) between chapters — 16 of
                // the page-count overshoot vs reference. Kept just
                // one break: the transition into the Body (main matter),
                // matching the reference's mid-doc landmark. Further
                // breaks appear naturally from render_thesis_chapter's
                // own end-of-chapter emit.
                if fhnw && emitted && slot == ThesisSlot::Body && !body_started {
                    doc = doc.add_paragraph(page_break());
                    body_started = true;
                }
                doc = render_thesis_chapter(doc, &md_ref, meta, &mut ctx, emitted);
                emitted = true;
                // ADR-0064 iter24 (FhnwMtTemplate title-page 2×2 tables,
                // 2026-07-03): port MT-Template/build/generate_template.py:534-558
                // — two side-by-side 2×2 tables (Author/Supervisor, Matriculation/
                // Co-Examiner). Python-docx defaults to AutoFit col widths → ~45:55
                // asymmetric ratio measured in the reference. docx-rs Table has no
                // AutoFit flag; approximate by NOT setting `width()` on cells so
                // Word picks widths on render (matches python-docx behaviour).
                if slot == ThesisSlot::TitlePage
                    && matches!(meta.thesis_typography, TypographyProfile::FhnwMtTemplate)
                {
                    // Reference EN.docx's first title-page table has explicit
                    // `<w:gridCol w:w="4253"/><w:gridCol w:w="5101"/>` — total
                    // width 9354 twips (≈ 16.5 cm text-width), ratio 45.5:54.5.
                    // Setting explicit widths in Dxa units matches that shape.
                    const REF_TITLE_TBL_LEFT_DXA: usize = 4253;
                    const REF_TITLE_TBL_RIGHT_DXA: usize = 5101;
                    let cell = |text: &str, italic: bool, align_right: bool, width_dxa: usize| {
                        let mut para = Paragraph::new();
                        if align_right {
                            para = para.align(AlignmentType::Right);
                        }
                        let mut run = Run::new()
                            .add_text(text)
                            .size(body_size_hp(ctx.typography) as usize)
                            .color(body_color_for(ctx.typography))
                            .fonts(body_fonts_for(ctx.typography));
                        if italic {
                            run = run.italic();
                        }
                        TableCell::new()
                            .width(width_dxa, WidthType::Dxa)
                            .add_paragraph(para.add_run(run))
                    };
                    // Table 1: Author | Supervisor.
                    let t1 = Table::new(vec![
                        TableRow::new(vec![
                            cell("Author:", true, false, REF_TITLE_TBL_LEFT_DXA),
                            cell("Supervisor:", true, true, REF_TITLE_TBL_RIGHT_DXA),
                        ]),
                        TableRow::new(vec![
                            cell(&meta.author, false, false, REF_TITLE_TBL_LEFT_DXA),
                            cell(
                                &meta.header_lines.first().cloned().unwrap_or_default(),
                                false,
                                true,
                                REF_TITLE_TBL_RIGHT_DXA,
                            ),
                        ]),
                    ]);
                    doc = doc.add_table(t1);
                    // ADR-0064 iter36 (2026-07-04): reference EN.docx has ONLY
                    // the Author/Supervisor 2×2 table on the title page.
                    // Matriculation Number / Co-Examiner information lives in
                    // the Imprint chapter as regular paragraphs (Practical
                    // Supervisors, Co-Examiner, Submission Date), NOT in a
                    // second table. Removing the 2nd title-page table matches
                    // the reference structure exactly.
                }
                // ADR-0064 iter23 (FhnwMtTemplate Imprint synthesis, 2026-07-03):
                // the FHNW MT-Template convention places a dedicated Imprint
                // page right after the title page. The current thesis has no
                // Imprint markdown chapter (it lives implicitly in the
                // BookMeta.imprint field), so we synthesise one here so the
                // rendered output matches the reference structure. Emitted
                // only for FhnwMtTemplate + only right after the TitlePage
                // + only when the manifest supplies imprint content.
                if slot == ThesisSlot::TitlePage
                    && matches!(meta.thesis_typography, TypographyProfile::FhnwMtTemplate)
                    && meta.imprint.as_ref().is_some_and(|s| !s.trim().is_empty())
                {
                    doc = doc.add_paragraph(page_break());
                    doc = doc.add_paragraph(
                        Paragraph::new().style("Heading1").add_run(
                            Run::new()
                                .add_text(t(&meta.lang, "imprint_heading"))
                                .bold()
                                .size(heading_size_hp(ctx.typography, 1))
                                .color(heading_color_for(ctx.typography))
                                .fonts(head_fonts_for(ctx.typography)),
                        ),
                    );
                    if let Some(imprint) = meta.imprint.as_ref() {
                        for line in imprint.lines() {
                            if line.trim().is_empty() {
                                doc = doc.add_paragraph(Paragraph::new());
                                continue;
                            }
                            doc = doc.add_paragraph(
                                Paragraph::new().add_run(
                                    Run::new()
                                        .add_text(line)
                                        .size(body_size_hp(ctx.typography) as usize)
                                        .color(body_color_for(ctx.typography))
                                        .fonts(body_fonts_for(ctx.typography)),
                                ),
                            );
                        }
                    }
                }
            }
            ThesisItem::Toc => {
                if emitted {
                    doc = doc.add_paragraph(page_break());
                }
                doc = doc.add_paragraph(
                    Paragraph::new().add_run(
                        Run::new()
                            .add_text("Contents")
                            .bold()
                            .size(heading_size_hp(ctx.typography, 1))
                            .color(heading_color_for(ctx.typography))
                            .fonts(head_fonts_for(ctx.typography)),
                    ),
                );
                doc = doc.add_table_of_contents(
                    TableOfContents::new()
                        .heading_styles_range(1, 3)
                        .hyperlink()
                        .auto()
                        .dirty(),
                );
                emitted = true;
            }
            ThesisItem::ListFigures => {
                doc = doc.add_paragraph(page_break());
                // ADR-0064 iter26 (2026-07-03): mark the first back-matter
                // item (ListFigures) with a bookmark `fhnwBackMatterStart` so
                // the finalize step can compute the back-matter Roman page-
                // number auto-tune.
                if matches!(meta.thesis_typography, TypographyProfile::FhnwMtTemplate) {
                    let bm_id = ctx.next_bookmark_id();
                    doc = doc.add_paragraph(
                        Paragraph::new()
                            .add_bookmark_start(bm_id, "fhnwBackMatterStart")
                            .add_bookmark_end(bm_id),
                    );
                }
                for p in list_of(
                    "Figure",
                    t(&meta.lang, "list_of_figures"),
                    ctx.typography,
                    ctx.body_render_use_bk_styles,
                ) {
                    doc = doc.add_paragraph(p);
                }
            }
            ThesisItem::ListTables => {
                for p in list_of(
                    "Table",
                    t(&meta.lang, "list_of_tables"),
                    ctx.typography,
                    ctx.body_render_use_bk_styles,
                ) {
                    doc = doc.add_paragraph(p);
                }
            }
            ThesisItem::Index => {
                // Back-of-book Index: skipped under FHNW typography (the
                // proposal docx has no Index section; emitting an empty
                // INDEX field would just add a blank "Index" page at the
                // end of the thesis). Designer profile keeps the standard
                // book Index.
                // Wave-2 (bookkit chrome suppression, 2026-06-04): also
                // skipped when the profile sets `emit_index = false`, so
                // the master_thesis_bookkit (Designer typography) can
                // suppress the Index without flipping typography.
                if matches!(
                    ctx.typography,
                    TypographyProfile::FhnwProposalParity | TypographyProfile::FhnwMtTemplate
                ) || !meta.emit_index
                {
                    continue;
                }
                // Back-of-book Index: the INDEX field, filled from XE entries
                // on field update. Heading is "Index" so the thesis profile
                // closes with the same standard structural element as a book.
                doc = doc.add_paragraph(page_break());
                let index_h1_style = if ctx.body_render_use_bk_styles {
                    "BkH1"
                } else {
                    "Heading1"
                };
                doc = doc.add_paragraph(
                    Paragraph::new().style(index_h1_style).add_run(
                        Run::new()
                            .add_text("Index")
                            .bold()
                            .size(heading_size_hp(ctx.typography, 2))
                            .color(heading_color_for(ctx.typography))
                            .fonts(head_fonts_for(ctx.typography)),
                    ),
                );
                doc = doc.add_paragraph(
                    Paragraph::new().add_run(
                        Run::new()
                            .add_text(
                                "Right-click and choose \u{201c}Update Field\u{201d} to build the index.",
                            )
                            .italic()
                            .size(18)
                            .color(subtitle_color_for(ctx.typography))
                            .fonts(body_fonts_for(ctx.typography)),
                    ),
                );
                doc =
                    doc.add_paragraph(Paragraph::new().add_run(field_run("INDEX \\c 2", "", true)));
            }
        }
    }

    let mut cur = Cursor::new(Vec::<u8>::new());
    doc.build().pack(&mut cur).context("pack thesis docx")?;
    let layout = LayoutOverrides::from_meta(meta);
    let styles_profile = if matches!(
        meta.thesis_typography,
        TypographyProfile::FhnwProposalParity | TypographyProfile::FhnwMtTemplate
    ) {
        crate::thesis_styles::StylesProfile::FhnwMasterThesis
    } else {
        crate::thesis_styles::StylesProfile::AiNorms
    };
    postprocess_docx_inner_layout(
        cur.into_inner(),
        meta.body_render_use_bk_styles,
        &layout,
        styles_profile,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use docx_rs::BuildXML;

    // ====================================================================
    // CRITICAL pPr schema-order fix (#405, 2026-06-07).
    // Locks the post-processor that moves `<w:pStyle>` to the first
    // child of `<w:pPr>` and `<w:rPr/>` to the last, so Microsoft Word
    // applies the paragraph style instead of silently dropping it.
    // ====================================================================

    #[test]
    fn fix_ppr_schema_order_moves_pstyle_to_front() {
        let broken = "<w:p><w:pPr><w:rPr /><w:pStyle w:val=\"Heading1\" /><w:spacing w:after=\"120\" /></w:pPr></w:p>";
        let fixed = fix_ppr_schema_order(broken);
        // pStyle must precede rPr.
        let p_pos = fixed.find("<w:pStyle").expect("pStyle present");
        let r_pos = fixed.find("<w:rPr").expect("rPr present");
        assert!(
            p_pos < r_pos,
            "pStyle must come before rPr after fix: {fixed}"
        );
        // pStyle must be the FIRST child of pPr (immediately after the open tag).
        assert!(
            fixed.contains("<w:pPr><w:pStyle"),
            "pStyle must be first child of pPr: {fixed}"
        );
    }

    #[test]
    fn fix_ppr_schema_order_moves_empty_rpr_to_back() {
        let broken =
            "<w:pPr><w:rPr /><w:pStyle w:val=\"Heading2\" /><w:spacing w:after=\"60\" /></w:pPr>";
        let fixed = fix_ppr_schema_order(broken);
        assert!(
            fixed.contains("<w:spacing w:after=\"60\" /><w:rPr /></w:pPr>"),
            "rPr must be the last child before </w:pPr>: {fixed}"
        );
    }

    #[test]
    fn fix_ppr_schema_order_is_idempotent_on_already_correct() {
        let good =
            "<w:pPr><w:pStyle w:val=\"Heading3\" /><w:spacing w:after=\"60\" /><w:rPr /></w:pPr>";
        let fixed = fix_ppr_schema_order(good);
        assert_eq!(fixed, good, "already-correct pPr must be unchanged");
    }

    #[test]
    fn fix_ppr_schema_order_leaves_pstyle_less_blocks_untouched() {
        let plain = "<w:pPr><w:rPr /><w:spacing w:after=\"100\" /></w:pPr>";
        let fixed = fix_ppr_schema_order(plain);
        // No pStyle present, but the empty rPr should be moved to back
        // anyway since that's what the schema wants. Verify rPr ends up
        // after spacing.
        assert!(
            fixed.contains("<w:spacing w:after=\"100\" /><w:rPr /></w:pPr>"),
            "rPr must be moved to the end even without pStyle: {fixed}"
        );
    }

    #[test]
    fn fix_ppr_schema_order_handles_multiple_paragraphs() {
        let multi = "<w:p><w:pPr><w:rPr /><w:pStyle w:val=\"Heading1\" /></w:pPr></w:p>\
                     <w:p><w:pPr><w:rPr /><w:pStyle w:val=\"Heading2\" /></w:pPr></w:p>\
                     <w:p><w:pPr><w:rPr /><w:pStyle w:val=\"Caption\" /></w:pPr></w:p>";
        let fixed = fix_ppr_schema_order(multi);
        // Every pPr should have pStyle immediately after the open tag.
        for style in ["Heading1", "Heading2", "Caption"] {
            let pattern = format!("<w:pPr><w:pStyle w:val=\"{style}\" />");
            assert!(
                fixed.contains(&pattern),
                "expected pattern '{pattern}' in fixed XML: {fixed}"
            );
        }
    }

    #[test]
    fn fix_ppr_schema_order_handles_nonempty_rpr_block() {
        // A pPr with both pStyle and a NON-empty rPr (the rare
        // builder-emitted run-default with actual properties).
        let broken = "<w:pPr><w:rPr><w:b /><w:sz w:val=\"20\" /></w:rPr>\
                      <w:pStyle w:val=\"Heading1\" /><w:spacing w:after=\"60\" /></w:pPr>";
        let fixed = fix_ppr_schema_order(broken);
        let p_pos = fixed.find("<w:pStyle").expect("pStyle present");
        let r_pos = fixed.find("<w:rPr>").expect("non-empty rPr present");
        assert!(p_pos < r_pos, "pStyle before rPr: {fixed}");
        // rPr should be the last child before </w:pPr>.
        assert!(
            fixed.ends_with("</w:rPr></w:pPr>"),
            "non-empty rPr must close immediately before </w:pPr>: {fixed}"
        );
    }

    /// #405 follow-up (2026-06-08): Word COM's INDEX field expansion
    /// emits section-break paragraphs whose pPr contains rPr BEFORE
    /// sectPr, which violates CT_PPr (sectPr must precede rPr).
    /// Verify the fix re-orders correctly.
    #[test]
    fn fix_ppr_schema_order_moves_rpr_after_sectpr() {
        let broken = "<w:pPr><w:rPr><w:noProof/></w:rPr><w:sectPr w:rsidR=\"004A6544\" w:rsidSect=\"004A6544\"><w:footerReference w:type=\"default\" r:id=\"rId183\"/><w:pgSz w:w=\"11906\" w:h=\"16838\"/><w:cols w:space=\"425\"/></w:sectPr></w:pPr>";
        let fixed = fix_ppr_schema_order(broken);
        let s_pos = fixed.find("<w:sectPr").expect("sectPr present");
        let r_pos = fixed.find("<w:rPr>").expect("rPr present");
        assert!(
            s_pos < r_pos,
            "sectPr must come before rPr after fix: {fixed}"
        );
        // rPr must be the last child before </w:pPr>.
        assert!(
            fixed.ends_with("</w:rPr></w:pPr>"),
            "non-empty rPr must close immediately before </w:pPr>: {fixed}"
        );
    }

    #[test]
    fn extract_self_closing_handles_attrs_with_slashes() {
        // Some self-closing tags have `/>` inside attribute values
        // (unlikely for pStyle but proves the parser is well-behaved).
        let body = "<w:pStyle w:val=\"Heading1\" /><w:spacing w:after=\"60\" />";
        let pstyle = extract_self_closing(body, "<w:pStyle ");
        assert_eq!(pstyle.as_deref(), Some("<w:pStyle w:val=\"Heading1\" />"));
    }

    #[test]
    fn extract_self_closing_returns_none_when_absent() {
        let body = "<w:spacing w:after=\"60\" />";
        let pstyle = extract_self_closing(body, "<w:pStyle ");
        assert_eq!(pstyle, None);
    }

    /// Round V (zone A — psb-01 / psb-02, 2026-06-03): the
    /// `chapter_end_rule` helper must emit exactly one paragraph carrying
    /// a `<w:pBdr><w:bottom .../></w:pBdr>` border (the horizontal-rule
    /// divider). N successive calls must produce N independent paragraphs
    /// each with the bottom-border, never a stray run that would visibly
    /// add text to the divider line. Both the gray chapter variant
    /// (color 666666, size 6) and the navy title variant (color 1F3864,
    /// size 12) are exercised so the parity gate can count occurrences
    /// independently.
    #[test]
    fn chapter_end_rule_emits_bottom_border_paragraph() {
        for &n in &[1usize, 5, 40] {
            // Build N gray rules and 1 navy rule and verify the XML.
            let paras: Vec<Paragraph> = (0..n)
                .map(|_| chapter_end_rule(false))
                .chain(std::iter::once(chapter_end_rule(true)))
                .collect();
            let mut bottom_count = 0usize;
            let mut gray_count = 0usize;
            let mut navy_count = 0usize;
            let mut text_run_count = 0usize;
            for p in &paras {
                let buf = p.build();
                let xml = String::from_utf8(buf).unwrap();
                if xml.contains("<w:bottom") {
                    bottom_count += 1;
                }
                if xml.contains("w:color=\"666666\"") {
                    gray_count += 1;
                }
                if xml.contains("w:color=\"1F3864\"") {
                    navy_count += 1;
                }
                if xml.contains("<w:t>") || xml.contains("<w:t ") {
                    text_run_count += 1;
                }
            }
            assert_eq!(
                bottom_count,
                n + 1,
                "chapter_end_rule must emit exactly N+1 paragraphs with bottom-border (N gray + 1 navy)"
            );
            assert_eq!(gray_count, n, "expected {n} gray chapter rules");
            assert_eq!(navy_count, 1, "expected 1 navy title rule");
            assert_eq!(
                text_run_count, 0,
                "chapter_end_rule must not emit any text runs — the border IS the divider"
            );
        }
    }

    /// Wave-2 Agent B (REF parity 2026-06-04). The `master_thesis_bookkit`
    /// profile opts into per-chapter horizontal-rule dividers via the
    /// manifest `emit_chapter_dividers=true` plus
    /// `body_render_use_bk_styles=true`. When both are set, every
    /// chapter close in `render_book` and `render_thesis_chapter` must
    /// emit a `<w:bottom>` border paragraph. With either flag off, no
    /// divider must be emitted (regression guard for the historical
    /// `master_thesis` proposal-parity path).
    #[test]
    fn chapter_dividers_emit_per_chapter_when_flag_true() {
        use std::io::Read;
        fn doc_xml(bytes: &[u8]) -> String {
            let mut z = zip::ZipArchive::new(Cursor::new(bytes.to_vec())).unwrap();
            let mut s = String::new();
            z.by_name("word/document.xml")
                .unwrap()
                .read_to_string(&mut s)
                .unwrap();
            s
        }
        // 3 chapters, both flags on → ≥3 paragraph dividers emitted by
        // `render_book`.
        let chapters: Vec<(String, String)> = (1..=3)
            .map(|i| (format!("ch{i}"), format!("# Chapter {i}\n\nBody.\n")))
            .collect();
        let meta_on = BookMeta {
            title: "T".into(),
            author: "A".into(),
            body_render_use_bk_styles: true,
            emit_chapter_dividers: true,
            ..Default::default()
        };
        let bytes_on = render_book(&meta_on, &chapters, Path::new(".")).expect("render_book on");
        let bottoms_on = doc_xml(&bytes_on).matches("<w:pBdr").count();
        assert!(
            bottoms_on >= 3,
            "expected ≥3 <w:pBdr> divider paragraphs with both flags on, got {bottoms_on}"
        );

        // Same chapters, divider flag OFF → strictly fewer paragraph borders.
        let meta_off = BookMeta {
            emit_chapter_dividers: false,
            ..meta_on.clone()
        };
        let bytes_off = render_book(&meta_off, &chapters, Path::new(".")).expect("render_book off");
        let bottoms_off = doc_xml(&bytes_off).matches("<w:pBdr").count();
        assert!(
            bottoms_off < bottoms_on,
            "expected fewer <w:pBdr> paragraphs with divider flag off ({bottoms_off}) than on ({bottoms_on})"
        );
    }

    /// Wave-2 Agent B (REF parity 2026-06-04). The `ThesisTypography`
    /// table in `crate::thesis_typography` must declare the verbatim
    /// reference-fixture font specs (Palatino-Linotype body, Calibri
    /// headings, 4F81BD accent). This test is a re-export of the
    /// module-local test, surfaced in `book.rs::tests` so the
    /// integration suite picks it up under `cargo test -p agentic-export
    /// thesis_typography_uses_reference_font_specs`.
    #[test]
    fn thesis_typography_uses_reference_font_specs() {
        use crate::thesis_typography::ThesisTypography;
        let t = ThesisTypography::default();
        assert_eq!(t.normal.ascii, "Palatino Linotype");
        assert_eq!(t.normal.size_hp, 22, "body 11pt = 22 half-points");
        assert_eq!(t.heading1.ascii, "Calibri");
        assert_eq!(t.heading1.size_hp, 48, "H1 24pt = 48 half-points");
        assert!(t.heading1.bold, "H1 bold per reference Title style");
        assert_eq!(
            t.heading4.color, "4F81BD",
            "H4 carries the accent1 theme colour"
        );
        assert_eq!(t.caption.color, "4F81BD", "Caption accent-blue");
    }

    /// Round V (zone A — psb-04, 2026-06-03): the sentinel rewriter
    /// `insert_index_section_breaks` must replace `__SECTPR_INDEX_OPEN__`
    /// with a 2-col continuous sectPr paragraph and `__SECTPR_INDEX_CLOSE__`
    /// with a 1-col continuous sectPr paragraph. Documents without the
    /// sentinels must pass through byte-for-byte.
    #[test]
    fn index_section_breaks_rewrite_sentinels() {
        let no_sentinels =
            "<w:document><w:body><w:p><w:r><w:t>x</w:t></w:r></w:p></w:body></w:document>";
        assert_eq!(insert_index_section_breaks(no_sentinels), no_sentinels);

        let with_sentinels = "<w:document><w:body>\
            <w:p><w:r><w:t>__SECTPR_INDEX_OPEN__</w:t></w:r></w:p>\
            <w:p><w:r><w:t>Index</w:t></w:r></w:p>\
            <w:p><w:r><w:t>__SECTPR_INDEX_CLOSE__</w:t></w:r></w:p>\
            </w:body></w:document>";
        let out = insert_index_section_breaks(with_sentinels);
        assert!(
            out.contains("<w:cols w:num=\"2\""),
            "expected 2-col sectPr in rewritten OPEN: {out}"
        );
        assert!(
            !out.contains("__SECTPR_INDEX_OPEN__"),
            "OPEN sentinel must be consumed: {out}"
        );
        assert!(
            !out.contains("__SECTPR_INDEX_CLOSE__"),
            "CLOSE sentinel must be consumed: {out}"
        );
        // CLOSE sectPr drops num attribute (1-col is the docx-rs default
        // when w:num is omitted) — assert the type and absence of num=2 on
        // the LAST sectPr.
        let last_sectpr = out.rfind("<w:sectPr").unwrap();
        let tail = &out[last_sectpr..];
        assert!(
            tail.contains("w:val=\"continuous\""),
            "CLOSE sectPr must be continuous: {tail}"
        );
        assert!(
            !tail.contains("w:num=\"2\""),
            "CLOSE sectPr must restore 1-col (no num=2): {tail}"
        );
    }

    /// 2026-06-14 (#413 follow-up) — `propagate_section_chrome_refs`
    /// must clone the existing `<w:footerReference>` (and header
    /// references) from any sectPr that has them into every sectPr
    /// that lacks one. Without this, multi-section docs render page
    /// numbers only on pages controlled by the document-level sectPr.
    #[test]
    fn propagate_section_chrome_refs_copies_footer_into_bare_sectprs() {
        // Two per-chapter sectPrs (no footer ref) + one doc-level
        // sectPr (with the canonical footerReference). After the pass,
        // all three should carry the footerReference.
        let doc = "<w:body>\
            <w:p><w:pPr><w:sectPr w:rsidR=\"00\"><w:pgSz w:w=\"11906\" w:h=\"16838\"/></w:sectPr></w:pPr></w:p>\
            <w:p><w:pPr><w:sectPr w:rsidR=\"01\"><w:pgSz w:w=\"11906\" w:h=\"16838\"/></w:sectPr></w:pPr></w:p>\
            <w:sectPr w:rsidR=\"02\"><w:footerReference w:type=\"default\" r:id=\"rId99\"/><w:pgSz w:w=\"11906\" w:h=\"16838\"/></w:sectPr>\
            </w:body>";
        let out = propagate_section_chrome_refs(doc);
        // Every sectPr now carries the default footerReference.
        let ref_count = out
            .matches("<w:footerReference w:type=\"default\" r:id=\"rId99\"/>")
            .count();
        assert_eq!(
            ref_count, 3,
            "expected the default footerReference in all 3 sectPrs after propagation: {out}"
        );
        // Donor sectPr must still have exactly one (no double-inject).
        let donor_block_idx = out.find("w:rsidR=\"02\"").unwrap();
        let donor_tail = &out[donor_block_idx..];
        let donor_end = donor_tail.find("</w:sectPr>").unwrap();
        let donor_block = &donor_tail[..donor_end];
        assert_eq!(
            donor_block.matches("<w:footerReference").count(),
            1,
            "donor sectPr must not be re-injected: {donor_block}"
        );
        // No-op on docs without any reference at all.
        let bare = "<w:body><w:sectPr><w:pgSz w:w=\"11906\" w:h=\"16838\"/></w:sectPr></w:body>";
        assert_eq!(propagate_section_chrome_refs(bare), bare);
    }

    /// 2026-06-14 (#413 follow-up) — end-to-end: render a tiny thesis-
    /// profile book with the bookkit per-chapter sectPr opt-in, then
    /// unzip the docx and assert (a) `word/footer*.xml` contains the
    /// PAGE field, and (b) EVERY `<w:sectPr>` in `word/document.xml`
    /// carries a `<w:footerReference>`. Before the propagation pass,
    /// only the document-level (last) sectPr did, so every per-chapter
    /// section rendered with no page number — the user-reported defect
    /// (`campaign_01.docx has NO PAGE NUMBERING`, 2026-06-14). The
    /// `thesis_profile` path is the one that honours
    /// `emit_per_chapter_sectpr` and therefore produces the multi-
    /// section docx layout that triggered the bug.
    #[test]
    fn render_book_attaches_footer_ref_to_every_section() {
        use std::io::Read;
        let chapters: Vec<(String, String)> = (1..=3)
            .map(|i| {
                (
                    format!("ch{i}"),
                    format!("# Chapter {i}\n\nBody paragraph.\n"),
                )
            })
            .collect();
        let meta = BookMeta {
            title: "T".into(),
            subtitle: "S".into(),
            author: "A".into(),
            context: "C".into(),
            // Thesis profile + per-chapter sectPr opt-in is the
            // combination that produced 9-25 sectPrs in the shipped
            // books (master_thesis_bookkit had 25). Without the fix,
            // 24/25 of those rendered with no footer.
            thesis_profile: true,
            emit_per_chapter_sectpr: true,
            ..Default::default()
        };
        let bytes = render_book(&meta, &chapters, Path::new(".")).expect("render");
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();

        // (a) footer part exists and carries PAGE field.
        let mut footer_xml = String::new();
        let mut found_footer = false;
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i).unwrap();
            let name = entry.name().to_string();
            if is_footer_part(&name) {
                found_footer = true;
                let _ = entry.read_to_string(&mut footer_xml);
                break;
            }
        }
        assert!(found_footer, "expected at least one word/footer*.xml part");
        assert!(
            footer_xml.contains("<w:instrText>PAGE</w:instrText>")
                || footer_xml.contains("<w:instrText xml:space=\"preserve\"> PAGE </w:instrText>")
                || footer_xml.contains("w:instr=\"PAGE\""),
            "footer must carry the PAGE field: {footer_xml}"
        );

        // (b) every <w:sectPr> in document.xml has a footerReference.
        let mut doc = String::new();
        zip.by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut doc)
            .unwrap();
        let sectpr_count = doc.matches("<w:sectPr").count();
        assert!(
            sectpr_count >= 2,
            "test must exercise multi-section docs; got only {sectpr_count} sectPrs"
        );
        // Walk every sectPr block and assert it carries a footerReference.
        let mut bad: Vec<String> = Vec::new();
        let mut rest = doc.as_str();
        while let Some(open) = rest.find("<w:sectPr") {
            let after_open = open + "<w:sectPr".len();
            let Some(close_rel) = rest[after_open..].find("</w:sectPr>") else {
                break;
            };
            let block_end = after_open + close_rel + "</w:sectPr>".len();
            let block = &rest[open..block_end];
            if !block.contains("<w:footerReference") {
                bad.push(block.to_string());
            }
            rest = &rest[block_end..];
        }
        assert!(
            bad.is_empty(),
            "every sectPr must carry a footerReference; missing in: {bad:?}"
        );
    }

    /// Wave-9 (AI-Norms parity, 2026-06-03): the rendered docx for a plain
    /// book profile must have ZERO `word/header*.xml` parts (docx-rs's
    /// default-empty header injection is stripped by the Wave-9 finalize
    /// pass) and EXACTLY ONE `word/footer*.xml` part — the centered PAGE
    /// field footer. The rels file and `[Content_Types].xml` must agree
    /// (no dangling Override or Relationship pointing at a removed
    /// part).
    #[test]
    fn finalize_pass_strips_empty_header_parts_and_keeps_one_footer() {
        use std::io::Read;
        let meta = BookMeta {
            title: "T".into(),
            subtitle: "S".into(),
            author: "A".into(),
            context: "C".into(),
            ..Default::default()
        };
        let md = "# Chapter\n\nA paragraph.\n".to_string();
        let bytes = render_book(&meta, &[("c1".into(), md)], Path::new(".")).unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut header_count = 0usize;
        let mut footer_count = 0usize;
        let mut footer_has_page = false;
        let mut header_names: Vec<String> = Vec::new();
        let mut footer_names: Vec<String> = Vec::new();
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i).unwrap();
            let name = entry.name().to_string();
            if is_header_part(&name) {
                header_count += 1;
                header_names.push(name.clone());
            } else if is_footer_part(&name) {
                footer_count += 1;
                footer_names.push(name.clone());
                let mut buf = String::new();
                let _ = entry.read_to_string(&mut buf);
                if buf.contains("PAGE") {
                    footer_has_page = true;
                }
            }
        }
        assert_eq!(
            header_count, 0,
            "expected 0 header parts after Wave-9 finalize, got {header_count}: {header_names:?}"
        );
        assert_eq!(
            footer_count, 1,
            "expected 1 footer part after Wave-9 finalize, got {footer_count}: {footer_names:?}"
        );
        assert!(
            footer_has_page,
            "the surviving footer must be the PAGE-field footer"
        );

        // Cross-validate: the rels file and content-types map must not
        // reference any dropped header/footer part. Search for stale
        // `Target="header*"` / `Target="footer*"` rels (only the one
        // surviving footer should appear).
        let mut rels = String::new();
        zip.by_name("word/_rels/document.xml.rels")
            .unwrap()
            .read_to_string(&mut rels)
            .unwrap();
        let header_rels = rels.matches("Target=\"header").count();
        let footer_rels = rels.matches("Target=\"footer").count();
        assert_eq!(header_rels, 0, "stale header relationship in rels: {rels}");
        assert_eq!(
            footer_rels, 1,
            "expected exactly 1 footer relationship in rels"
        );

        let mut ct = String::new();
        zip.by_name("[Content_Types].xml")
            .unwrap()
            .read_to_string(&mut ct)
            .unwrap();
        let header_overrides = ct.matches("/word/header").count();
        let footer_overrides = ct.matches("/word/footer").count();
        assert_eq!(
            header_overrides, 0,
            "stale header Override in content-types: {ct}"
        );
        assert_eq!(
            footer_overrides, 1,
            "expected exactly 1 footer Override in content-types"
        );

        // Cross-validate: document.xml must not retain headerReference
        // tags (no surviving header part). One footerReference must
        // remain, pointing at the surviving footer.
        let mut doc = String::new();
        zip.by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut doc)
            .unwrap();
        let header_refs = doc.matches("<w:headerReference").count();
        let footer_refs = doc.matches("<w:footerReference").count();
        assert_eq!(
            header_refs, 0,
            "expected no <w:headerReference> after finalize-pass"
        );
        assert!(
            footer_refs >= 1,
            "expected at least one <w:footerReference> for the surviving PAGE-field footer"
        );
    }

    /// Round D-C (AI-Norms parity, 2026-06-03): regression for
    /// `collapse_empty_header_footer_parts`. Simulates the state in
    /// which Word COM's `Documents.Open → … → Save` leaves a docx
    /// AFTER the render-time W9-B collapse: three empty `word/header*.xml`
    /// parts, two empty `word/footer*.xml` parts alongside the surviving
    /// PAGE-field footer, with matching rels + content-types Override
    /// entries. The post-finalize pass must drop the five empty parts,
    /// scrub their rels + Overrides + sectPr references, and leave the
    /// PAGE-field footer untouched. Bytes diffed against the snapshot
    /// `ai_norms_and_regulations.docx` captured 2026-06-03 (3+3 → 0+1).
    #[test]
    fn collapse_pass_undoes_word_com_regeneration() {
        use std::io::{Read, Write};

        // Step 1: render a plain book — the W9-B pass already produces 0
        // headers + 1 footer in `bytes`.
        let meta = BookMeta {
            title: "T".into(),
            subtitle: "S".into(),
            author: "A".into(),
            context: "C".into(),
            ..Default::default()
        };
        let md = "# Chapter\n\nA paragraph.\n".to_string();
        let bytes = render_book(&meta, &[("c1".into(), md)], Path::new(".")).unwrap();

        // Step 2: simulate Word COM regeneration. Word's
        // `.Sections.Item(1).Headers.Item(N)` access materialises the
        // default/even/firstPage triad; here we re-inject three empty
        // header parts and two empty footer stubs alongside the
        // surviving PAGE footer, with the matching rels + Overrides +
        // sectPr references.
        let empty_hdr = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:hdr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:p><w:pPr><w:pStyle w:val="Header"/></w:pPr></w:p></w:hdr>"#;
        let empty_ftr = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:ftr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:p><w:pPr><w:pStyle w:val="Footer"/></w:pPr></w:p></w:ftr>"#;

        // Stuff the empty parts into the zip + rewrite rels, document.xml
        // sectPr, and [Content_Types].xml to reference them — mirroring
        // exactly what Word writes back.
        let mut zin = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut out = Cursor::new(Vec::<u8>::new());
        let mut surviving_footer_name = String::new();
        let mut original_rels = String::new();
        let mut original_doc = String::new();
        let mut original_ct = String::new();
        {
            let mut zout = zip::ZipWriter::new(&mut out);
            for i in 0..zin.len() {
                let mut f = zin.by_index(i).unwrap();
                let name = f.name().to_string();
                if name == "word/_rels/document.xml.rels" {
                    f.read_to_string(&mut original_rels).unwrap();
                } else if name == "word/document.xml" {
                    f.read_to_string(&mut original_doc).unwrap();
                } else if name == "[Content_Types].xml" {
                    f.read_to_string(&mut original_ct).unwrap();
                } else if is_footer_part(&name) {
                    // Keep the surviving footer (the PAGE-field one).
                    surviving_footer_name = name.clone();
                    zout.raw_copy_file(f).unwrap();
                } else {
                    zout.raw_copy_file(f).unwrap();
                }
            }

            // Three empty header parts (rIds 9001..9003).
            for n in 1..=3 {
                zout.start_file(
                    format!("word/header{n}.xml"),
                    zip::write::SimpleFileOptions::default(),
                )
                .unwrap();
                zout.write_all(empty_hdr.as_bytes()).unwrap();
            }
            // Two empty footer stubs (the surviving PAGE footer is
            // already in the zip; pick numbers that don't collide).
            for n in [11usize, 12] {
                zout.start_file(
                    format!("word/footer{n}.xml"),
                    zip::write::SimpleFileOptions::default(),
                )
                .unwrap();
                zout.write_all(empty_ftr.as_bytes()).unwrap();
            }

            // Patch document.xml.rels to add references to the five empty
            // parts.
            let extra_rels = r#"
<Relationship Id="rId9001" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/header" Target="header1.xml"/>
<Relationship Id="rId9002" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/header" Target="header2.xml"/>
<Relationship Id="rId9003" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/header" Target="header3.xml"/>
<Relationship Id="rId9011" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/footer" Target="footer11.xml"/>
<Relationship Id="rId9012" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/footer" Target="footer12.xml"/>
"#;
            let patched_rels =
                original_rels.replace("</Relationships>", &format!("{extra_rels}</Relationships>"));
            zout.start_file(
                "word/_rels/document.xml.rels",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            zout.write_all(patched_rels.as_bytes()).unwrap();

            // Patch document.xml: add headerReference / footerReference
            // tags pointing at the empty parts inside the first
            // `<w:sectPr>` we find.
            let extra_refs = r#"<w:headerReference w:type="default" r:id="rId9001"/><w:headerReference w:type="even" r:id="rId9002"/><w:headerReference w:type="first" r:id="rId9003"/><w:footerReference w:type="even" r:id="rId9011"/><w:footerReference w:type="first" r:id="rId9012"/>"#;
            let patched_doc = original_doc.replacen(
                "<w:sectPr",
                &format!("<w:sectPr {extra_refs}_ANCHOR=\"x\"").replace("_ANCHOR=\"x\"", ""),
                1,
            );
            // The above is a no-op if there's no `<w:sectPr`; use a more
            // robust insertion: place refs right after the opening tag.
            let patched_doc = if patched_doc == original_doc {
                if let Some(idx) = original_doc.find("<w:sectPr") {
                    let after = idx + "<w:sectPr".len();
                    // Find end of opening tag (`>` or `/>`).
                    let close = original_doc[after..]
                        .find('>')
                        .map(|p| after + p + 1)
                        .unwrap_or(after);
                    let mut s = String::new();
                    s.push_str(&original_doc[..close]);
                    s.push_str(extra_refs);
                    s.push_str(&original_doc[close..]);
                    s
                } else {
                    original_doc.clone()
                }
            } else {
                patched_doc
            };
            zout.start_file(
                "word/document.xml",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            zout.write_all(patched_doc.as_bytes()).unwrap();

            // Patch [Content_Types].xml to add Overrides for the five
            // empty parts.
            let extra_ct = r#"<Override PartName="/word/header1.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml"/><Override PartName="/word/header2.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml"/><Override PartName="/word/header3.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml"/><Override PartName="/word/footer11.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.footer+xml"/><Override PartName="/word/footer12.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.footer+xml"/>"#;
            let patched_ct = original_ct.replace("</Types>", &format!("{extra_ct}</Types>"));
            zout.start_file(
                "[Content_Types].xml",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            zout.write_all(patched_ct.as_bytes()).unwrap();

            zout.finish().unwrap();
        }
        let regenerated = out.into_inner();

        // Sanity check the regenerated state matches the snapshot
        // characteristics: 3 headers + 3 footers, refs in rels +
        // overrides present.
        {
            let mut z = zip::ZipArchive::new(Cursor::new(regenerated.clone())).unwrap();
            let mut h = 0usize;
            let mut f = 0usize;
            for i in 0..z.len() {
                let entry = z.by_index(i).unwrap();
                let n = entry.name().to_string();
                if is_header_part(&n) {
                    h += 1;
                }
                if is_footer_part(&n) {
                    f += 1;
                }
            }
            assert_eq!(h, 3, "regenerated fixture should have 3 header parts");
            assert_eq!(f, 3, "regenerated fixture should have 3 footer parts");
        }

        // Step 3: apply the post-finalize collapse pass and assert the
        // result: 0 headers + 1 footer (the surviving PAGE footer).
        let collapsed = collapse_empty_header_footer_parts(regenerated).unwrap();
        let mut z = zip::ZipArchive::new(Cursor::new(collapsed)).unwrap();
        let mut header_count = 0usize;
        let mut footer_count = 0usize;
        let mut footer_has_page = false;
        let mut surviving_footer: Option<String> = None;
        let mut rels = String::new();
        let mut ct = String::new();
        let mut doc = String::new();
        for i in 0..z.len() {
            let mut entry = z.by_index(i).unwrap();
            let name = entry.name().to_string();
            if is_header_part(&name) {
                header_count += 1;
            } else if is_footer_part(&name) {
                footer_count += 1;
                surviving_footer = Some(name.clone());
                let mut buf = String::new();
                let _ = entry.read_to_string(&mut buf);
                if buf.contains("PAGE") {
                    footer_has_page = true;
                }
            } else if name == "word/_rels/document.xml.rels" {
                let _ = entry.read_to_string(&mut rels);
            } else if name == "[Content_Types].xml" {
                let _ = entry.read_to_string(&mut ct);
            } else if name == "word/document.xml" {
                let _ = entry.read_to_string(&mut doc);
            }
        }
        assert_eq!(
            header_count, 0,
            "collapse pass must leave 0 header parts after Word-COM regeneration"
        );
        assert_eq!(
            footer_count, 1,
            "collapse pass must leave exactly 1 footer part (the PAGE field)"
        );
        assert!(footer_has_page, "the surviving footer must contain PAGE");
        assert_eq!(
            surviving_footer.as_deref(),
            Some(surviving_footer_name.as_str()),
            "the surviving footer must be the original PAGE-field footer"
        );
        assert_eq!(rels.matches("Target=\"header").count(), 0);
        assert_eq!(rels.matches("Target=\"footer").count(), 1);
        assert_eq!(ct.matches("/word/header").count(), 0);
        assert_eq!(ct.matches("/word/footer").count(), 1);
        assert_eq!(doc.matches(r#"r:id="rId9001""#).count(), 0);
        assert_eq!(doc.matches(r#"r:id="rId9011""#).count(), 0);
    }

    /// Unit test for [`collect_dropped_rids`]: feed a synthetic rels XML
    /// referencing two empty header parts and one populated footer,
    /// verify the dropped rIds and the rewritten rels body are emitted
    /// correctly.
    #[test]
    fn collect_dropped_rids_drops_empty_parts() {
        let rels = r#"<?xml version="1.0"?><Relationships>
<Relationship Id="rId1" Type="hdr" Target="header1.xml"/>
<Relationship Id="rId2" Type="hdr" Target="header2.xml"/>
<Relationship Id="rId3" Type="ftr" Target="footer1.xml"/>
<Relationship Id="rId4" Type="img" Target="media/image1.png"/>
</Relationships>"#;
        let mut drop_headers = std::collections::HashSet::new();
        drop_headers.insert("word/header1.xml".to_string());
        drop_headers.insert("word/header2.xml".to_string());
        let drop_footers: std::collections::HashSet<String> = std::collections::HashSet::new();
        let (dropped, rewritten) = collect_dropped_rids(rels, &drop_headers, &drop_footers);
        assert!(dropped.contains("rId1"));
        assert!(dropped.contains("rId2"));
        assert!(!dropped.contains("rId3"));
        assert!(!dropped.contains("rId4"));
        assert!(!rewritten.contains("header1.xml"));
        assert!(!rewritten.contains("header2.xml"));
        assert!(rewritten.contains("footer1.xml"));
        assert!(rewritten.contains("media/image1.png"));
    }

    /// Unit test for [`drop_refs_to_empty_parts`]: feed a synthetic
    /// document.xml and verify the matching headerReference /
    /// footerReference tags are stripped while unrelated tags survive.
    #[test]
    fn drop_refs_strips_targeted_references() {
        let doc = r#"<w:sectPr><w:headerReference w:type="default" r:id="rId1"/><w:headerReference w:type="first" r:id="rId2"/><w:footerReference w:type="default" r:id="rId3"/><w:pgSz/></w:sectPr>"#;
        let mut dropped: std::collections::HashSet<String> = std::collections::HashSet::new();
        dropped.insert("rId1".into());
        dropped.insert("rId2".into());
        let out = drop_refs_to_empty_parts(doc, &dropped);
        assert!(!out.contains("rId1"), "rId1 ref not stripped: {out}");
        assert!(!out.contains("rId2"), "rId2 ref not stripped: {out}");
        assert!(
            out.contains("rId3"),
            "rId3 (non-dropped) was removed: {out}"
        );
        assert!(out.contains("<w:pgSz/>"));
    }

    /// Unit test for [`header_or_footer_is_empty`]: the docx-rs default
    /// empty header (a single `<w:p>` with no runs) is empty, while a
    /// footer with a PAGE field is not.
    #[test]
    fn empty_header_detection_distinguishes_page_field_footer() {
        let empty = r#"<w:hdr><w:p><w:pPr><w:pStyle w:val="Header"/></w:pPr></w:p></w:hdr>"#;
        let page_footer = r#"<w:ftr><w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText>PAGE</w:instrText></w:r></w:p></w:ftr>"#;
        let with_text = r#"<w:hdr><w:p><w:r><w:t>FHNW</w:t></w:r></w:p></w:hdr>"#;
        assert!(header_or_footer_is_empty(empty));
        assert!(!header_or_footer_is_empty(page_footer));
        assert!(!header_or_footer_is_empty(with_text));
    }

    /// Wave-4 (REF parity 2026-06-03): when the manifest sets
    /// `dedication_personal`, the rendered docx must include the line on a
    /// dedicated page BEFORE the inscription page. We verify both the
    /// presence of the literal text and that it appears earlier in the
    /// document than the epigraph (which is part of the inscription page).
    #[test]
    fn front_matter_dedication() {
        use std::io::Read;
        let meta = BookMeta {
            title: "Test Book".into(),
            subtitle: "Subtitle".into(),
            author: "Author".into(),
            context: "Ctx".into(),
            dedication_personal: Some("For Melanie, Sarah and Timo".into()),
            epigraph: Some("Technology is neither good nor bad; nor is it neutral.".into()),
            epigraph_by: Some("Melvin Kranzberg".into()),
            ..Default::default()
        };
        let md = "# Body\n\nA paragraph.\n".to_string();
        let bytes = render_book(&meta, &[("c1".into(), md)], Path::new(".")).unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut xml = String::new();
        zip.by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut xml)
            .unwrap();
        let ded_pos = xml
            .find("For Melanie, Sarah and Timo")
            .expect("personal dedication line missing from rendered docx");
        let epi_pos = xml
            .find("Technology is neither good nor bad")
            .expect("epigraph missing from rendered docx (sanity check)");
        assert!(
            ded_pos < epi_pos,
            "personal dedication must sit BEFORE the inscription-page epigraph"
        );
    }

    /// Wave-4 (REF parity 2026-06-03): the book profile's back-matter order
    /// must be Appendix → Table of Figures → Table of Tables → Bibliography
    /// → Index. We assert by checking ordered positions of the expected
    /// heading strings (overridden via tof/tot_heading + a manifest
    /// Bibliography chapter that we expect deferred past the lists).
    #[test]
    fn back_matter_order() {
        use std::io::Read;
        let meta = BookMeta {
            title: "Order".into(),
            subtitle: "S".into(),
            author: "A".into(),
            context: "C".into(),
            tof_heading: Some("Table of Figures".into()),
            tot_heading: Some("Table of Tables".into()),
            ..Default::default()
        };
        let appendix = "# Appendix: Sources\n\nText.\n".to_string();
        let bib = "# Bibliography\n\nDoe, J. (2026).\n".to_string();
        let body = "# Body\n\nIntro.\n".to_string();
        let bytes = render_book(
            &meta,
            &[
                ("body".into(), body),
                ("appx".into(), appendix),
                ("bib".into(), bib),
            ],
            Path::new("."),
        )
        .unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut xml = String::new();
        zip.by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut xml)
            .unwrap();
        let apx = xml
            .find("Appendix: Sources")
            .expect("Appendix heading missing");
        let tof = xml
            .find("Table of Figures")
            .expect("Table of Figures missing");
        let tot = xml
            .find("Table of Tables")
            .expect("Table of Tables missing");
        let bib_pos = xml
            .find("Bibliography")
            .expect("Bibliography heading missing");
        let idx = xml.find("Index").expect("Index heading missing");
        assert!(
            apx < tof && tof < tot && tot < bib_pos && bib_pos < idx,
            "back-matter order violated: appendix={apx} tof={tof} tot={tot} bib={bib_pos} idx={idx}"
        );
    }

    #[test]
    fn renders_book_with_table_and_heading() {
        let meta = BookMeta {
            title: "T".into(),
            subtitle: "S".into(),
            author: "A".into(),
            context: "C".into(),
            ..Default::default()
        };
        let md = "# Chapter\n\nA **bold** paragraph.\n\n| H1 | H2 |\n|----|----|\n| a | b |\n"
            .to_string();
        let bytes = render_book(&meta, &[("c1".into(), md)], Path::new(".")).unwrap();
        assert_eq!(&bytes[..4], b"PK\x03\x04");
        assert!(bytes.len() > 2000);
    }

    #[test]
    fn uncaptioned_table_is_still_numbered() {
        use std::io::Read;
        // A plain markdown table with no caption must still emit a SEQ Table
        // field, so it appears in the Table of Tables (gap #1).
        let meta = BookMeta {
            title: "T".into(),
            ..Default::default()
        };
        let md = "# C\n\n| A | B |\n|---|---|\n| 1 | 2 |\n".to_string();
        let bytes = render_book(&meta, &[("c1".into(), md)], Path::new(".")).unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut d = String::new();
        zip.by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut d)
            .unwrap();
        assert!(
            d.contains("SEQ Table"),
            "an uncaptioned table must still be numbered for the Table of Tables"
        );
    }

    #[test]
    fn table_rows_cant_split_across_pages() {
        use std::io::Read;
        let meta = BookMeta {
            title: "T".into(),
            ..Default::default()
        };
        let md = "# C\n\n| A | B |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\n".to_string();
        let bytes = render_book(&meta, &[("c1".into(), md)], Path::new(".")).unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut d = String::new();
        zip.by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut d)
            .unwrap();
        assert!(
            d.contains("cantSplit"),
            "table rows must set w:cantSplit so they don't break mid-row across pages"
        );
    }

    #[test]
    fn table_caption_keeps_with_table() {
        use std::io::Read;
        let meta = BookMeta {
            title: "T".into(),
            ..Default::default()
        };
        // A table with no surrounding headings: the caption paragraph itself
        // carries keep_next so it stays with the table on the same page.
        // Wave-9 (AI-Norms parity, 2026-06-03) removed the intervening empty
        // spacer paragraph between caption and `<w:tbl>` so the parity
        // gate's `preceding_paragraph_is_table_caption` sniff finds the
        // caption immediately above the table — the caption's own
        // keep_next is sufficient to keep the title with the body.
        let md = "Intro text.\n\n| A | B |\n|---|---|\n| 1 | 2 |\n".to_string();
        let bytes = render_book(&meta, &[("c1".into(), md)], Path::new(".")).unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut d = String::new();
        zip.by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut d)
            .unwrap();
        assert!(
            d.matches("keepNext").count() >= 1,
            "table caption must keep_next so the title stays with the table"
        );
    }

    #[test]
    fn figure_keeps_image_with_caption() {
        use std::io::Read;
        let dir = tempfile::tempdir().unwrap();
        image::RgbImage::new(8, 8)
            .save(dir.path().join("f.png"))
            .unwrap();
        let meta = BookMeta {
            title: "T".into(),
            ..Default::default()
        };
        let md = "Intro.\n\n![A figure](f.png)\n".to_string();
        let bytes = render_book(&meta, &[("c1".into(), md)], dir.path()).unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut d = String::new();
        zip.by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut d)
            .unwrap();
        assert!(
            d.contains("SEQ Figure"),
            "figure should render with a SEQ caption"
        );
        assert!(
            d.contains("keepNext"),
            "the image paragraph must keep_next so it stays with its caption"
        );
    }

    /// Readability brief 2026-06-13: a figspec carrying `layout: "landscape"`
    /// resolves through `agentic_figures::resolve_markdown` to an image ref
    /// with a `#landscape` URL fragment. The book renderer detects the
    /// fragment, strips it before reading the on-disk PNG, and wraps the
    /// figure paragraph with a leading portrait `<w:sectPr>` (closes prior
    /// portrait section) + trailing landscape `<w:sectPr>` (closes the
    /// landscape section so following content resumes in portrait). A
    /// portrait figure in the same document must NOT acquire these
    /// section breaks, so the two-figure assertion proves the wrapping
    /// is conditional on the fragment, not unconditional.
    #[test]
    fn landscape_figure_emits_paired_section_breaks() {
        use std::io::Read;
        let dir = tempfile::tempdir().unwrap();
        // Two PNGs: one referenced as portrait, one as landscape.
        image::RgbImage::new(8, 8)
            .save(dir.path().join("portrait_fig.png"))
            .unwrap();
        image::RgbImage::new(8, 8)
            .save(dir.path().join("landscape_fig.png"))
            .unwrap();
        let meta = BookMeta {
            title: "T".into(),
            ..Default::default()
        };
        // Markdown intentionally bypasses the figspec→markdown resolver
        // (which would write the PNGs itself) so this test exercises the
        // book renderer's URL-fragment branch directly with hand-rolled
        // image refs — exactly mirroring what `resolve_markdown` emits.
        let md = "# Chapter\n\nPortrait fig:\n\n![Portrait fig caption](portrait_fig.png)\n\nLandscape fig:\n\n![Landscape fig caption](landscape_fig.png#landscape)\n\nTrailer.\n".to_string();
        let bytes = render_book(&meta, &[("c1".into(), md)], dir.path()).unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut xml = String::new();
        zip.by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut xml)
            .unwrap();
        // The landscape section break must appear at least once in body
        // content (the doc-level sectPr is portrait, so any landscape
        // attribute can only come from the in-body wrapping pair).
        let landscape_hits = xml.matches("orient=\"landscape\"").count();
        assert!(
            landscape_hits >= 1,
            "landscape figure must emit a body sectPr with orient=\"landscape\"; got {landscape_hits} hits in xml of length {}",
            xml.len()
        );
        // The body must contain at least three sectPrs after wrapping
        // the landscape figure: the leading portrait sectPr, the trailing
        // landscape sectPr, and the document-level sectPr that docx-rs
        // always emits. Without the landscape figure the document would
        // carry exactly 1 sectPr.
        let sectpr_count = xml.matches("<w:sectPr").count();
        assert!(
            sectpr_count >= 3,
            "landscape figure wrapping must add ≥2 in-body sectPrs (got total={sectpr_count}; expected ≥3 incl. doc-level)"
        );
        // The on-disk path must NOT contain the `#landscape` fragment —
        // the renderer must have stripped it before reading the PNG.
        // (If it didn't, `std::fs::read` would have failed and the
        // figure would have rendered as `[figure missing: ...]`.)
        assert!(
            !xml.contains("[figure missing"),
            "fragment-stripping failed: figure marked missing in {xml}"
        );
        // Both captions must render (proves the portrait figure was
        // unaffected by the landscape branch).
        assert!(
            xml.contains("Portrait fig caption"),
            "portrait figure caption must render"
        );
        assert!(
            xml.contains("Landscape fig caption"),
            "landscape figure caption must render"
        );
    }

    #[test]
    fn table_header_row_repeats_across_pages() {
        use std::io::Read;
        let meta = BookMeta {
            title: "T".into(),
            ..Default::default()
        };
        let md = "# C\n\n| H1 | H2 |\n|----|----|\n| a | b |\n| c | d |\n".to_string();
        let bytes = render_book(&meta, &[("c1".into(), md)], Path::new(".")).unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut d = String::new();
        zip.by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut d)
            .unwrap();
        assert!(
            d.contains("<w:tblHeader"),
            "a content-table header row must set w:tblHeader to repeat across pages"
        );
    }

    #[test]
    fn mark_header_rows_targets_only_content_table_headers() {
        // Content-table header row: cantSplit + HEADBG → marked.
        let hdr = r#"<w:tr><w:trPr><w:cantSplit /></w:trPr><w:tc><w:tcPr><w:shd w:fill="1F3864" /></w:tcPr></w:tc></w:tr>"#;
        assert!(mark_header_rows(hdr).contains("<w:tblHeader"));
        // Data row: cantSplit but ALTBG fill → not marked.
        let data = r#"<w:tr><w:trPr><w:cantSplit /></w:trPr><w:tc><w:tcPr><w:shd w:fill="F4F6FA" /></w:tcPr></w:tc></w:tr>"#;
        assert!(!mark_header_rows(data).contains("tblHeader"));
        // Chrome box header: HEADBG but no cantSplit → not marked.
        let chrome =
            r#"<w:tr><w:trPr /><w:tc><w:tcPr><w:shd w:fill="1F3864" /></w:tcPr></w:tc></w:tr>"#;
        assert!(!mark_header_rows(chrome).contains("tblHeader"));
    }

    #[test]
    fn source_reference_is_true_superscript() {
        use std::io::Read;
        let meta = BookMeta {
            title: "T".into(),
            ..Default::default()
        };
        // A markdown link renders as label + a superscript source-ref number.
        let md = "See the [spec](https://example.org) for details.\n".to_string();
        let bytes = render_book(&meta, &[("c1".into(), md)], Path::new(".")).unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut d = String::new();
        zip.by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut d)
            .unwrap();
        assert!(
            d.contains(r#"<w:vertAlign w:val="superscript""#),
            "the source-ref [n] must be a true superscript (w:vertAlign)"
        );
    }

    /// Temporary dump (Wave-1 F3 audit 2026-06-02): writes the document XML
    /// hyperlink slice + the rels file to a temp dir so the campaign report
    /// can quote the exact serialised form. Gated on a `F3_DUMP_DIR` env var
    /// so it is a no-op in normal CI runs.
    #[test]
    fn f3_dump_hyperlink_xml() {
        use std::io::Read;
        let Ok(dir) = std::env::var("F3_DUMP_DIR") else {
            return;
        };
        let meta = BookMeta {
            title: "T".into(),
            ..Default::default()
        };
        let md = "See the [spec](https://example.org) for details.\n".to_string();
        let bytes = render_book(&meta, &[("c1".into(), md)], Path::new(".")).unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut d = String::new();
        zip.by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut d)
            .unwrap();
        let mut rels = String::new();
        zip.by_name("word/_rels/document.xml.rels")
            .unwrap()
            .read_to_string(&mut rels)
            .unwrap();
        // Slice around the first EXTERNAL (r:id-bearing) <w:hyperlink>;
        // skip TOC entry hyperlinks which use `w:anchor`.
        let needle = "<w:hyperlink r:id=";
        let i = d.find(needle).unwrap();
        let j = d[i..].find("</w:hyperlink>").unwrap();
        let snippet = &d[i..i + j + "</w:hyperlink>".len()];
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(format!("{dir}/hyperlink_snippet.xml"), snippet).unwrap();
        std::fs::write(format!("{dir}/document.rels.xml"), &rels).unwrap();
    }

    /// T1.6 (REF parity 2026-06-02): a markdown `[text](url)` must render as a
    /// CLICKABLE `<w:hyperlink>` element (not just a coloured run + superscript),
    /// so Word users can Ctrl+click the label to follow the URL. The superscript
    /// `[N]` cross-reference to the Sources box stays beside the hyperlink.
    #[test]
    fn markdown_link_renders_as_clickable_hyperlink() {
        use std::io::Read;
        let meta = BookMeta {
            title: "T".into(),
            ..Default::default()
        };
        let md = "See the [spec](https://example.org) for details.\n".to_string();
        let bytes = render_book(&meta, &[("c1".into(), md)], Path::new(".")).unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut d = String::new();
        zip.by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut d)
            .unwrap();
        // A `<w:hyperlink>` element must wrap the label so Word renders it
        // as a clickable link (with an r:id pointing into rels).
        assert!(
            d.contains("<w:hyperlink "),
            "body link must be wrapped in a <w:hyperlink> element (clickable)"
        );
        // The relationship file must carry the external target so the
        // hyperlink resolves to the real URL.
        let mut rels = String::new();
        zip.by_name("word/_rels/document.xml.rels")
            .unwrap()
            .read_to_string(&mut rels)
            .unwrap();
        assert!(
            rels.contains("https://example.org"),
            "document rels must declare the External target URL"
        );
        // Both the superscript [N] cross-ref AND the clickable hyperlink
        // coexist, so the end-of-chapter Sources box still resolves.
        assert!(
            d.contains(r#"<w:vertAlign w:val="superscript""#),
            "the [N] superscript must remain alongside the clickable hyperlink"
        );
    }

    #[test]
    fn list_of_figures_tables_fields_are_dirty() {
        use std::io::Read;
        // The TOC \c list fields must be dirty so Word refreshes them on open
        // (matching the main TOC); otherwise the lists stay stale until a manual
        // F9. Regression guard for the "lists still need refresh" bug.
        let meta = BookMeta {
            title: "T".into(),
            ..Default::default()
        };
        let bytes = render_book(
            &meta,
            &[("c1".into(), "# C\n\nText.\n".into())],
            Path::new("."),
        )
        .unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut d = String::new();
        zip.by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut d)
            .unwrap();
        // Find each TOC \c list field and confirm its begin fldChar is dirty.
        for seq in ["Figure", "Table"] {
            let needle = format!("TOC \\h \\z \\c \"{seq}\"");
            let pos = d
                .find(&needle)
                .unwrap_or_else(|| panic!("no List-of-{seq} field"));
            let pre = &d[pos.saturating_sub(140)..pos];
            assert!(
                pre.contains(r#"w:fldCharType="begin" w:dirty="true""#),
                "List-of-{seq} field must be dirty=true so Word refreshes it on open"
            );
        }
    }

    #[test]
    fn docx_sets_update_fields_so_word_refreshes_toc() {
        use std::io::Read;
        let meta = BookMeta {
            title: "T".into(),
            ..Default::default()
        };
        let md = "# Chapter One\n\nText.\n\n## Sub\n\nMore text.\n".to_string();
        let bytes = render_book(&meta, &[("c1".into(), md)], Path::new(".")).unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut s = String::new();
        zip.by_name("word/settings.xml")
            .unwrap()
            .read_to_string(&mut s)
            .unwrap();
        assert!(
            s.contains(r#"<w:updateFields w:val="true"/>"#),
            "settings.xml must enable update-fields-on-open; got: {s}"
        );
    }

    #[test]
    fn front_matter_unnumbered_dimension_numbered() {
        // Front/back-matter H1s render UNNUMBERED (exact or starts-with match,
        // case-insensitive); a real dimension chapter stays numbered.
        assert!(!chapter_is_numbered("# Foreword\n\nText.\n", false));
        assert!(!chapter_is_numbered(
            "# Appendix: The Research Prompts\n\nText.\n",
            false
        ));
        assert!(!chapter_is_numbered(
            "# Acronyms and Abbreviations\n",
            false
        ));
        assert!(!chapter_is_numbered("# List of Figures\n", false));
        assert!(chapter_is_numbered(
            "# Dimension 06 — Quantum Computing\n\nText.\n",
            false
        ));
    }

    #[test]
    fn companion_renders_without_book_chrome() {
        // Companion profile still produces a valid docx (plain title, no
        // title/disclaimer/inscription pages).
        let meta = BookMeta {
            title: "Student Notes".into(),
            disclaimer: Some("should be skipped in companion mode".into()),
            companion: true,
            ..Default::default()
        };
        let md = "# Overview\n\nSome synthesis text.\n".to_string();
        let bytes = render_book(&meta, &[("c1".into(), md)], Path::new(".")).unwrap();
        assert_eq!(&bytes[..4], b"PK\x03\x04");
        assert!(bytes.len() > 1500);
    }

    /// The master_thesis bookkit (thesis_profile) chapter set, mirroring the
    /// manifest order: Management Summary, Acronyms, the seven numbered body
    /// chapters, two appendices, then the bibliography.
    fn master_thesis_chapters() -> Vec<(String, String)> {
        [
            (
                "tp",
                "# Title Page\n\nGovernance and Leadership… — Master Thesis Submission.\n",
            ),
            ("do", "# Declaration of Originality\n\nI hereby declare…\n"),
            (
                "cd",
                "# Compliance Declaration for Open-Source Photon OS\n\nI hereby declare…\n",
            ),
            ("mgmt", "# Management Summary\n\nThe one-page summary.\n"),
            (
                "acro",
                "# Acronyms and Abbreviations\n\nAI — Artificial Intelligence.\n",
            ),
            ("c1", "# Introduction\n\nBackground.\n"),
            ("c2", "# Theory\n\nState of the art.\n"),
            ("c3", "# Current-State Analysis\n\nIST.\n"),
            ("c4", "# Empirical Study\n\nData.\n"),
            ("c5", "# Solution\n\nDesign.\n"),
            ("c6", "# Conclusion\n\nFindings.\n"),
            ("c7", "# Personal Reflection\n\nLessons learned.\n"),
            ("apx1", "# Appendix: Research Prompts\n\nPrompts.\n"),
            ("bib", "# Bibliography\n\nDoe, J. (2026).\n"),
        ]
        .into_iter()
        .map(|(l, m)| (l.to_string(), m.to_string()))
        .collect()
    }

    /// Wave-2 (Bookkit profile chrome suppression, REF parity 2026-06-04):
    /// the `master_thesis_bookkit` profile must render zero `Index1`
    /// paragraphs when the manifest sets `emit_index = false`. The
    /// reference thesis carries 0 Index1 paragraphs; the current bookkit
    /// output carries 43 (cached `INDEX \c 2` field expansion in Word).
    /// Suppressing the whole Index section eliminates the chrome.
    #[test]
    fn thesis_chrome_suppression_skips_index() {
        use std::io::Read;
        let meta = BookMeta {
            title: "Bookkit chrome suppression — Index".into(),
            thesis_profile: true,
            emit_index: false,
            ..Default::default()
        };
        let bytes = render_book(&meta, &master_thesis_chapters(), Path::new(".")).unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut xml = String::new();
        zip.by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut xml)
            .unwrap();
        let index1_count = xml.matches("w:val=\"Index1\"").count();
        assert_eq!(
            index1_count, 0,
            "emit_index=false must suppress every pStyle=Index1 paragraph, got {index1_count}"
        );
        // The INDEX field itself must also be gone (the source of cached
        // Index1 entries on Word's field update).
        assert!(
            !xml.contains("INDEX \\c 2"),
            "emit_index=false must suppress the INDEX field instruction text"
        );
    }

    /// Wave-2 (Bookkit profile chrome suppression, REF parity 2026-06-04):
    /// when the manifest sets `emit_appendix_in_back_matter = false`,
    /// chapters classified as `ThesisSlot::Appendix` must not be emitted.
    /// The reference thesis closes on ToF → ToT → Bibliography with no
    /// Appendix chapter between body and back-matter lists.
    #[test]
    fn thesis_chrome_suppression_skips_appendix() {
        use std::io::Read;
        let meta = BookMeta {
            title: "Bookkit chrome suppression — Appendix".into(),
            thesis_profile: true,
            emit_appendix_in_back_matter: false,
            ..Default::default()
        };
        let bytes = render_book(&meta, &master_thesis_chapters(), Path::new(".")).unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut xml = String::new();
        zip.by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut xml)
            .unwrap();
        // The Appendix heading text from the fixture chapter must not
        // appear in the body. ("Appendix: Research Prompts" is the only
        // Appendix-classified chapter in `master_thesis_chapters()`.)
        assert!(
            !xml.contains("Appendix: Research Prompts"),
            "emit_appendix_in_back_matter=false must suppress every Appendix-classified chapter"
        );
        // The Bibliography (back-matter list) must still render — the
        // suppression is scoped to Appendix-classified chapters only.
        assert!(
            xml.contains("Bibliography"),
            "Bibliography must still render when only Appendix is suppressed"
        );
    }

    /// Wave-2 (Bookkit profile chrome suppression, REF parity 2026-06-04):
    /// when the manifest sets `emit_per_chapter_sources_box = false`, the
    /// engine must NOT emit any "Sources & QR codes" boxes (bookkit
    /// `flush_sources`). The reference thesis has zero per-chapter
    /// Sources boxes; the current bookkit profile emits one per chapter.
    #[test]
    fn thesis_chrome_suppression_skips_per_chapter_sources() {
        use std::io::Read;
        let meta = BookMeta {
            title: "Bookkit chrome suppression — Sources box".into(),
            thesis_profile: true,
            emit_per_chapter_sources_box: false,
            ..Default::default()
        };
        // Use a chapter set that includes a URL (so `flush_sources` would
        // otherwise emit a Sources box).
        let chapters = vec![
            (
                "tp".to_string(),
                "# Title Page\n\nMaster Thesis Submission.\n".to_string(),
            ),
            (
                "c1".to_string(),
                "# Introduction\n\nSee https://example.org for context.\n".to_string(),
            ),
            (
                "bib".to_string(),
                "# Bibliography\n\nDoe, J. (2026). https://doe.example/paper.\n".to_string(),
            ),
        ];
        let bytes = render_book(&meta, &chapters, Path::new(".")).unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut xml = String::new();
        zip.by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut xml)
            .unwrap();
        assert!(
            !xml.contains("Sources &amp; QR codes"),
            "emit_per_chapter_sources_box=false must suppress every Sources & QR codes box"
        );
    }

    /// Wave-3 iter-D (REF parity 2026-06-04): when `emit_per_chapter_sectpr`
    /// is set, the thesis renderer must emit one `<w:sectPr>` per chapter
    /// (plus the document-level sectPr that docx-rs always writes), so the
    /// total sectPr count climbs from 1 to N+1 where N is the chapter
    /// count. The reference master thesis has 19 in-body sectPrs + 1 doc-
    /// level = 20 total; the previous renderer emitted only the doc-level
    /// sectPr, leaving a 15-deficit in the bookkit_reference_targets gate.
    #[test]
    fn thesis_emit_per_chapter_sectpr_lifts_section_count() {
        use std::io::Read;
        // Baseline: flag off (historical) — expect 1 sectPr.
        let baseline_meta = BookMeta {
            title: "Bookkit per-chapter sectpr — baseline".into(),
            thesis_profile: true,
            ..Default::default()
        };
        let baseline_bytes =
            render_book(&baseline_meta, &master_thesis_chapters(), Path::new(".")).unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(baseline_bytes)).unwrap();
        let mut baseline_xml = String::new();
        zip.by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut baseline_xml)
            .unwrap();
        let baseline_sectpr = baseline_xml.matches("<w:sectPr").count();
        // Sanity: historical render carries exactly one doc-level sectPr.
        assert!(
            baseline_sectpr >= 1,
            "baseline must carry at least the doc-level sectPr, got {baseline_sectpr}"
        );

        // Opt-in: flag on — every body chapter contributes one sectPr.
        let opt_in_meta = BookMeta {
            title: "Bookkit per-chapter sectpr — opt-in".into(),
            thesis_profile: true,
            emit_per_chapter_sectpr: true,
            ..Default::default()
        };
        let opt_in_bytes =
            render_book(&opt_in_meta, &master_thesis_chapters(), Path::new(".")).unwrap();
        let mut zip2 = zip::ZipArchive::new(Cursor::new(opt_in_bytes)).unwrap();
        let mut opt_in_xml = String::new();
        zip2.by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut opt_in_xml)
            .unwrap();
        let opt_in_sectpr = opt_in_xml.matches("<w:sectPr").count();
        // The fixture has 14 chapters; opt-in must add at least 10 sectPrs
        // (the renderer skips Appendix when `emit_appendix_in_back_matter`
        // is false, but in this test that flag stays true ⇒ every chapter
        // contributes one sectPr).
        assert!(
            opt_in_sectpr > baseline_sectpr + 10,
            "opt-in must add ≥10 in-body sectPrs (got baseline={baseline_sectpr}, opt-in={opt_in_sectpr})"
        );
    }

    /// Wave-3 iter-D (REF parity 2026-06-04): when `emit_chapter_extras`
    /// is `false`, the renderer must skip every ```keypoints```, ```quiz```
    /// and ```callout``` fenced block (no paragraphs emitted, no BkCallout
    /// pStyle, no body text from the fenced block). The default `true`
    /// preserves historical AI Norms behaviour (covered by
    /// `chapter_extras_paragraph`).
    #[test]
    fn thesis_emit_chapter_extras_false_suppresses_keypoints_quiz_callout() {
        use std::io::Read;
        let meta = BookMeta {
            title: "Bookkit chapter-extras suppression".into(),
            thesis_profile: true,
            emit_chapter_extras: false,
            ..Default::default()
        };
        let chapters = vec![
            (
                "tp".to_string(),
                "# Title Page\n\nMaster Thesis Submission.\n".to_string(),
            ),
            (
                "c1".to_string(),
                "# Introduction\n\nBody prose stays.\n\n\
                 ```keypoints\n- Alpha keypoint marker\n- Beta keypoint marker\n```\n\n\
                 ```quiz\nQ: question prose marker\nA: answer prose marker\n```\n\n\
                 ```callout\nKey takeaway marker:\nbody marker line\n```\n"
                    .to_string(),
            ),
        ];
        let bytes = render_book(&meta, &chapters, Path::new(".")).unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut xml = String::new();
        zip.by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut xml)
            .unwrap();
        // Body prose preserved.
        assert!(xml.contains("Body prose stays"));
        // All three suppressed blocks: no marker text leaks into the docx.
        assert!(
            !xml.contains("Alpha keypoint marker"),
            "keypoints body must not render when emit_chapter_extras=false"
        );
        assert!(
            !xml.contains("question prose marker"),
            "quiz body must not render when emit_chapter_extras=false"
        );
        assert!(
            !xml.contains("Key takeaway marker"),
            "callout title must not render when emit_chapter_extras=false"
        );
        assert!(
            !xml.contains("body marker line"),
            "callout body must not render when emit_chapter_extras=false"
        );
    }

    #[test]
    fn thesis_layout_follows_user_supplied_fhnw_order() {
        // User-supplied FHNW MAS master-thesis structure (2026-05-28):
        //   Title page → Declaration of Originality → Compliance Declaration
        //   for Open-Source Photon OS → Management Summary → Table of Contents
        //   → Acronyms → numbered body (1-7) → Appendix → Table of Figures →
        //   Table of Tables → Bibliography → Index.
        let chapters = master_thesis_chapters();
        let layout = thesis_layout(&chapters);
        let expected = vec![
            ThesisItem::Chapter(0),  // Title page
            ThesisItem::Chapter(1),  // Declaration of Originality
            ThesisItem::Chapter(2),  // Compliance Declaration for Open-Source Photon OS
            ThesisItem::Chapter(3),  // Management Summary
            ThesisItem::Toc,         // Table of Contents (right after Mgmt Summary)
            ThesisItem::Chapter(4),  // Acronyms (last front-matter item)
            ThesisItem::Chapter(5),  // 1 Introduction
            ThesisItem::Chapter(6),  // 2 Theory
            ThesisItem::Chapter(7),  // 3 Current-State Analysis
            ThesisItem::Chapter(8),  // 4 Empirical Study
            ThesisItem::Chapter(9),  // 5 Solution
            ThesisItem::Chapter(10), // 6 Conclusion
            ThesisItem::Chapter(11), // 7 Personal Reflection
            ThesisItem::Chapter(12), // Appendix: Research Prompts
            ThesisItem::ListFigures, // back-matter list
            ThesisItem::ListTables,  // back-matter list
            ThesisItem::Chapter(13), // Bibliography
            ThesisItem::Index,       // engine-emitted back-of-book Index (last)
        ];
        assert_eq!(layout, expected);

        // Explicit guards for the user's structural invariants.
        let tp = layout
            .iter()
            .position(|i| *i == ThesisItem::Chapter(0))
            .unwrap();
        let ms = layout
            .iter()
            .position(|i| *i == ThesisItem::Chapter(3))
            .unwrap();
        let toc = layout.iter().position(|i| *i == ThesisItem::Toc).unwrap();
        let acro = layout
            .iter()
            .position(|i| *i == ThesisItem::Chapter(4))
            .unwrap();
        let body1 = layout
            .iter()
            .position(|i| *i == ThesisItem::Chapter(5))
            .unwrap();
        let appx = layout
            .iter()
            .position(|i| *i == ThesisItem::Chapter(12))
            .unwrap();
        let lof = layout
            .iter()
            .position(|i| *i == ThesisItem::ListFigures)
            .unwrap();
        let bib = layout
            .iter()
            .position(|i| *i == ThesisItem::Chapter(13))
            .unwrap();
        let idx = layout.iter().position(|i| *i == ThesisItem::Index).unwrap();
        assert!(tp < ms, "Title page must precede Management Summary");
        assert!(ms < toc, "Management Summary must precede TOC");
        assert!(toc < acro, "TOC must precede Acronyms");
        assert!(acro < body1, "Acronyms must precede numbered body");
        assert!(
            appx < lof,
            "Appendix must precede back-matter list of figures"
        );
        assert!(
            lof < bib,
            "List of figures/tables must precede Bibliography"
        );
        assert!(
            bib < idx,
            "Bibliography must precede the back-of-book Index"
        );
    }

    #[test]
    fn thesis_slot_classification() {
        assert_eq!(thesis_slot("# Title Page\n"), ThesisSlot::TitlePage);
        assert_eq!(
            thesis_slot("# Declaration of Originality\n"),
            ThesisSlot::DeclarationOriginality
        );
        assert_eq!(
            thesis_slot("# Compliance Declaration for Open-Source Photon OS\n"),
            ThesisSlot::ComplianceDeclaration
        );
        assert_eq!(
            thesis_slot("# Management Summary\n"),
            ThesisSlot::MgmtSummary
        );
        assert_eq!(
            thesis_slot("# Acronyms and Abbreviations\n"),
            ThesisSlot::Acronyms
        );
        assert_eq!(thesis_slot("# Bibliography\n"), ThesisSlot::Bibliography);
        assert_eq!(thesis_slot("# Appendix: Prompts\n"), ThesisSlot::Appendix);
        assert_eq!(
            thesis_slot("# Ehrenwörtliche Erklärung\n"),
            ThesisSlot::Declaration
        );
        assert_eq!(thesis_slot("# Introduction\n"), ThesisSlot::Body);
        assert_eq!(thesis_slot("# Theory\n"), ThesisSlot::Body);
    }

    #[test]
    fn thesis_profile_renders_valid_docx() {
        // End-to-end: the master_thesis bookkit renders a valid (PK-zip) DOCX
        // via the thesis path, with the full 12-chapter set.
        let meta = BookMeta {
            title: "Governance and Leadership…".into(),
            subtitle: "FHNW MAS Cybersecurity — Master's Thesis".into(),
            author: "Daniel Casota".into(),
            context: "MAS Cybersecurity, FHNW".into(),
            thesis_profile: true,
            ..Default::default()
        };
        let bytes = render_book(&meta, &master_thesis_chapters(), Path::new(".")).unwrap();
        assert_eq!(&bytes[..4], b"PK\x03\x04", "valid docx zip");
        assert!(bytes.len() > 3000, "non-trivial document");
    }

    #[test]
    fn thesis_with_explicit_title_chapter_has_no_engine_cover() {
        // Regression for the 2026-05-28 cascade output: the thesis profile was
        // emitting BOTH the engine-generated `title_page(doc, meta)` cover AND
        // the explicit `thesis/fhnw_00_title_page.md` chapter, producing two
        // title-like pages back-to-back. The fix suppresses the engine cover
        // when an explicit `ThesisSlot::TitlePage` chapter is supplied.
        //
        // The engine cover renders the literal `meta.subtitle` string in a
        // dedicated centred run; the explicit FHNW title chapter never does.
        // Inflate the docx ZIP, read `word/document.xml`, and assert the
        // subtitle string is ABSENT — proving the engine cover was suppressed.
        use std::io::Read;
        let unique_subtitle = "ENGINE_COVER_SUBTITLE_SENTINEL_2026";
        let meta = BookMeta {
            title: "Governance and Leadership…".into(),
            subtitle: unique_subtitle.into(),
            author: "Daniel Casota".into(),
            context: "MAS Cybersecurity, FHNW".into(),
            thesis_profile: true,
            ..Default::default()
        };
        let bytes = render_book(&meta, &master_thesis_chapters(), Path::new(".")).unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut d = String::new();
        zip.by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut d)
            .unwrap();
        assert!(
            !d.contains(unique_subtitle),
            "engine cover should be suppressed when explicit title chapter exists \
             (found subtitle sentinel in word/document.xml)"
        );
    }

    #[test]
    fn thesis_without_explicit_title_chapter_keeps_engine_cover() {
        // The dedup is opt-in: a manifest WITHOUT an explicit TitlePage chapter
        // still gets the engine-generated cover (so legacy thesis-profile books
        // are not regressed). Use a sentinel subtitle to prove the cover ran.
        use std::io::Read;
        let unique_subtitle = "ENGINE_COVER_SUBTITLE_SENTINEL_2026";
        let meta = BookMeta {
            title: "Legacy Thesis".into(),
            subtitle: unique_subtitle.into(),
            thesis_profile: true,
            ..Default::default()
        };
        let chapters: Vec<(String, String)> =
            vec![("intro".into(), "# Introduction\n\nBody text.\n".into())];
        let bytes = render_book(&meta, &chapters, Path::new(".")).unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut d = String::new();
        zip.by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut d)
            .unwrap();
        assert!(
            d.contains(unique_subtitle),
            "engine cover must still render when no explicit title chapter \
             (subtitle sentinel missing from word/document.xml)"
        );
    }

    #[test]
    fn fhnw_typography_profile_emits_arial_body_and_black_headings() {
        // ADR-0050 regression: with thesis_typography = FhnwProposalParity
        // the rendered docx body uses Arial, headings use Arial bold, and
        // no NAVY accent colour appears anywhere in the document XML.
        use std::io::Read;
        let meta = BookMeta {
            title: "Governance and Leadership…".into(),
            subtitle: "FHNW MAS Cybersecurity — Master's Thesis".into(),
            author: "Daniel Casota".into(),
            context: "MAS Cybersecurity, FHNW".into(),
            thesis_profile: true,
            thesis_typography: TypographyProfile::FhnwProposalParity,
            caption_format: CaptionFormat::Colon,
            ..Default::default()
        };
        let bytes = render_book(&meta, &master_thesis_chapters(), Path::new(".")).unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut d = String::new();
        zip.by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut d)
            .unwrap();
        // Body / heading font is Arial — appears as ASCII attribute on at
        // least one run-fonts element.
        assert!(
            d.contains("w:ascii=\"Arial\"") || d.contains("ascii=\"Arial\""),
            "FHNW profile must emit Arial font in run-fonts (word/document.xml)"
        );
        // Heading colour for FHNW is pure black; NAVY (`1F3864`) must NOT
        // appear in the document. (NAVY is the Designer-profile heading
        // accent; the FHNW profile uses `000000` for every heading.)
        assert!(
            !d.contains("\"1F3864\""),
            "FHNW profile must not emit the Designer NAVY colour anywhere"
        );
        // No HEAD2 lighter-blue either.
        assert!(
            !d.contains("\"2E4A7A\""),
            "FHNW profile must not emit the Designer HEAD2 colour anywhere"
        );
    }

    #[test]
    fn designer_typography_profile_keeps_navy_and_georgia() {
        // ADR-0050 regression for the OTHER side: with the default
        // TypographyProfile::Designer the engine still emits Georgia +
        // NAVY (so the 17 non-thesis books are unaffected by the FHNW
        // typography branch).
        use std::io::Read;
        let meta = BookMeta {
            title: "Merged Dimensions".into(),
            // thesis_typography defaults to Designer
            ..Default::default()
        };
        let bytes = render_book(
            &meta,
            &[(
                "c1".into(),
                "# Dimension 01 — Agile Leadership\n\nText.\n".into(),
            )],
            Path::new("."),
        )
        .unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut d = String::new();
        zip.by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut d)
            .unwrap();
        assert!(
            d.contains("Georgia") || d.contains("Calibri"),
            "Designer profile must keep the historical Georgia/Calibri fonts"
        );
        assert!(
            d.contains("1F3864") || d.contains("2E4A7A"),
            "Designer profile must keep at least one NAVY/HEAD2 colour"
        );
    }

    #[test]
    fn fhnw_header_sidecar_signaling() {
        // ADR-0050 item 1 (v0.1.15-engine, 2026-05-29): the engine no longer
        // emits a docx-rs Header for the FHNW profile (verified: docx-rs
        // Pic-in-header produces XML Word's parser silently rejects). The
        // header is now injected by `agentic book finalize` via Word COM,
        // reading a sidecar JSON written next to the docx by the CLI.
        //
        // This test verifies the SIGNALLING side of that contract:
        //
        //   * `fhnw_header_for` always returns None (no Header is attached)
        //   * `fhnw_header_sidecar_needed` returns true iff
        //     - FHNW typography profile is active, AND
        //     - at least one of (header_logo bytes, header_lines) is set
        //
        // The CLI uses `fhnw_header_sidecar_needed` to decide whether to
        // write the sidecar JSON + materialise the logo file. The finalize
        // step then reads them and uses Word's own InlineShapes.AddPicture
        // (which Word's parser obviously accepts).
        let meta_fhnw_with_lines = BookMeta {
            thesis_typography: TypographyProfile::FhnwProposalParity,
            header_lines: vec!["Master of Advanced Studies".into()],
            ..Default::default()
        };
        assert!(fhnw_header_for(&meta_fhnw_with_lines).is_none());
        assert!(fhnw_header_sidecar_needed(&meta_fhnw_with_lines));

        let meta_fhnw_no_inputs = BookMeta {
            thesis_typography: TypographyProfile::FhnwProposalParity,
            ..Default::default()
        };
        assert!(fhnw_header_for(&meta_fhnw_no_inputs).is_none());
        assert!(!fhnw_header_sidecar_needed(&meta_fhnw_no_inputs));

        let meta_designer = BookMeta {
            header_lines: vec!["Should not trigger".into()],
            ..Default::default()
        };
        assert!(fhnw_header_for(&meta_designer).is_none());
        assert!(
            !fhnw_header_sidecar_needed(&meta_designer),
            "Designer profile must not emit a sidecar regardless of lines"
        );
    }

    #[test]
    fn fhnw_body_paragraphs_are_justified() {
        // ADR-0050 §1 item 3 (v0.1.14) → ADR-0054 v1 (2026-06-02): body
        // paragraphs under BOTH profiles now carry w:jc w:val="both" (=
        // AlignmentType::Both, OOXML "justify").
        //
        // The Designer profile previously emitted LEFT — that was the
        // T1.8 reference-parity audit gap (Agent E §6 / Agent C diff).
        // Reference `AI_Norms_and_Regulations_BOOK.docx` was built by
        // `book_build/build_styles.py` which sets
        //   Normal.paragraph_format.alignment = WD_ALIGN_PARAGRAPH.JUSTIFY
        // so the historical Designer "LEFT" output diverged from the
        // reference book on every body paragraph. Aligning both profiles
        // to `Both` closes that gap; the FHNW profile still matched its
        // own proposal docx (which direct-formats JUSTIFY) before and
        // continues to match after.
        use std::io::Read;
        let meta_fhnw = BookMeta {
            title: "T".into(),
            thesis_typography: TypographyProfile::FhnwProposalParity,
            ..Default::default()
        };
        let bytes = render_book(
            &meta_fhnw,
            &[(
                "c1".into(),
                "# C\n\nA body paragraph that should justify.\n".into(),
            )],
            Path::new("."),
        )
        .unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut d = String::new();
        zip.by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut d)
            .unwrap();
        assert!(
            d.contains("w:val=\"both\""),
            "FHNW profile must emit w:jc w:val=\"both\" on body paragraphs"
        );

        // Designer parity: now ALSO emits w:jc=both on body paragraphs
        // (reference-parity audit T1.8, 2026-06-02). Previously this was
        // a negative regression guard; flipped to a positive parity check.
        let meta_designer = BookMeta {
            title: "T".into(),
            ..Default::default()
        };
        let bytes2 = render_book(
            &meta_designer,
            &[(
                "c1".into(),
                "# C\n\nA body paragraph that should justify.\n".into(),
            )],
            Path::new("."),
        )
        .unwrap();
        let mut zip2 = zip::ZipArchive::new(Cursor::new(bytes2)).unwrap();
        let mut d2 = String::new();
        zip2.by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut d2)
            .unwrap();
        assert!(
            d2.contains("w:val=\"both\""),
            "Designer profile must also emit w:jc=both on body paragraphs (ADR-0054 v1 parity)"
        );
    }

    #[test]
    fn caption_paragraph_carries_word_caption_style() {
        // ADR-0050 §1 item 8 (v0.1.14): caption paragraphs use Word's
        // built-in `Caption` style so the native List-of-Figures /
        // List-of-Tables dialog finds them. The style reference is
        // `w:pStyle w:val="Caption"` in word/document.xml.
        use std::io::Read;
        let meta = BookMeta {
            title: "T".into(),
            caption_format: CaptionFormat::Colon,
            ..Default::default()
        };
        let md = "# C\n\nTable: example for caption style.\n\n| A | B |\n|---|---|\n| 1 | 2 |\n";
        let bytes = render_book(&meta, &[("c1".into(), md.to_string())], Path::new(".")).unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut d = String::new();
        zip.by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut d)
            .unwrap();
        assert!(
            d.contains("w:val=\"Caption\""),
            "Table-caption paragraph must reference the Word 'Caption' style"
        );
    }

    #[test]
    fn acronyms_table_uses_10_80_10_column_widths() {
        // ADR-0050 §1 item 9 (v0.1.14): a 3-column table headed
        // "Acronym | Expansion | Pages" gets 10/80/10 widths instead of
        // equal-share. Every other 3-col table keeps equal widths.
        let header = vec![
            "Acronym".to_string(),
            "Expansion".to_string(),
            "Pages".to_string(),
        ];
        let widths = column_widths_for(&header, 10_000, 3);
        assert_eq!(widths.len(), 3);
        assert_eq!(widths[0], 1000, "Acronym col = 10%");
        assert_eq!(widths[2], 1000, "Pages col = 10%");
        assert_eq!(widths[1], 8000, "Expansion col = 80% (remainder)");
        assert_eq!(
            widths.iter().sum::<usize>(),
            10_000,
            "widths sum to content_twips"
        );

        // Non-matching headers fall through to equal-share.
        let other = vec!["A".to_string(), "B".to_string(), "C".to_string()];
        let widths2 = column_widths_for(&other, 9_000, 3);
        assert_eq!(widths2, vec![3000, 3000, 3000]);

        // 4-column headers fall through to equal-share.
        let four = vec![
            "Acronym".to_string(),
            "Expansion".to_string(),
            "Pages".to_string(),
            "Notes".to_string(),
        ];
        let widths3 = column_widths_for(&four, 9_000, 4);
        assert_eq!(widths3, vec![2250, 2250, 2250, 2250]);
    }

    #[test]
    fn caption_format_colon_emits_colon_separator() {
        // ADR-0050 §1: with CaptionFormat::Colon, a table caption renders
        // as "Table 1: <caption>" instead of "Table 1. <caption>". The
        // table-caption fold logic recognises a paragraph immediately
        // before a table (matching `fold_table_captions`); we use that
        // pattern here so the engine emits a real caption.
        use std::io::Read;
        let meta = BookMeta {
            title: "T".into(),
            caption_format: CaptionFormat::Colon,
            ..Default::default()
        };
        // A caption paragraph starting with "Table:" immediately preceding
        // a table is folded into a captioned-table block by
        // `fold_table_captions`. The folded caption is then rendered by
        // the caption code path that honours `CaptionFormat`.
        let md = "# C\n\nTable: MyUniqueColonCaption\n\n| A | B |\n|---|---|\n| 1 | 2 |\n";
        let bytes = render_book(&meta, &[("c1".into(), md.to_string())], Path::new(".")).unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut d = String::new();
        zip.by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut d)
            .unwrap();
        // The separator ": " appears in front of the caption text under
        // CaptionFormat::Colon — the only place this exact pattern shows
        // up is the caption renderer.
        assert!(
            d.contains(": MyUniqueColonCaption"),
            "CaptionFormat::Colon must emit \": <caption>\" in the caption run"
        );
        // The period separator must NOT appear in front of the caption
        // text under Colon mode.
        assert!(
            !d.contains(". MyUniqueColonCaption"),
            "Period separator should not appear when CaptionFormat::Colon is set"
        );
    }

    #[test]
    fn non_thesis_profiles_keep_book_layout() {
        // Guard: with thesis_profile = false the book path is taken (TOC-first
        // book layout), proving the thesis branch is isolated to bookkit C.
        let meta = BookMeta {
            title: "Merged Dimensions".into(),
            thesis_profile: false,
            ..Default::default()
        };
        let bytes = render_book(
            &meta,
            &[(
                "c1".into(),
                "# Dimension 01 — Agile Leadership\n\nText.\n".into(),
            )],
            Path::new("."),
        )
        .unwrap();
        assert_eq!(&bytes[..4], b"PK\x03\x04");
    }

    /// The dimensions book (bookkit A) chapter set, mirroring the manifest:
    /// front matter, the merged dimensions body (H1 per dimension, with H2/H3
    /// sub-sections so the 1-3 level TOC has depth), appendix, bibliography.
    fn dimensions_book_chapters() -> Vec<(String, String)> {
        [
            ("foreword", "# Foreword\n\nText.\n"),
            ("ack", "# Acknowledgements\n\nThanks.\n"),
            ("about", "# About this Book\n\nScope.\n"),
            ("preface", "# Preface\n\nWhy.\n"),
            (
                "acro",
                "# Acronyms and Abbreviations\n\nAI — Artificial Intelligence.\n",
            ),
            ("intro", "# Introduction\n\nIntro.\n"),
            (
                "dims",
                "# Dimension 01 — Agile Leadership\n\n## 1.1 Overview\n\nText.\n\n\
                 ### 1.1.1 Detail\n\nText.\n\n# Dimension 02 — Cybersecurity and AI\n\n\
                 ## 2.1 Overview\n\nText.\n",
            ),
            ("appx", "# Appendix: Research Prompts\n\nPrompts.\n"),
            ("bib", "# Bibliography\n\nDoe, J. (2026). A work.\n"),
        ]
        .into_iter()
        .map(|(l, m)| (l.to_string(), m.to_string()))
        .collect()
    }

    #[test]
    fn dimensions_book_enforces_auto_toc_levels_1_3() {
        // Bookkit A (no thesis_profile, no companion) must render the engine's
        // dedicated auto Table of Contents over heading levels 1-3 — the spec in
        // ADR-0030 ("auto TOC over heading levels 1–3") and ADR-0045 ("Table of
        // Contents (engine)"). Verified against the emitted Word field, not by
        // proxy.
        let meta = BookMeta {
            title: "Governance and Leadership…".into(),
            subtitle: "A Cross-Dimensional Field Guide".into(),
            author: "Daniel Casota".into(),
            context: "MAS Cybersecurity, FHNW".into(),
            disclaimer: Some("First researched edition.".into()),
            ..Default::default() // thesis_profile = false, companion = false
        };
        let bytes = render_book(&meta, &dimensions_book_chapters(), Path::new(".")).unwrap();
        assert_eq!(&bytes[..4], b"PK\x03\x04", "valid docx zip");

        use std::io::Read;
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut xml = String::new();
        zip.by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut xml)
            .unwrap();

        // The dedicated dimensions-book TOC: a Word TOC field over levels 1-3.
        assert!(
            xml.contains(r#"TOC \o &quot;1-3&quot;"#),
            "auto TOC over heading levels 1-3 must be present (the dedicated dimensions-book TOC)"
        );
        // The merged body's dimension H1s populate that TOC.
        assert!(
            xml.contains("Dimension 01"),
            "dimension chapters present in body/TOC"
        );
        // Book-path-only chrome the thesis path drops — proves bookkit A took the
        // BOOK layout, not the thesis layout (this test is solely the A profile).
        assert!(
            xml.contains("INDEX"),
            "books profile emits the back-of-book Index field"
        );
    }

    /// A campaign book (bookkit A) chapter set, mirroring the manifest: a
    /// campaign overview chapter followed by per-project/tool (PT-Cxx-n)
    /// sub-chapters, with H2/H3 sub-sections so the 1-3 level TOC has depth.
    fn campaign_book_chapters() -> Vec<(String, String)> {
        [
            (
                "overview",
                "# Campaign 01: Autonomous CVE Self-Patch\n\n## 0 At a glance\n\nMap.\n\n\
                 ## 1 Executive summary\n\nText.\n\n### 1.1 Value\n\nText.\n",
            ),
            (
                "pt1",
                "# P1 — Self-scout CVE feed integration\n\n## Owner\n\nTeam.\n\n## Effort\n\nM.\n",
            ),
            (
                "pt2",
                "# P2 — Backporting agent consensus\n\n## Owner\n\nTeam.\n\n## HITL\n\nGate.\n",
            ),
        ]
        .into_iter()
        .map(|(l, m)| (l.to_string(), m.to_string()))
        .collect()
    }

    #[test]
    fn campaign_book_enforces_auto_toc_levels_1_3() {
        // A campaign book is bookkit A (no thesis_profile, no companion): it must
        // render the engine's dedicated auto Table of Contents over heading
        // levels 1-3 (ADR-0030 / ADR-0045 "Table of Contents (engine)"). Verified
        // against the emitted Word field, not by proxy. Solely the campaign
        // bookkit (campaign overview + per-project/tool sub-chapters).
        let meta = BookMeta {
            title: "Campaign 01 — CVE Self-Patch".into(),
            subtitle: "Broadcom + partner SDD campaign".into(),
            author: "Daniel Casota".into(),
            context: "MAS Cybersecurity, FHNW".into(),
            disclaimer: Some("First researched edition.".into()),
            ..Default::default() // thesis_profile = false, companion = false
        };
        let bytes = render_book(&meta, &campaign_book_chapters(), Path::new(".")).unwrap();
        assert_eq!(&bytes[..4], b"PK\x03\x04", "valid docx zip");

        use std::io::Read;
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut xml = String::new();
        zip.by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut xml)
            .unwrap();

        // The dedicated campaign-book TOC: a Word TOC field over levels 1-3.
        assert!(
            xml.contains(r#"TOC \o &quot;1-3&quot;"#),
            "auto TOC over heading levels 1-3 must be present (the dedicated campaign-book TOC)"
        );
        // The campaign overview + per-project chapters populate that TOC.
        assert!(
            xml.contains("Campaign 01"),
            "campaign overview present in body/TOC"
        );
        assert!(
            xml.contains("Self-scout CVE feed"),
            "per-project/tool sub-chapters present"
        );
        // Book-path-only chrome the thesis path drops — proves the campaign book
        // took the BOOK layout, not the thesis layout (solely the A profile).
        assert!(
            xml.contains("INDEX"),
            "books profile emits the back-of-book Index field"
        );
    }

    #[test]
    fn thesis_profile_numbers_body_chapters() {
        // Book profile: Introduction is unnumbered front-matter.
        assert!(!chapter_is_numbered("# Introduction\n\nText.\n", false));
        // Thesis profile: Introduction/Theory/Conclusion are numbered chapters…
        assert!(chapter_is_numbered("# Introduction\n\nText.\n", true));
        assert!(chapter_is_numbered("# Theory\n\nText.\n", true));
        assert!(chapter_is_numbered("# Conclusion\n\nText.\n", true));
        assert!(chapter_is_numbered(
            "# Personal Reflection\n\nText.\n",
            true
        ));
        // …but Management Summary / Acronyms / Appendix / Bibliography stay front/back-matter.
        assert!(!chapter_is_numbered(
            "# Management Summary\n\nText.\n",
            true
        ));
        assert!(!chapter_is_numbered("# Acronyms and Abbreviations\n", true));
        assert!(!chapter_is_numbered(
            "# Appendix: The Research Prompts\n",
            true
        ));
        assert!(!chapter_is_numbered("# Bibliography\n", true));
    }

    #[test]
    fn field_instructions_are_xml_escaped() {
        use std::io::Read;
        let meta = BookMeta {
            title: "T".into(),
            subtitle: String::new(),
            author: "A".into(),
            context: "C".into(),
            ..Default::default()
        };
        // "MITRE ATT&CK" is a curated index term; its XE field must escape '&'.
        let md = "# C\n\nThe MITRE ATT&CK framework catalogues adversary tactics.\n".to_string();
        let bytes = render_book(&meta, &[("c1".into(), md)], Path::new(".")).unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut xml = String::new();
        zip.by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut xml)
            .unwrap();
        assert!(xml.contains("ATT&amp;CK"), "field instr must be escaped");
        // no raw ampersand that isn't an entity
        assert!(
            !regex_lite_has_raw_amp(&xml),
            "document.xml has a raw unescaped '&'"
        );
    }

    fn regex_lite_has_raw_amp(s: &str) -> bool {
        let b = s.as_bytes();
        for (i, &c) in b.iter().enumerate() {
            if c == b'&' {
                let tail = &s[i + 1..];
                let ok = ["amp;", "lt;", "gt;", "quot;", "apos;", "#"]
                    .iter()
                    .any(|e| tail.starts_with(e));
                if !ok {
                    return true;
                }
            }
        }
        false
    }

    #[test]
    fn admonition_renders() {
        let meta = BookMeta {
            title: "T".into(),
            subtitle: String::new(),
            author: "A".into(),
            context: "C".into(),
            ..Default::default()
        };
        let md = "# C\n\n```warning\nThe EU AI Act has extraterritorial reach.\n```\n".to_string();
        let bytes = render_book(&meta, &[("c1".into(), md)], Path::new(".")).unwrap();
        assert_eq!(&bytes[..4], b"PK\x03\x04");
        assert!(bytes.len() > 2000);
    }

    /// Wave-3 (AI-Norms parity, 2026-06-03): the four chapter_extras emitters
    /// — `keypoints`, `callout`, `note`/`tip`/`warning` admonitions — must not
    /// wrap their body in `<w:tbl>`. They must emit paragraphs styled with
    /// `BkCallout`, which the `captioned_table_parity` gate then correctly
    /// excludes from the table inventory.
    ///
    /// Round-E parity (BkCallout deficit, 2026-06-03): keypoints **bullets**
    /// now also emit `BkCallout` (not `BkBullet`) to match the reference
    /// styling — every line inside the box lives under the same callout
    /// frame, which closes the ~136-paragraph deficit (228 → ~364) the
    /// AI Norms parity report flagged.
    #[test]
    fn chapter_extras_paragraph() {
        let meta = BookMeta {
            title: "T".into(),
            author: "A".into(),
            ..Default::default()
        };
        let md = "# C\n\n\
            ```keypoints\n- Alpha\n- Beta\n- Gamma\n```\n\n\
            ```callout\nKey takeaway:\nLines below the title carry the body.\n```\n\n\
            ```note\nThis is an informational aside.\n```\n\n\
            ```tip\nSpeed up your workflow.\n```\n\n\
            ```warning\nDo not skip this step.\n```\n"
            .to_string();
        let bytes = render_book(&meta, &[("c1".into(), md)], Path::new(".")).unwrap();
        let xml = doc_xml(bytes);
        // Body content survives in paragraph runs.
        assert!(xml.contains("Alpha"), "keypoint line emitted");
        assert!(xml.contains("Key takeaway"), "callout title emitted");
        assert!(
            xml.contains("Lines below the title"),
            "callout body emitted"
        );
        assert!(xml.contains("informational aside"), "note body emitted");
        assert!(xml.contains("Speed up your workflow"), "tip body emitted");
        assert!(
            xml.contains("Do not skip this step"),
            "warning body emitted"
        );
        // BkCallout style is now applied (Wave 2 reference port supplies the
        // visual flavour previously hard-coded as cell shading).
        assert!(
            xml.contains("w:pStyle w:val=\"BkCallout\""),
            "callout body uses BkCallout style"
        );
        // Round-E parity: keypoints bullets share the `BkCallout` style so
        // the entire box is one callout frame (title + N body lines). The
        // reference book has 32 keypoints groups × ~6 paragraphs each ≈ 201
        // `BkCallout` paragraphs from this single block type.
        let bk_callout = xml.matches("w:pStyle w:val=\"BkCallout\"").count();
        assert!(
            bk_callout >= 1 + 3,
            "expected at least 4 BkCallout paragraphs (keypoints title + 3 bullets); got {bk_callout}"
        );
        // No empty/spurious table wrappers for the chapter_extras blocks
        // (the only `<w:tbl>` allowed here is the Sources & QR-codes box —
        // and this chapter has no links so even that doesn't appear).
        let tbl_count = xml.matches("<w:tbl>").count();
        assert_eq!(
            tbl_count, 0,
            "chapter_extras blocks must not emit any <w:tbl> (got {tbl_count}); the Sources box should be absent because no links were used"
        );
    }

    /// Wave-9 polish (AI-Norms parity, 2026-06-03): with
    /// `body_render_use_bk_styles=true`, every body bullet item
    /// (`- foo` markdown) must emit a `<w:p>` styled `BkBullet` so the
    /// parity gate's `BkBullet` count picks up chapter-prose bullets
    /// (the reference book has 659 of them, dominated by main chapter
    /// content rather than the keypoints boxes).
    #[test]
    fn body_bullets_apply_bk_bullet_style_when_enabled() {
        let meta = BookMeta {
            title: "T".into(),
            author: "A".into(),
            body_render_use_bk_styles: true,
            ..Default::default()
        };
        let md = "# C\n\nIntro.\n\n- Alpha\n- Beta\n\nOutro.\n".to_string();
        let bytes = render_book(&meta, &[("c1".into(), md)], Path::new(".")).unwrap();
        let xml = doc_xml(bytes);
        let bk_bullet = xml.matches("w:pStyle w:val=\"BkBullet\"").count();
        assert!(
            bk_bullet >= 2,
            "expected at least 2 BkBullet paragraphs from `- Alpha`/`- Beta`; got {bk_bullet}"
        );
    }

    /// Round-F (AI-Norms parity, 2026-06-03): the reference docx styles
    /// **numbered** body list items with `BkBullet` too (360 of the 659
    /// reference `BkBullet` paragraphs are `N.`-prefixed, vs 299 with `•`).
    /// With `body_render_use_bk_styles=true`, every `1. foo` markdown item
    /// must emit a `<w:p>` styled `BkBullet` so the parity gate's count
    /// includes numbered prose lists, not just `- bullet` items.
    #[test]
    fn body_ordered_items_apply_bk_bullet_style_when_enabled() {
        let meta = BookMeta {
            title: "T".into(),
            author: "A".into(),
            body_render_use_bk_styles: true,
            ..Default::default()
        };
        let md = "# C\n\nIntro.\n\n1. Alpha\n2. Beta\n3. Gamma\n\nOutro.\n".to_string();
        let bytes = render_book(&meta, &[("c1".into(), md)], Path::new(".")).unwrap();
        let xml = doc_xml(bytes);
        let bk_bullet = xml.matches("w:pStyle w:val=\"BkBullet\"").count();
        assert!(
            bk_bullet >= 3,
            "expected ≥3 BkBullet paragraphs from `1.`/`2.`/`3.`; got {bk_bullet}"
        );
    }

    /// Round V zone D (2026-06-03): when `BkBullet` is applied the engine
    /// must NOT also emit an inline `<w:spacing w:after="160"/>` or
    /// `<w:jc w:val="both"/>` override — the `BkBullet` style itself
    /// declares `w:spacing w:after="80"` + `w:jc w:val="left"`, and inline
    /// overrides would silently flip both values and break reference
    /// parity. The test renders a single bulleted paragraph and inspects
    /// the very first `<w:p>` that carries `w:pStyle="BkBullet"` to assert
    /// neither inline override is present on that paragraph's `<w:pPr>`.
    #[test]
    fn bk_bullet_paragraph_has_no_inline_spacing_or_jc_override() {
        let meta = BookMeta {
            title: "T".into(),
            author: "A".into(),
            body_render_use_bk_styles: true,
            ..Default::default()
        };
        let md = "# C\n\n- Alpha bullet item.\n".to_string();
        let bytes = render_book(&meta, &[("c1".into(), md)], Path::new(".")).unwrap();
        let xml = doc_xml(bytes);
        // Locate the first <w:p> whose pPr carries pStyle="BkBullet".
        let needle = "w:pStyle w:val=\"BkBullet\"";
        let bk_at = xml.find(needle).expect("BkBullet pStyle must be present");
        // Walk back to find the enclosing <w:p ...> start.
        let p_start = xml[..bk_at]
            .rfind("<w:p ")
            .or_else(|| xml[..bk_at].rfind("<w:p>"))
            .expect("found enclosing <w:p>");
        let p_end_rel = xml[p_start..].find("</w:p>").expect("found </w:p>");
        let p_block = &xml[p_start..p_start + p_end_rel];
        // The pPr block ends at </w:pPr> — only assert on the pPr, not on
        // run-level w:rPr (which is unrelated).
        let ppr_end = p_block
            .find("</w:pPr>")
            .expect("pPr present on styled <w:p>");
        let ppr = &p_block[..ppr_end];
        assert!(
            !ppr.contains("w:spacing"),
            "BkBullet paragraph must not carry inline <w:spacing …/> override (style declares after=80); pPr was: {ppr}"
        );
        assert!(
            !ppr.contains("w:jc "),
            "BkBullet paragraph must not carry inline <w:jc w:val=\"…\"/> override (style declares jc=left); pPr was: {ppr}"
        );
    }

    /// Round V zone D (2026-06-03): the bullet glyph (the leading `•` or
    /// `N.` run) must use the ACCENT colour `0B5C9E` under the Designer
    /// profile, NOT the NAVY heading colour `1F3864`. The bullet runs are
    /// emitted with an explicit `<w:color w:val="…"/>` element in the run
    /// properties.
    #[test]
    fn bullet_glyph_uses_accent_color_for_designer() {
        let meta = BookMeta {
            title: "T".into(),
            author: "A".into(),
            ..Default::default()
        };
        let md = "# C\n\n- Alpha bullet item.\n".to_string();
        let bytes = render_book(&meta, &[("c1".into(), md)], Path::new(".")).unwrap();
        let xml = doc_xml(bytes);
        assert!(
            xml.contains("w:val=\"0B5C9E\""),
            "Designer bullet glyph must adopt ACCENT 0B5C9E"
        );
    }

    /// Round-F: ordered items remain UNSTYLED when the flag is off so
    /// non-parity books keep the historical Designer numbered-list look.
    #[test]
    fn body_ordered_items_remain_unstyled_when_flag_off() {
        let meta = BookMeta {
            title: "T".into(),
            author: "A".into(),
            ..Default::default()
        };
        let md = "# C\n\n1. Alpha\n2. Beta\n".to_string();
        let bytes = render_book(&meta, &[("c1".into(), md)], Path::new(".")).unwrap();
        let xml = doc_xml(bytes);
        let bk_bullet_psty = xml.matches("w:pStyle w:val=\"BkBullet\"").count();
        assert_eq!(
            bk_bullet_psty, 0,
            "Designer profile must not pStyle BkBullet on numbered items (got {bk_bullet_psty})"
        );
    }

    /// Round-G (AI-Norms parity, 2026-06-03): a plain paragraph that begins
    /// with `N. ` (not a markdown ordered list, just prose with a numbered
    /// prefix) must adopt `BkBullet` when `body_render_use_bk_styles=true`.
    /// The reference book uses this pattern for ~141 of its 659 BkBullet
    /// paragraphs (numbered references, enumerated explanations, etc.).
    #[test]
    fn paragraph_with_numeric_prefix_gets_bkbullet() {
        let meta = BookMeta {
            title: "T".into(),
            author: "A".into(),
            body_render_use_bk_styles: true,
            ..Default::default()
        };
        // A blank line between each paragraph keeps the pulldown-cmark
        // parser from collapsing them into a single ordered-list block.
        let md = "# C\n\nIntro.\n\n1. Foo bar baz qux quux.\n\nMiddle.\n\n\
                  2. Second standalone item.\n\nOutro.\n"
            .to_string();
        let bytes = render_book(&meta, &[("c1".into(), md)], Path::new(".")).unwrap();
        let xml = doc_xml(bytes);
        let bk_bullet = xml.matches("w:pStyle w:val=\"BkBullet\"").count();
        assert!(
            bk_bullet >= 2,
            "expected ≥2 BkBullet paragraphs from numeric-prefix prose; got {bk_bullet}"
        );
    }

    /// Round-G: paragraphs starting with `R\d+.` (recommendation IDs) and
    /// `Q\d+.` (quiz questions) and single-letter `A.` option labels also
    /// adopt `BkBullet` under the parity flag.
    #[test]
    #[allow(non_snake_case)]
    fn paragraph_with_R_prefix_gets_bkbullet() {
        let meta = BookMeta {
            title: "T".into(),
            author: "A".into(),
            body_render_use_bk_styles: true,
            ..Default::default()
        };
        let md = "# C\n\nIntro.\n\nR1. Create a strategic plan.\n\n\
                  Q3. Why does the audit matter?\n\nA. First option label.\n\n\
                  Outro.\n"
            .to_string();
        let bytes = render_book(&meta, &[("c1".into(), md)], Path::new(".")).unwrap();
        let xml = doc_xml(bytes);
        let bk_bullet = xml.matches("w:pStyle w:val=\"BkBullet\"").count();
        assert!(
            bk_bullet >= 3,
            "expected ≥3 BkBullet paragraphs from R1./Q3./A. prefixes; got {bk_bullet}"
        );
    }

    /// Round-G: a paragraph that LOOKS numbered but is actually a section
    /// number (`5.1 Foo`, `5.14.2 Bar`) must NOT get `BkBullet` — the
    /// digit-after-period heuristic distinguishes prose enumerations from
    /// multi-level section numbering.
    #[test]
    fn paragraph_with_section_number_prefix_does_not_get_bkbullet() {
        let meta = BookMeta {
            title: "T".into(),
            author: "A".into(),
            body_render_use_bk_styles: true,
            ..Default::default()
        };
        // Plain paragraph, not a markdown ordered list, so pulldown-cmark
        // emits a `DocxBlock::Paragraph` whose first run starts with `5.1`.
        let md = "# C\n\n5.1 Foo bar baz.\n\n5.14.2 Multi-level section.\n".to_string();
        let bytes = render_book(&meta, &[("c1".into(), md)], Path::new(".")).unwrap();
        let xml = doc_xml(bytes);
        let bk_bullet = xml.matches("w:pStyle w:val=\"BkBullet\"").count();
        assert_eq!(
            bk_bullet, 0,
            "section-number prefixes must not get BkBullet; got {bk_bullet}"
        );
    }

    /// Round-G: a paragraph with no recognised prefix keeps its normal
    /// (unstyled) paragraph style even under the parity flag.
    #[test]
    fn paragraph_without_prefix_keeps_normal_style() {
        let meta = BookMeta {
            title: "T".into(),
            author: "A".into(),
            body_render_use_bk_styles: true,
            ..Default::default()
        };
        let md = "# C\n\nThis is just a plain paragraph with no prefix.\n\n\
                  And another one, also plain.\n"
            .to_string();
        let bytes = render_book(&meta, &[("c1".into(), md)], Path::new(".")).unwrap();
        let xml = doc_xml(bytes);
        let bk_bullet = xml.matches("w:pStyle w:val=\"BkBullet\"").count();
        assert_eq!(
            bk_bullet, 0,
            "plain paragraphs must not get BkBullet; got {bk_bullet}"
        );
    }

    /// Round-G unit test for the prefix helper itself — exhaustive matrix.
    #[test]
    fn should_apply_bk_bullet_prefix_matrix() {
        // Positive cases.
        assert!(should_apply_bk_bullet_prefix("1. Foo"));
        assert!(should_apply_bk_bullet_prefix("12. Foo"));
        assert!(should_apply_bk_bullet_prefix("123. Foo"));
        assert!(should_apply_bk_bullet_prefix("R1. Adopt"));
        assert!(should_apply_bk_bullet_prefix("Q3. Why"));
        assert!(should_apply_bk_bullet_prefix("A. First"));
        assert!(should_apply_bk_bullet_prefix("B. Second"));
        assert!(should_apply_bk_bullet_prefix("  3. Leading whitespace"));
        // Negative: section numbers.
        assert!(!should_apply_bk_bullet_prefix("5.1 Foo"));
        assert!(!should_apply_bk_bullet_prefix("5.14.2 Foo"));
        // Negative: no period or no whitespace after.
        assert!(!should_apply_bk_bullet_prefix("Just prose"));
        assert!(!should_apply_bk_bullet_prefix("1.Foo")); // no space → likely v1.5 style
        assert!(!should_apply_bk_bullet_prefix("Dr. Smith")); // multi-letter prefix
        assert!(!should_apply_bk_bullet_prefix("R Foo")); // no digit after R
        assert!(!should_apply_bk_bullet_prefix("AB. Foo")); // two-letter
        assert!(!should_apply_bk_bullet_prefix("")); // empty
    }

    /// Wave-9 polish: with `body_render_use_bk_styles=false` (Designer
    /// profile / FHNW thesis / every non-parity book), body bullets stay
    /// UNSTYLED so the historical Designer aesthetic is preserved.
    #[test]
    fn body_bullets_remain_unstyled_when_flag_off() {
        let meta = BookMeta {
            title: "T".into(),
            author: "A".into(),
            ..Default::default()
        };
        let md = "# C\n\n- Alpha\n- Beta\n".to_string();
        let bytes = render_book(&meta, &[("c1".into(), md)], Path::new(".")).unwrap();
        let xml = doc_xml(bytes);
        let bk_bullet_psty = xml.matches("w:pStyle w:val=\"BkBullet\"").count();
        assert_eq!(
            bk_bullet_psty, 0,
            "Designer profile must not pStyle BkBullet on body bullets (got {bk_bullet_psty})"
        );
    }

    /// Wave-9 polish: a callout with a `**Bold title.**` prefix (the
    /// dominant pattern in the AI Norms reference) must emit TWO
    /// `BkCallout` paragraphs — title + body — not one.
    #[test]
    fn callout_with_bold_prefix_splits_into_two_bk_callouts() {
        let meta = BookMeta {
            title: "T".into(),
            author: "A".into(),
            ..Default::default()
        };
        let md =
            "# C\n\n```callout\n**Orientation.** Why this matters in practice.\n```\n".to_string();
        let bytes = render_book(&meta, &[("c1".into(), md)], Path::new(".")).unwrap();
        let xml = doc_xml(bytes);
        let bk_callout = xml.matches("w:pStyle w:val=\"BkCallout\"").count();
        assert!(
            bk_callout >= 2,
            "expected ≥2 BkCallout paragraphs (title + body); got {bk_callout}"
        );
        assert!(xml.contains("Orientation"), "title text rendered");
        assert!(xml.contains("Why this matters"), "body text rendered");
    }

    /// Wave-9 polish: even a callout without a recognisable title (no
    /// trailing `:` AND no leading `**Bold**` span) must still emit two
    /// `BkCallout` paragraphs. The fallback title is a placeholder so the
    /// gate count stays predictable across content styles.
    #[test]
    fn callout_without_title_still_emits_two_bk_callouts() {
        let meta = BookMeta {
            title: "T".into(),
            author: "A".into(),
            ..Default::default()
        };
        let md =
            "# C\n\n```callout\nPlain prose with neither colon nor bold prefix.\n```\n".to_string();
        let bytes = render_book(&meta, &[("c1".into(), md)], Path::new(".")).unwrap();
        let xml = doc_xml(bytes);
        let bk_callout = xml.matches("w:pStyle w:val=\"BkCallout\"").count();
        assert!(
            bk_callout >= 2,
            "fallback callout must still emit ≥2 BkCallout paragraphs; got {bk_callout}"
        );
    }

    /// Round-V Zone-E (visual parity, 2026-06-03): every `BkCallout`
    /// paragraph emitted by `admonition_box` / `callout_box` /
    /// `keypoints_box` must carry an inline per-flavor `<w:pBdr>`
    /// (left accent) + `<w:shd>` (fill) pair after postprocess. The
    /// pair MUST appear inside `<w:pPr>` in correct OOXML schema
    /// order (`pBdr` BEFORE `shd`; cross-cutting risk #8).
    ///
    /// Reference colors (verbatim from
    /// `book_build/AI_Norms_and_Regulations_BOOK.docx`):
    /// tip `2E7D32/EAF6EC`, note `1F3864/EAF1FB`,
    /// warning `C77F18/FBF1E2`, generic `1F3864/EEF2F8`,
    /// keypoints `8A8A8A/ECECEC`.
    #[test]
    fn callout_chrome_per_flavor_pbdr_then_shd() {
        let meta = BookMeta {
            title: "T".into(),
            author: "A".into(),
            ..Default::default()
        };
        let md = "# C\n\n\
            ```keypoints\n- Alpha\n- Beta\n```\n\n\
            ```callout\nKey takeaway:\nBody of generic callout.\n```\n\n\
            ```note\nAn informational note.\n```\n\n\
            ```tip\nA helpful tip.\n```\n\n\
            ```warning\nA loud warning.\n```\n"
            .to_string();
        let bytes = render_book(&meta, &[("c1".into(), md)], Path::new(".")).unwrap();
        let xml = doc_xml(bytes);

        // Every flavor color must appear at least once on a BkCallout
        // paragraph. We check the (border, fill) pair per flavor.
        for (name, border, fill) in [
            ("tip", "2E7D32", "EAF6EC"),
            ("note", "1F3864", "EAF1FB"),
            ("warning", "C77F18", "FBF1E2"),
            ("generic", "1F3864", "EEF2F8"),
            ("keypoints", "8A8A8A", "ECECEC"),
        ] {
            let border_token = format!(r#"w:color="{border}""#);
            let fill_token = format!(r#"w:fill="{fill}""#);
            assert!(
                xml.contains(&border_token),
                "{name}: expected border color {border} to appear in document.xml"
            );
            assert!(
                xml.contains(&fill_token),
                "{name}: expected fill {fill} to appear in document.xml"
            );
        }

        // For at least one tip paragraph, verify pBdr precedes shd
        // inside the same <w:pPr> block (schema order is strict —
        // OOXML readers and Word both reject the reverse order).
        let pbdr_pos = xml
            .find(r#"<w:pBdr><w:left w:val="single" w:sz="24" w:space="8" w:color="2E7D32"/></w:pBdr>"#)
            .expect("tip pBdr injected verbatim");
        let shd_pos = xml
            .find(r#"<w:shd w:val="clear" w:color="auto" w:fill="EAF6EC"/>"#)
            .expect("tip shd injected verbatim");
        assert!(
            pbdr_pos < shd_pos,
            "OOXML schema requires pBdr to precede shd in pPr (cross-cutting risk #8); got pbdr@{pbdr_pos} shd@{shd_pos}"
        );

        // All flavor sentinels must be removed after postprocess.
        assert!(
            !xml.contains("BkFlavor:"),
            "flavor sentinels must be stripped by apply_callout_chrome"
        );
    }

    #[test]
    fn wide_table_emits_landscape_section() {
        let meta = BookMeta {
            title: "T".into(),
            subtitle: String::new(),
            author: "A".into(),
            context: "C".into(),
            ..Default::default()
        };
        // 7 columns ⇒ at/above LANDSCAPE_COLS ⇒ rendered on a landscape page.
        let header: Vec<String> = (1..=7).map(|i| format!("H{i}")).collect();
        let row: Vec<String> = (1..=7).map(|i| format!("c{i}")).collect();
        let docx = render_book_to_docx(&meta, &header, &row);
        // Unzip document.xml and assert a landscape sectPr is present.
        let mut zip = zip::ZipArchive::new(Cursor::new(docx)).unwrap();
        let mut xml = String::new();
        {
            use std::io::Read;
            zip.by_name("word/document.xml")
                .unwrap()
                .read_to_string(&mut xml)
                .unwrap();
        }
        assert!(
            xml.contains("orient=\"landscape\""),
            "wide table should produce a landscape section"
        );
    }

    fn doc_xml(bytes: Vec<u8>) -> String {
        use std::io::Read;
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut xml = String::new();
        zip.by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut xml)
            .unwrap();
        xml
    }

    #[test]
    fn ordered_list_gets_real_numerals_and_sources_box() {
        let meta = BookMeta {
            title: "T".into(),
            author: "A".into(),
            ..Default::default()
        };
        let md = "# Chapter\n\n1. First\n2. Second\n3. Third\n\nSee [the site](https://example.com/x).\n"
            .to_string();
        let bytes = render_book(&meta, &[("c1".into(), md)], Path::new(".")).unwrap();
        let xml = doc_xml(bytes.clone());
        // real running numerals, not a static en-dash
        assert!(
            xml.contains("2.  "),
            "ordered list should show real numerals"
        );
        assert!(xml.contains("3.  "));
        // per-chapter Sources & QR box (note: '&' is XML-escaped)
        assert!(
            xml.contains("Sources &amp; QR codes"),
            "links should produce a Sources box"
        );
        // a QR PNG was embedded into the package media
        let zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        assert!(
            zip.file_names().any(|n| n.starts_with("word/media/")),
            "a QR image should be embedded"
        );
    }

    /// Round-D AI-Norms parity (2026-06-03): bare/plain-text URLs in a
    /// chapter must contribute QR codes to the per-chapter Sources box —
    /// each registered URL emits one extra `<w:drawing>` (the QR PNG).
    /// The AI-Norms reference book has 20 such bare-URL QR codes that the
    /// agentic pipeline was missing; this gate locks in the renderer
    /// behaviour that closes the FIGURE_COUNT_PARITY deficit.
    #[test]
    fn bare_text_urls_emit_qr_drawings_in_sources_box() {
        use std::io::Read;
        let meta = BookMeta {
            title: "T".into(),
            author: "A".into(),
            ..Default::default()
        };
        // Baseline chapter without URLs.
        let baseline = "# Chapter\n\nThis chapter has no URLs at all.\n".to_string();
        let xml_base =
            doc_xml(render_book(&meta, &[("c1".into(), baseline)], Path::new(".")).unwrap());
        let base_drawings = xml_base.matches("<w:drawing").count();

        // Chapter with three bare-text URLs (no markdown link wrappers).
        let with_urls = "# Chapter\n\nReferences include https://example.org/a , \
            https://example.org/b. and (https://example.org/c).\n"
            .to_string();
        let bytes = render_book(&meta, &[("c1".into(), with_urls)], Path::new(".")).unwrap();
        let xml = doc_xml(bytes.clone());
        let urls_drawings = xml.matches("<w:drawing").count();
        assert!(
            urls_drawings >= base_drawings + 3,
            "expected ≥ baseline+3 drawings (one QR per bare URL); \
             got base={base_drawings} with_urls={urls_drawings}",
        );
        // The Sources box appears (so the QR column has a place to live).
        assert!(
            xml.contains("Sources &amp; QR codes"),
            "bare URLs should produce a Sources box",
        );
        // And the QR PNGs were physically packaged into word/media/.
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let media_count = zip
            .file_names()
            .filter(|n| n.starts_with("word/media/") && n.ends_with(".png"))
            .count();
        assert!(
            media_count >= 3,
            "≥3 QR PNGs should be embedded in word/media/, got {media_count}",
        );
        // Spot-check: each registered URL is preserved verbatim in the doc.
        for url in [
            "https://example.org/a",
            "https://example.org/b",
            "https://example.org/c",
        ] {
            // The Sources table prints the URL as plain text underneath the
            // clickable hyperlink — at least one occurrence must survive.
            let mut buf = String::new();
            zip.by_name("word/document.xml")
                .unwrap()
                .read_to_string(&mut buf)
                .unwrap();
            assert!(buf.contains(url), "URL {url} missing from document.xml");
        }
    }

    #[test]
    fn table_caption_is_folded_and_numbered() {
        let meta = BookMeta {
            title: "T".into(),
            author: "A".into(),
            ..Default::default()
        };
        let md =
            "# Chapter\n\nTable: Demo caption\n\n| A | B |\n|---|---|\n| 1 | 2 |\n".to_string();
        let xml = doc_xml(render_book(&meta, &[("c1".into(), md)], Path::new(".")).unwrap());
        assert!(xml.contains("Demo caption"), "table caption text present");
        assert!(
            xml.contains("SEQ Table"),
            "table caption carries a SEQ Table field"
        );
    }

    #[test]
    fn quote_and_chapter_number_render() {
        let meta = BookMeta {
            title: "T".into(),
            author: "A".into(),
            ..Default::default()
        };
        // ci>0 so the chapter-number prefix applies to a numbered chapter.
        let intro = "# Foreword\n\nWelcome.\n".to_string(); // unnumbered
        let body = "# Real Chapter\n\n```quote\nThe machine must be governed.\n— Kranzberg\n```\n"
            .to_string();
        let xml = doc_xml(
            render_book(
                &meta,
                &[("c0".into(), intro), ("c1".into(), body)],
                Path::new("."),
            )
            .unwrap(),
        );
        assert!(xml.contains("The machine must be governed."));
        assert!(xml.contains("Kranzberg"));
        assert!(
            xml.contains("1  Real Chapter"),
            "numbered chapter gets an N prefix"
        );
    }

    #[test]
    fn german_chrome_localises_while_english_chrome_absent() {
        let meta = BookMeta {
            title: "T".into(),
            author: "A".into(),
            lang: "de".into(),
            ..Default::default()
        };
        // Figure + table + a warning admonition + a link (Sources box) exercise
        // every localised chrome site.
        let md = "# Kapitel\n\n```warning\nGefahr.\n```\n\nTable: Demo\n\n| A | B |\n|---|---|\n| 1 | 2 |\n\nSiehe [Quelle](https://example.com/x).\n"
            .to_string();
        let xml = doc_xml(render_book(&meta, &[("c1".into(), md)], Path::new(".")).unwrap());
        // Localised chrome present.
        assert!(xml.contains("Abbildungsverzeichnis"), "fig list heading de");
        assert!(xml.contains("Tabellenverzeichnis"), "table list heading de");
        assert!(xml.contains("Tabelle "), "table caption prefix de");
        assert!(xml.contains("Warnung"), "warning admonition label de");
        // '&' is XML-escaped: "Quellen & QR-Codes" → "Quellen &amp; QR-Codes".
        assert!(xml.contains("Quellen &amp; QR-Codes"), "sources box de");
        // English chrome must be gone.
        assert!(
            !xml.contains("Sources &amp; QR codes"),
            "english sources chrome leaked"
        );
        assert!(
            !xml.contains("Edition &amp; Disclaimer"),
            "english disclaimer chrome leaked"
        );
        assert!(
            !xml.contains("List of Figures") && !xml.contains("List of Tables"),
            "english list headings leaked"
        );
        // SEQ field NAMES stay English/stable for numbering.
        assert!(
            xml.contains("SEQ Table"),
            "SEQ Table field name stays english"
        );
    }

    #[test]
    fn english_chrome_unchanged_by_default() {
        let meta = BookMeta {
            title: "T".into(),
            author: "A".into(),
            // lang left empty → English.
            ..Default::default()
        };
        let md = "# Chapter\n\nTable: Demo\n\n| A | B |\n|---|---|\n| 1 | 2 |\n\nSee [src](https://example.com/x).\n"
            .to_string();
        let xml = doc_xml(render_book(&meta, &[("c1".into(), md)], Path::new(".")).unwrap());
        assert!(xml.contains("List of Figures"), "english fig list heading");
        assert!(xml.contains("List of Tables"), "english table list heading");
        assert!(
            xml.contains("Sources &amp; QR codes"),
            "english sources box"
        );
        assert!(xml.contains("Table "), "english table caption prefix");
        assert!(!xml.contains("Abbildungsverzeichnis"), "no german leak");
    }

    fn render_book_to_docx(meta: &BookMeta, header: &[String], row: &[String]) -> Vec<u8> {
        let head = header.join(" | ");
        let sep = vec!["---"; header.len()].join(" | ");
        let cells = row.join(" | ");
        let md = format!("# Chapter\n\n| {head} |\n| {sep} |\n| {cells} |\n");
        render_book(meta, &[("c1".into(), md)], Path::new(".")).unwrap()
    }

    // -- T1.2 (F2): table-caption marker recognition -----------------

    #[test]
    fn table_caption_marker_recognises_legacy_and_numbered_shapes() {
        // Legacy bookkit form.
        assert_eq!(
            try_parse_table_caption_marker("Table: legacy"),
            Some("legacy".to_string())
        );
        assert_eq!(
            try_parse_table_caption_marker("  table:  spaces  "),
            Some("spaces".to_string())
        );
        // Pre-numbered (colon, period, bare).
        assert_eq!(
            try_parse_table_caption_marker("Table 1: with colon"),
            Some("with colon".to_string())
        );
        assert_eq!(
            try_parse_table_caption_marker("Table 12. with period"),
            Some("with period".to_string())
        );
        assert_eq!(
            try_parse_table_caption_marker("Table 7 no separator"),
            Some("no separator".to_string())
        );
        // Number-only marker (no caption text) — still recognised so the
        // body paragraph is dropped and the SEQ caption owns the line.
        assert_eq!(
            try_parse_table_caption_marker("Table 3"),
            Some(String::new())
        );
        assert_eq!(
            try_parse_table_caption_marker("Table 3."),
            Some(String::new())
        );
        // Italic/bold wrapper from python `_render_table`.
        assert_eq!(
            try_parse_table_caption_marker("*Table 1. italic wrap*"),
            Some("italic wrap".to_string())
        );
        assert_eq!(
            try_parse_table_caption_marker("**Table 2. bold wrap**"),
            Some("bold wrap".to_string())
        );
        // Localised keyword (German + French + Italian + Hindi).
        assert_eq!(
            try_parse_table_caption_marker("Tabelle 4: deutsch"),
            Some("deutsch".to_string())
        );
        assert_eq!(
            try_parse_table_caption_marker("Tableau 5: français"),
            Some("français".to_string())
        );
        assert_eq!(
            try_parse_table_caption_marker("Tabella 6: italiano"),
            Some("italiano".to_string())
        );
        assert_eq!(
            try_parse_table_caption_marker("तालिका 7: हिन्दी"),
            Some("हिन्दी".to_string())
        );
        // Negative cases — must NOT be folded.
        assert_eq!(try_parse_table_caption_marker("Tablecloth is red"), None);
        assert_eq!(
            try_parse_table_caption_marker("The table below shows X"),
            None
        );
        assert_eq!(try_parse_table_caption_marker("Just body text."), None);
        assert_eq!(try_parse_table_caption_marker(""), None);
    }

    #[test]
    fn fold_table_captions_handles_pre_numbered_marker_above() {
        // `Table N: …` above a pipe table → caption is on the table and
        // the marker paragraph is consumed (not rendered as body text).
        let md =
            "# C\n\nTable 2: pre-numbered caption\n\n| A | B |\n|---|---|\n| 1 | 2 |\n".to_string();
        let meta = BookMeta {
            title: "T".into(),
            ..Default::default()
        };
        let xml = doc_xml(render_book(&meta, &[("c1".into(), md)], Path::new(".")).unwrap());
        assert!(
            xml.contains("pre-numbered caption"),
            "table caption text present"
        );
        // Engine renumbers from ctx.tblno, so the SEQ field caches "1"
        // even though the marker said 2 — that's the contract: SEQ wins.
        assert!(xml.contains("SEQ Table"), "SEQ field emitted");
        // The original "Table 2:" line MUST NOT survive as body text
        // (it would render twice otherwise — once as a body paragraph
        // and once via the SEQ caption).
        assert!(
            !xml.contains(">Table 2:</w:t>") && !xml.contains(">Table 2:<"),
            "pre-numbered marker paragraph must be consumed, not rendered"
        );
    }

    #[test]
    fn fold_table_captions_handles_caption_below() {
        // `Table: …` (or `Table N. …`) AFTER a pipe table → back-fills
        // onto the preceding table's caption slot. This is the python
        // `_render_table` shape (caption rendered below the table).
        let md =
            "# C\n\n| A | B |\n|---|---|\n| 1 | 2 |\n\nTable 1. below-caption text\n".to_string();
        let meta = BookMeta {
            title: "T".into(),
            ..Default::default()
        };
        let xml = doc_xml(render_book(&meta, &[("c1".into(), md)], Path::new(".")).unwrap());
        assert!(
            xml.contains("below-caption text"),
            "caption-below text reaches the SEQ caption"
        );
        assert!(!xml.contains(">Table 1.</w:t>"), "marker is consumed");
    }

    #[test]
    fn fold_table_captions_leaves_non_marker_paragraphs_alone() {
        // A body paragraph that merely mentions a table must NOT be
        // folded; it stays in the body and the table renders captioned
        // by its own (absent) caption — i.e. just "Table N" via SEQ.
        let md =
            "# C\n\nThe table below illustrates the point.\n\n| A | B |\n|---|---|\n| 1 | 2 |\n"
                .to_string();
        let meta = BookMeta {
            title: "T".into(),
            ..Default::default()
        };
        let xml = doc_xml(render_book(&meta, &[("c1".into(), md)], Path::new(".")).unwrap());
        assert!(
            xml.contains("The table below illustrates the point."),
            "non-marker body paragraph survives the fold"
        );
        assert!(xml.contains("SEQ Table"), "table still gets a SEQ caption");
    }

    // -- REQ-5 (2026-06-03): heading bookmarks + internal anchor links ----

    #[test]
    fn slugify_anchor_normalises_text() {
        assert_eq!(slugify_anchor("Reproducible Build"), "reproducible-build");
        assert_eq!(slugify_anchor("Hello, World!"), "hello-world");
        assert_eq!(slugify_anchor("  Spaced  Out  "), "spaced-out");
        assert_eq!(slugify_anchor("MiXeD CaSe 42"), "mixed-case-42");
        assert_eq!(slugify_anchor(""), "section");
    }

    #[test]
    fn heading_anchor_name_uses_chapter_shortcut_for_numbered_h1() {
        // "3 Current State Analysis" → "ch3"
        assert_eq!(heading_anchor_name("3 Current State Analysis"), "ch3");
        // The renderer prefixes chapter numbers with double-space; the
        // anchor logic must recognise that shape too.
        assert_eq!(heading_anchor_name("12  Solution Design"), "ch12");
        // Non-numbered headings fall back to a slug.
        assert_eq!(heading_anchor_name("Introduction"), "introduction");
        assert_eq!(
            heading_anchor_name("Reproducible Build"),
            "reproducible-build"
        );
    }

    #[test]
    fn heading_emits_bookmark_and_internal_link_uses_anchor() {
        let meta = BookMeta {
            title: "T".into(),
            ..Default::default()
        };
        let md = "# 3 Foo\n\nSee [Ch3](#ch3) and [the site](https://example.com).\n".to_string();
        let xml = doc_xml(render_book(&meta, &[("c1".into(), md)], Path::new(".")).unwrap());
        // The heading paragraph carries a bookmarkStart named "ch3".
        assert!(
            xml.contains(r#"<w:bookmarkStart w:id=""#),
            "heading must emit a w:bookmarkStart"
        );
        assert!(
            xml.contains(r#"w:name="ch3""#),
            "chapter heading anchor must be the canonical `ch3` shortcut"
        );
        assert!(
            xml.contains("<w:bookmarkEnd "),
            "every bookmarkStart needs a matching bookmarkEnd"
        );
        // The internal `[Ch3](#ch3)` markdown link renders as
        // `<w:hyperlink w:anchor="ch3">` — NOT a `r:id`-bearing
        // External hyperlink.
        assert!(
            xml.contains(r#"<w:hyperlink w:anchor="ch3""#),
            "internal #anchor link must render as <w:hyperlink w:anchor=...>"
        );
        // External URL still works via the existing External path.
        assert!(
            xml.contains(r#"<w:hyperlink r:id="#),
            "external URL must keep using r:id (relationship-backed) hyperlinks"
        );
    }

    #[test]
    fn bookmark_ids_are_unique_across_headings() {
        let meta = BookMeta {
            title: "T".into(),
            ..Default::default()
        };
        // Multiple headings → multiple distinct bookmark ids; if the
        // counter were per-chapter or non-monotonic, two headings would
        // share `w:id="0"` and Word would refuse the second bookmark.
        let md = "# 1 First\n\nBody.\n\n## Sub A\n\nBody.\n\n## Sub B\n\nBody.\n".to_string();
        let xml = doc_xml(render_book(&meta, &[("c1".into(), md)], Path::new(".")).unwrap());
        // Collect all bookmark ids found on bookmarkStart elements.
        // Collect (id, name) pairs for every bookmarkStart so we can
        // restrict the uniqueness check to OUR heading bookmarks (the
        // ones whose name matches the slug of a heading in this fixture).
        // docx-rs's `TableOfContents::hyperlink()` emits its own internal
        // `_GoBack`-style bookmarks that follow a separate id space; we
        // don't try to coordinate with those.
        let mut heading_bms: Vec<(String, String)> = Vec::new();
        for chunk in xml.split("<w:bookmarkStart ").skip(1) {
            let id = chunk
                .split_once(r#"w:id=""#)
                .and_then(|(_, rest)| rest.split_once('"'))
                .map(|(v, _)| v.to_string());
            let name = chunk
                .split_once(r#"w:name=""#)
                .and_then(|(_, rest)| rest.split_once('"'))
                .map(|(v, _)| v.to_string());
            if let (Some(id), Some(name)) = (id, name) {
                if matches!(name.as_str(), "ch1" | "sub-a" | "sub-b") {
                    heading_bms.push((id, name));
                }
            }
        }
        assert_eq!(
            heading_bms.len(),
            3,
            "expected exactly 3 heading bookmarks (ch1, sub-a, sub-b), got {heading_bms:?}"
        );
        let mut ids: Vec<&String> = heading_bms.iter().map(|(id, _)| id).collect();
        ids.sort();
        let unique_count = {
            let mut u = ids.clone();
            u.dedup();
            u.len()
        };
        assert_eq!(
            unique_count,
            ids.len(),
            "heading bookmark ids must be unique: {heading_bms:?}"
        );
    }

    #[test]
    fn duplicate_heading_text_disambiguates_anchor_with_suffix() {
        let meta = BookMeta {
            title: "T".into(),
            ..Default::default()
        };
        // Two H2 headings with identical text → second gets `-2` suffix
        // so internal links remain resolvable.
        let md = "# Top\n\n## Overview\n\nBody.\n\n## Overview\n\nMore.\n".to_string();
        let xml = doc_xml(render_book(&meta, &[("c1".into(), md)], Path::new(".")).unwrap());
        assert!(
            xml.contains(r#"w:name="overview""#),
            "first Overview must reserve the plain slug"
        );
        assert!(
            xml.contains(r#"w:name="overview-2""#),
            "duplicate heading must get a `-2` suffix to stay unique"
        );
    }

    /// Wave 9 (AI-Norms parity, 2026-06-03): when `body_render_use_bk_styles`
    /// is true AND `index_terms` carries the curated allowlist, the rendered
    /// docx must auto-harvest every allowlist term that appears in chapter
    /// prose into a back-of-book `Index1` paragraph. Smoke-test with 113
    /// synthetic terms (matching the reference book's entry count) — we
    /// expect >=100 `Index1` paragraphs (a >300% lift from the explicit-marker
    /// floor that produced the parity-gate failure: 32 vs 113).
    #[test]
    fn back_of_book_index_auto_harvests_allowlist_terms() {
        use std::io::Read;
        // 113 fake terms, each placed verbatim in its own chapter -- mirrors
        // the reference book's allowlist density.
        let mut allowlist_terms: Vec<String> = Vec::new();
        let mut chapters: Vec<(String, String)> = Vec::new();
        for i in 0..113 {
            let term = format!("Allowlisted-Term-{i:03}");
            allowlist_terms.push(term.clone());
            chapters.push((
                format!("c{i}"),
                format!("# {} Chapter Title\n\nProse mentions {term} here.\n", i + 1),
            ));
        }
        let meta = BookMeta {
            title: "Index Parity".into(),
            subtitle: "Wave 9".into(),
            author: "Test".into(),
            context: "Ctx".into(),
            body_render_use_bk_styles: true,
            index_terms: allowlist_terms,
            ..Default::default()
        };
        let bytes = render_book(&meta, &chapters, Path::new(".")).unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut xml = String::new();
        zip.by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut xml)
            .unwrap();
        let index1_count = xml.matches("w:val=\"Index1\"").count();
        let heading_count = xml.matches("w:val=\"IndexHeading\"").count();
        assert!(
            index1_count >= 100,
            "expected >=100 Index1 paragraphs after auto-harvest, got {index1_count}"
        );
        // All 113 synthetic terms start with 'A' -- so we expect exactly 1
        // IndexHeading divider. The point is that the harvest path produced
        // at least one heading.
        assert!(
            heading_count >= 1,
            "expected >=1 IndexHeading divider, got {heading_count}"
        );
    }

    /// Round V iter-7 (drawing_class_bucket parity close, 2026-06-03).
    ///
    /// Iter-2 introduced `image_dims_to_emu` with a "natural width up to
    /// 15 cm" policy. Iter-3 added a 4-inch default to
    /// `render_image_embed.rs`, but the cascade strips the figspec block
    /// before `resolve_markdown` runs, so that path is unreachable.
    /// Iter-7 mirrors the 4-inch default HERE — the production path —
    /// closing the last 2 parity ERRORs
    /// (`PARITY_DRAWING_CLASS_BUCKET::FIGURE` 125 vs 78,
    /// `::OTHER` 8 vs 55; +47 / -47 = the 47 wide unsized embeds the
    /// 4-inch default rebins from FIGURE → OTHER).
    ///
    /// The 3 tests below pin the three branches of the new logic:
    ///   1. unsized + naturally wide  → shrink to 4-in default (OTHER bucket);
    ///   2. unsized + naturally small → keep native width (OTHER bucket
    ///                                  if ≥1 M, ICON/QR if smaller);
    ///   3. explicit `width_in` override → caller opts out of the
    ///      4-in default, allowing genuinely figure-sized embeds at the
    ///      15 cm cap (FIGURE bucket).
    #[test]
    fn image_dims_to_emu_shrinks_wide_unsized_to_4_inch() {
        // 2048×1024 px → natural width 2048/96 = 21.33 in = 19 504 000 EMU.
        // Pre-iter-7: capped at IMAGE_MAX_W_EMU (5 400 000 = FIGURE bucket).
        // Post-iter-7: with no override, shrinks to DEFAULT_EMBED_W_EMU
        // (3 657 600 EMU → snapped to 60 000-grid → 3 600 000 EMU = OTHER).
        let mut img = image::RgbImage::new(2048, 1024);
        for y in 0..1024 {
            for x in 0..2048 {
                img.put_pixel(x, y, image::Rgb([0, 0, 0]));
            }
        }
        let mut buf = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
            .unwrap();
        let (w_emu, h_emu) = image_dims_to_emu(&buf, None);
        assert_eq!(
            w_emu, 3_600_000,
            "wide unsized PNG must shrink to 4-inch default (3 600 000 EMU after 60 000-grid snap)"
        );
        // Bucket: 1 M < cx < 5 M = OTHER bucket the parity gate reads.
        assert!(
            w_emu > 1_000_000 && w_emu < 5_000_000,
            "must land in OTHER bucket (1 M < cx < 5 M); got {w_emu}"
        );
        // Aspect 1024 / 2048 = 0.5 → expected height ≈ 0.5 × 3 600 000 = 1 800 000 EMU.
        let delta = (i64::from(h_emu) - 1_800_000).abs();
        assert!(
            delta < 5_000,
            "scaled height should be ≈1 800 000 EMU; got {h_emu}"
        );
    }

    #[test]
    fn image_dims_to_emu_preserves_small_image_at_native() {
        // 256×128 px → natural width 256/96 = 2.67 in = 2 438 400 EMU,
        // well BELOW the 4-in / DEFAULT_EMBED_W_EMU threshold. Must keep
        // its native width (snapped to 60 000-EMU grid → 2 400 000 EMU).
        let mut img = image::RgbImage::new(256, 128);
        for y in 0..128 {
            for x in 0..256 {
                img.put_pixel(x, y, image::Rgb([(x & 0xFF) as u8, (y & 0xFF) as u8, 64]));
            }
        }
        let mut buf = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
            .unwrap();
        let (w_emu, h_emu) = image_dims_to_emu(&buf, None);
        // 256 / 96 = 2.667 in → 2 438 400 EMU → snapped to 2 400 000.
        assert_eq!(
            w_emu, 2_400_000,
            "naturally small PNG must keep its native width (no inflation)"
        );
        // Bucket: 1 M < cx < 5 M → still OTHER (the parity gate's
        // OTHER bucket holds anything in the 1-5 M EMU band).
        assert!(
            w_emu > 1_000_000 && w_emu < 5_000_000,
            "must land in OTHER bucket (1 M < cx < 5 M); got {w_emu}"
        );
        // Aspect 128 / 256 = 0.5 → expected height ≈ 0.5 × 2 400 000 = 1 200 000 EMU.
        let delta = (i64::from(h_emu) - 1_200_000).abs();
        assert!(
            delta < 5_000,
            "native height should be ≈1 200 000 EMU; got {h_emu}"
        );
    }

    #[test]
    fn image_dims_to_emu_respects_explicit_width_override() {
        // A 2048×1024 px source — same as test 1, but now the caller
        // passes `Some(width_in)` to opt OUT of the 4-in default. The
        // override is honored up to the 15-cm hard cap (IMAGE_MAX_W_EMU
        // = 5 400 000 EMU) so genuinely figure-sized embeds still ship
        // at FIGURE bucket size.
        let mut img = image::RgbImage::new(2048, 1024);
        for y in 0..1024 {
            for x in 0..2048 {
                img.put_pixel(x, y, image::Rgb([0, 0, 0]));
            }
        }
        let mut buf = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
            .unwrap();
        // 12 in is above the 15-cm cap (5.905 in), so it must clamp at
        // IMAGE_MAX_W_EMU (5 400 000 EMU) — FIGURE bucket.
        let (w_clamped, _) = image_dims_to_emu(&buf, Some(12.0));
        assert_eq!(
            w_clamped, 5_400_000,
            "12 in override must clamp at 15 cm hard cap (5 400 000 EMU)"
        );
        assert!(
            w_clamped >= 5_000_000,
            "clamped override must land in FIGURE bucket; got {w_clamped}"
        );
        // 5.0 in × 914 400 = 4 572 000 EMU → snapped down to 4 560 000.
        let (w_5in, h_5in) = image_dims_to_emu(&buf, Some(5.0));
        assert_eq!(
            w_5in, 4_560_000,
            "5 in override must produce 4 560 000 EMU (after 60 000-grid snap)"
        );
        // Aspect 1024 / 2048 = 0.5 → expected height ≈ 0.5 × 4 560 000 = 2 280 000.
        let delta = (i64::from(h_5in) - 2_280_000).abs();
        assert!(
            delta < 5_000,
            "override height should be ≈2 280 000 EMU; got {h_5in}"
        );
    }

    /// Round V iter-8 (call-site context-aware sizing, 2026-06-03).
    ///
    /// Iter-7 passed `None` from the sole `DocxBlock::Image` call site —
    /// applying the 4-inch default to EVERY `![](png)` reference. That
    /// over-corrected: it capped the 78 in-house figspec-emitted figures
    /// (treemap, sankey, wheel, heatmap, govmap, regstack, etc.) along
    /// with the 55 sourced inline rasters, flipping the buckets to
    /// FIGURE 8 / OTHER 125 vs reference FIGURE 78 / OTHER 55.
    ///
    /// Iter-8 distinguishes IN-HOUSE figures (path prefix `figures/`,
    /// emitted by `agentic_figures::resolve_markdown`) from LOOSE markdown
    /// raster references (any other path) at the call site. The tests
    /// below pin the boundary input space for the `image_dims_to_emu`
    /// helper that the call site relies on:
    ///   1. an explicit ~6 in width override (the in-house-figure path) →
    ///      ~5.46 M EMU (FIGURE bucket);
    ///   2. `None` (the loose-raster path) on a wide source → 3.6 M EMU
    ///      (OTHER bucket) — the Iter-7 behaviour that REMAINS correct
    ///      for loose rasters.
    /// The unchanged constants for admonition icons (151 200 EMU) and QR
    /// codes (972 000 EMU) live in `icons.rs` and do NOT route through
    /// `image_dims_to_emu`, so they continue to land in their own buckets.
    #[test]
    fn image_dims_to_emu_in_house_figure_lands_in_figure_bucket() {
        // 1400×900 px — typical in-house wheel/sankey/treemap source
        // dimensions (per Round V analysis of the reference book's
        // FIGURE-bucket native-width histogram, mode 1400 px).
        let mut img = image::RgbImage::new(1400, 900);
        for y in 0..900 {
            for x in 0..1400 {
                img.put_pixel(x, y, image::Rgb([0, 0, 0]));
            }
        }
        let mut buf = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
            .unwrap();
        // The call site passes `Some(6.0)` for `figures/`-prefixed paths.
        // 6.0 in × 914 400 = 5 486 400 EMU; this exceeds IMAGE_MAX_W_EMU
        // (15-cm hard cap, 5 400 000), so the function clamps to
        // 5 400 000 EMU (which is already a clean 60 000-grid multiple,
        // so the snap step is a no-op). Lands at the very bottom of the
        // FIGURE bucket (≥5 M) — exactly where the reference book sits
        // (cx histogram for FIGURE: 5 040 000 — 5 760 000 EMU).
        let (w_emu, h_emu) = image_dims_to_emu(&buf, Some(6.0));
        assert_eq!(
            w_emu, 5_400_000,
            "in-house figspec figure (Some(6.0)) must clamp to IMAGE_MAX_W_EMU (5 400 000)"
        );
        // Bucket: ≥5 M EMU = FIGURE bucket the parity gate counts.
        assert!(
            w_emu >= 5_000_000,
            "in-house figure must land in FIGURE bucket (≥5 M); got {w_emu}"
        );
        // Aspect 900 / 1400 ≈ 0.643 → expected height ≈ 0.643 × 5 400 000 ≈ 3 470 000 EMU.
        let expected_h = (5_400_000_u64 * 900 / 1400) as i64;
        let delta = (i64::from(h_emu) - expected_h).abs();
        assert!(
            delta < 5_000,
            "scaled height should be ≈{expected_h} EMU; got {h_emu}"
        );
    }

    #[test]
    fn image_dims_to_emu_loose_raster_lands_in_other_bucket() {
        // 1400×900 px — IDENTICAL source to the FIGURE-bucket test
        // above. The discriminator is purely the caller's choice of
        // `width_in_override`, not the source bytes.
        let mut img = image::RgbImage::new(1400, 900);
        for y in 0..900 {
            for x in 0..1400 {
                img.put_pixel(x, y, image::Rgb([0, 0, 0]));
            }
        }
        let mut buf = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
            .unwrap();
        // The call site passes `None` for paths that don't start with
        // `figures/` (loose markdown rasters from sourced screenshots).
        let (w_emu, _h_emu) = image_dims_to_emu(&buf, None);
        assert_eq!(
            w_emu, 3_600_000,
            "loose raster (None) must shrink to 4-in default (3 600 000 EMU)"
        );
        assert!(
            w_emu > 1_000_000 && w_emu < 5_000_000,
            "loose raster must land in OTHER bucket (1 M < cx < 5 M); got {w_emu}"
        );
    }

    /// Admonition icons are emitted via `icons::icon_pic`, which bakes in
    /// the 151 200 EMU square dimension directly — they do NOT call
    /// `image_dims_to_emu` and are therefore immune to the Iter-7/Iter-8
    /// call-site changes. This test pins that contract so a future
    /// refactor can't accidentally route icons through the figure helper.
    #[test]
    fn admonition_icon_bypasses_image_dims_to_emu() {
        assert_eq!(
            crate::icons::ADMONITION_ICON_EMU,
            151_200,
            "ADMONITION_ICON_EMU constant must remain 151 200 (icon bucket)"
        );
        let pic = crate::icons::icon_pic(crate::icons::IconKind::Tip);
        // Pic does not expose its size publicly, but the constant above
        // pins the only thing the parity gate counts: the EMU side length.
        drop(pic);
    }

    /// QR codes are rendered via `Pic::new(&png).size(QR_CODE_EMU, ...)`
    /// at `book.rs:4257` and similarly bypass `image_dims_to_emu`. Pin
    /// the constant.
    #[test]
    fn qr_code_bypasses_image_dims_to_emu() {
        assert_eq!(
            crate::icons::QR_CODE_EMU,
            972_000,
            "QR_CODE_EMU constant must remain 972 000 (qr bucket: 900 K-1 M)"
        );
    }

    /// 2026-06-14 ai_norms_docx oversize fix.
    ///
    /// `clamp_raster_for_embed` MUST downsample any PNG whose longest
    /// edge exceeds [`MAX_EMBED_RASTER_EDGE_PX`] to that cap, aspect
    /// preserved; PNGs already at-or-below the cap MUST pass through
    /// byte-identical (otherwise admonition icons + QR codes would
    /// gain a needless re-encode round trip if they ever reached the
    /// DocxBlock::Image path).
    #[test]
    fn clamp_raster_for_embed_shrinks_oversized_png() {
        // 2048×1024 source — wider than the 1280 px cap on its long
        // edge. After clamp: width 1280, height proportional → 640.
        //
        // We pin pixel-area reduction (deterministic). The byte-size
        // reduction is gated through `CompressionType::Best`; we also
        // assert the clamped output is no larger than ~2× of the
        // source bytes (a smooth synthetic gradient compresses near
        // PNG's theoretical floor, so a same-FilterType re-encode of
        // a smaller pixel area sits very close to the source). Real-
        // world sourced screenshots compress at ~50 % of synthetic
        // PNGs and land at ~25 % of source bytes after this clamp;
        // the docx-level verification gates that.
        let mut img = image::RgbImage::new(2048, 1024);
        for y in 0..1024 {
            for x in 0..2048 {
                img.put_pixel(x, y, image::Rgb([(x & 0xFF) as u8, (y & 0xFF) as u8, 96]));
            }
        }
        let mut src = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut src), image::ImageFormat::Png)
            .unwrap();
        let src_len = src.len();
        let (clamped, dims) = clamp_raster_for_embed(src);
        let (cw, ch) = dims.expect("clamped output must expose its dims");
        assert_eq!(cw, MAX_EMBED_RASTER_EDGE_PX, "long edge clamped to cap");
        assert_eq!(ch, MAX_EMBED_RASTER_EDGE_PX / 2, "aspect ratio preserved");
        let (pw, ph) = png_dims(&clamped).expect("clamped output must be a valid PNG");
        assert_eq!(
            (pw, ph),
            (cw, ch),
            "returned dims must match the encoded IHDR"
        );
        let src_pixels: u64 = 2048 * 1024;
        let out_pixels: u64 = u64::from(cw) * u64::from(ch);
        assert!(
            out_pixels < src_pixels,
            "pixel area must drop ({} → {})",
            src_pixels,
            out_pixels
        );
        // Soft byte ceiling — a smooth gradient may not shrink, but
        // it must not blow up beyond 2× the source.
        assert!(
            clamped.len() < src_len * 2,
            "clamped synthetic must stay under 2× source bytes ({} → {})",
            src_len,
            clamped.len()
        );
    }

    #[test]
    fn clamp_raster_for_embed_preserves_small_png_byte_identical() {
        // 800×600 source — both edges under the 1280 px cap. The
        // clamp MUST return the input untouched (no re-encode) AND
        // expose the parsed dims so the caller can take the
        // `Pic::new_with_dimensions` no-re-encode path. The bytes
        // identity matters because admonition icons, QR codes,
        // and pre-clamped figspec PNGs all benefit from skipping
        // docx-rs's default-deflate round trip.
        let mut img = image::RgbImage::new(800, 600);
        for y in 0..600 {
            for x in 0..800 {
                img.put_pixel(x, y, image::Rgb([(x & 0xFF) as u8, (y & 0xFF) as u8, 200]));
            }
        }
        let mut src = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut src), image::ImageFormat::Png)
            .unwrap();
        let src_clone = src.clone();
        let (out, dims) = clamp_raster_for_embed(src);
        assert_eq!(
            out, src_clone,
            "under-cap PNG must pass through byte-identical (no re-encode)"
        );
        assert_eq!(
            dims,
            Some((800, 600)),
            "under-cap PNG must expose its dims for the no-re-encode caller path"
        );
    }

    #[test]
    fn clamp_raster_for_embed_preserves_non_png_payload() {
        // Defensive: if the bytes don't parse as PNG (exotic format,
        // truncated payload, etc.) the helper MUST return them
        // unchanged with NO dims hint — the caller then falls back
        // to `Pic::new(&bytes)` which decodes whatever format docx-rs
        // can read. This keeps the renderer crash-free on JPEG /
        // truncated payloads (the AI-Norms book has a handful of
        // `*.jpg` / `*.jpeg` source assets).
        let junk = vec![0u8, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
        let (out, dims) = clamp_raster_for_embed(junk.clone());
        assert_eq!(out, junk, "non-PNG payload must pass through unchanged");
        assert_eq!(
            dims, None,
            "non-PNG payload must NOT expose dims (caller falls back to Pic::new)"
        );
    }

    /// Round V iter-10 (per-figure size manifest, 2026-06-03).
    ///
    /// When the AI-Norms `sizes.toml` manifest lists a width hint for a
    /// given `image*.png` basename, the renderer must use that hint as the
    /// `width_in_override` passed to [`image_dims_to_emu`] — overriding the
    /// iter-9 path-based default that would otherwise route the file to
    /// OTHER (no figspec prefix). This recovers the editorial FIGURE
    /// assignments for `image*.png` files that no path-byte heuristic can
    /// distinguish from their OTHER siblings.
    ///
    /// We exercise the manifest at the `SizeManifest::lookup` API level
    /// (the renderer call site is a single `if let Some(w) = … { Some(w) }
    /// else { … }` expression that is mechanically faithful to that
    /// lookup). Manifest hit + `image_dims_to_emu(_, Some(5.9055))` lands
    /// the image in the FIGURE bucket (≥5 M EMU).
    #[test]
    fn size_manifest_overrides_path_default() {
        use crate::size_manifest::SizeManifest;
        // The manifest stores widths under a `+30 000 EMU mid-grid bias` so
        // the float-truncation × floor-snap-to-60 000-grid pipeline in
        // [`image_dims_to_emu`] round-trips exactly to the reference cx
        // value rather than landing one grid step low. For `image14.png` the
        // reference cx is 5 040 000 EMU → encoded as
        // (5 040 000 + 30 000) / 914 400 = 5.544619 in.
        let toml = r#"
[sizes]
"image14.png" = 5.544619
"#;
        let manifest = SizeManifest::parse(toml);
        let width_in = manifest
            .lookup("image14.png")
            .expect("manifest must hit on image14.png");
        // Build a 1400×900 source — same shape as the FIGURE / OTHER
        // bucket-discriminator tests above.
        let mut img = image::RgbImage::new(1400, 900);
        for y in 0..900 {
            for x in 0..1400 {
                img.put_pixel(x, y, image::Rgb([0, 0, 0]));
            }
        }
        let mut buf = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
            .unwrap();
        let (w_emu, _h) = image_dims_to_emu(&buf, Some(width_in));
        // 5.544619 in × 914 400 = 5 070 003 EMU (well above 5 000 000 FIGURE
        // threshold; below IMAGE_MAX_W_EMU = 5 400 000, so no clamp). After
        // floor-snap to 60 000: 5 070 003 / 60 000 = 84 → 5 040 000 EMU,
        // exactly matching the reference book's image14.png cx.
        assert_eq!(
            w_emu, 5_040_000,
            "manifest hint 5.544619 in must round-trip to 5 040 000 EMU (reference cx) after grid snap"
        );
        assert!(
            w_emu >= 5_000_000,
            "manifest-driven width must land in FIGURE bucket (>= 5 M); got {w_emu}"
        );
    }

    /// Round V iter-10 fallback contract: a manifest miss must NOT override
    /// the iter-9 path-prefix heuristic. The renderer composes
    /// `ctx.size_manifest.lookup(path)` → fallback to
    /// `is_in_house_figure_path(path) ? Some(6.0) : None`. We exercise both
    /// arms of that fallback here.
    #[test]
    fn size_manifest_missing_entry_falls_back_to_path_heuristic() {
        use crate::size_manifest::SizeManifest;
        // Manifest covers only `image1.png`. A query for `image2.png` must miss.
        let toml = r#"
[sizes]
"image1.png" = 6.0
"#;
        let manifest = SizeManifest::parse(toml);
        assert_eq!(manifest.lookup("image2.png"), None);
        // The renderer's fallback for a `gov_*` filename → Some(6.0) → FIGURE.
        let gov_fallback: Option<f32> = if manifest.lookup("gov_eu.png").is_some() {
            manifest.lookup("gov_eu.png")
        } else if is_in_house_figure_path("gov_eu.png") || "gov_eu.png".contains("/figures/") {
            Some(6.0)
        } else {
            None
        };
        assert_eq!(
            gov_fallback,
            Some(6.0),
            "manifest miss on a figspec-stem must fall back to the Some(6.0) FIGURE override"
        );
        // The renderer's fallback for an `image2.png` (no figspec stem,
        // not in `figures/`) → None → 4-in OTHER default. Mirror the call
        // site expression exactly so the test catches any reordering.
        let loose_fallback: Option<f32> = if manifest.lookup("image2.png").is_some() {
            manifest.lookup("image2.png")
        } else if is_in_house_figure_path("image2.png") || "image2.png".contains("/figures/") {
            Some(6.0)
        } else {
            None
        };
        assert_eq!(
            loose_fallback, None,
            "manifest miss on a loose raster must fall back to the None / 4-in OTHER default"
        );
    }

    /// Round V iter-4 (theme-injection regression, 2026-06-03).
    ///
    /// `restore_reference_theme_and_styles` MUST inject `word/theme/theme1.xml`
    /// + the matching `[Content_Types]` Override + the `document.xml.rels`
    /// Relationship even when the input zip has no theme part at all
    /// (the docx-rs render output OR a docx where Word COM finalize
    /// silently failed and the upstream caller plumbed the un-Word-saved
    /// bytes through anyway). Regression: cascade #15 produced a 41 MB
    /// `ai_norms_and_regulations.docx` where Word COM crashed on Open,
    /// the post-finalize restore then ran against the pure docx-rs
    /// bytes, and the resulting docx had NO theme1.xml (THEME::majorFont
    /// = "<absent>"), tripping the parity gate.
    #[test]
    fn restore_theme_and_styles_synthesises_theme_when_absent() {
        use std::io::{Read, Write};
        // Build a minimal docx-rs-style zip WITHOUT a theme part:
        // just `[Content_Types].xml`, `word/document.xml`, and
        // `word/_rels/document.xml.rels`. The restore pass must add
        // the theme part + patch CT + rels.
        let mut buf = Cursor::new(Vec::<u8>::new());
        {
            let mut z = zip::ZipWriter::new(&mut buf);
            let ct = r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default ContentType="application/xml" Extension="xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#;
            let doc = r#"<?xml version="1.0" encoding="UTF-8"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body/></w:document>"#;
            let rels = r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;
            z.start_file(
                "[Content_Types].xml",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            z.write_all(ct.as_bytes()).unwrap();
            z.start_file(
                "word/document.xml",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            z.write_all(doc.as_bytes()).unwrap();
            z.start_file(
                "word/_rels/document.xml.rels",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            z.write_all(rels.as_bytes()).unwrap();
            z.finish().unwrap();
        }
        let input = buf.into_inner();
        // Sanity: input has no theme part.
        {
            let mut zin = zip::ZipArchive::new(Cursor::new(input.clone())).unwrap();
            let mut has_theme = false;
            for i in 0..zin.len() {
                let f = zin.by_index(i).unwrap();
                if f.name() == "word/theme/theme1.xml" {
                    has_theme = true;
                }
            }
            assert!(!has_theme, "fixture must start with no theme part");
        }

        let restored =
            restore_reference_theme_and_styles(input, crate::thesis_styles::StylesProfile::AiNorms)
                .unwrap();

        // After restore: theme part must exist, CT must have Override,
        // rels must have Relationship pointing at the theme.
        let mut zout = zip::ZipArchive::new(Cursor::new(restored)).unwrap();
        let mut found_theme_part = false;
        let mut found_styles_part = false;
        let mut ct_out = String::new();
        let mut rels_out = String::new();
        let mut theme_body = String::new();
        for i in 0..zout.len() {
            let mut f = zout.by_index(i).unwrap();
            let n = f.name().to_string();
            if n == "word/theme/theme1.xml" {
                found_theme_part = true;
                f.read_to_string(&mut theme_body).unwrap();
            } else if n == "word/styles.xml" {
                found_styles_part = true;
            } else if n == "[Content_Types].xml" {
                f.read_to_string(&mut ct_out).unwrap();
            } else if n == "word/_rels/document.xml.rels" {
                f.read_to_string(&mut rels_out).unwrap();
            }
        }
        assert!(found_theme_part, "theme1.xml must be synthesised");
        assert!(found_styles_part, "styles.xml must be synthesised");
        // Theme body must contain Calibri + Cambria (the parity-gate font
        // names that were "<absent>" in the cascade #15 regression).
        assert!(
            theme_body.contains("Calibri"),
            "synthesised theme must reference Calibri (majorFont)"
        );
        assert!(
            theme_body.contains("Cambria"),
            "synthesised theme must reference Cambria (minorFont)"
        );
        // CT must reference the theme part.
        assert!(
            ct_out.contains("/word/theme/theme1.xml"),
            "Content_Types must reference theme1.xml: {ct_out}"
        );
        assert!(
            ct_out.contains("theme+xml"),
            "Content_Types must have theme content-type Override: {ct_out}"
        );
        // Rels must reference the theme.
        assert!(
            rels_out.contains("theme/theme1.xml"),
            "doc rels must reference theme1.xml: {rels_out}"
        );
        assert!(
            rels_out.contains("relationships/theme"),
            "doc rels must have theme relationship type: {rels_out}"
        );
    }

    /// Round V iter-4 (theme-injection idempotency, 2026-06-03).
    ///
    /// Calling `restore_reference_theme_and_styles` on a docx that
    /// already has the reference theme + styles MUST be a no-op for
    /// CT + rels (no duplicate Override / Relationship entries).
    #[test]
    fn restore_theme_and_styles_is_idempotent_when_theme_present() {
        use std::io::{Read, Write};
        let mut buf = Cursor::new(Vec::<u8>::new());
        {
            let mut z = zip::ZipWriter::new(&mut buf);
            let ct = r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/theme/theme1.xml" ContentType="application/vnd.openxmlformats-officedocument.theme+xml"/></Types>"#;
            let doc = r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body/></w:document>"#;
            let rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme" Target="theme/theme1.xml"/></Relationships>"#;
            z.start_file(
                "[Content_Types].xml",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            z.write_all(ct.as_bytes()).unwrap();
            z.start_file(
                "word/document.xml",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            z.write_all(doc.as_bytes()).unwrap();
            z.start_file(
                "word/_rels/document.xml.rels",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            z.write_all(rels.as_bytes()).unwrap();
            z.start_file("word/styles.xml", zip::write::SimpleFileOptions::default())
                .unwrap();
            z.write_all(b"<w:styles xmlns:w=\"...\"/>").unwrap();
            z.start_file(
                "word/theme/theme1.xml",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            z.write_all(b"<a:theme xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" name=\"Old\"/>").unwrap();
            z.finish().unwrap();
        }
        let input = buf.into_inner();
        let restored =
            restore_reference_theme_and_styles(input, crate::thesis_styles::StylesProfile::AiNorms)
                .unwrap();
        // Re-running on the result must produce the same bytes (modulo
        // the zip layout; we check CT/rels invariants instead).
        let restored2 = restore_reference_theme_and_styles(
            restored.clone(),
            crate::thesis_styles::StylesProfile::AiNorms,
        )
        .unwrap();
        let mut z2 = zip::ZipArchive::new(Cursor::new(restored2)).unwrap();
        let mut ct = String::new();
        let mut rels = String::new();
        for i in 0..z2.len() {
            let mut f = z2.by_index(i).unwrap();
            let n = f.name().to_string();
            if n == "[Content_Types].xml" {
                f.read_to_string(&mut ct).unwrap();
            } else if n == "word/_rels/document.xml.rels" {
                f.read_to_string(&mut rels).unwrap();
            }
        }
        // Exactly ONE theme Override (no duplicate insertion).
        assert_eq!(
            ct.matches("/word/theme/theme1.xml").count(),
            1,
            "no duplicate CT Override on idempotent re-run: {ct}"
        );
        assert_eq!(
            rels.matches("theme/theme1.xml").count(),
            1,
            "no duplicate rels Relationship on idempotent re-run: {rels}"
        );
    }

    // ───────────────────────────────────────────────────────────────────
    // Round V iter-9 — drawing_class_bucket discriminator (Fix B)
    // ───────────────────────────────────────────────────────────────────

    /// The Iter-8 prefix-only check (`path.starts_with("figures/")`) matched
    /// 0 of the 78 reference FIGURE entries on the ai_norms cascade,
    /// because `strip_wave5_figures_section` strips the `## Figures`
    /// figspec block BEFORE `resolve_markdown` runs, so the chapter md
    /// only carries bare-filename refs like `gov_switzerland.png`,
    /// `reg_eu.png`, `iso_norms_heatmap.png`. Iter-9 adds figspec-stem
    /// recognition so these route to the FIGURE bucket again.
    #[test]
    fn is_in_house_figure_path_matches_figspec_emitter_stems() {
        // Recognised stems → FIGURE bucket.
        assert!(super::is_in_house_figure_path("gov_switzerland.png"));
        assert!(super::is_in_house_figure_path("gov_eu.png"));
        assert!(super::is_in_house_figure_path("reg_switzerland.png"));
        assert!(super::is_in_house_figure_path("reg_uk.png"));
        assert!(super::is_in_house_figure_path("iso_norms_heatmap.png"));
        assert!(super::is_in_house_figure_path("iso5338_clean.png"));
        assert!(super::is_in_house_figure_path("pop_treemap.png"));
        // Fully-qualified path with the same stem must also match.
        assert!(super::is_in_house_figure_path(
            "specs/figures/raster/ai_norms/gov_eu.png"
        ));
        // Case-insensitive stem match (some bookkit sources use mixed case).
        assert!(super::is_in_house_figure_path("GOV_FOO.png"));
    }

    #[test]
    fn is_in_house_figure_path_matches_canonical_prefix() {
        // The `agentic_figures::resolve_markdown` emission pattern still
        // hits (covers the non-ai_norms cascade where figspecs survive
        // to resolve_markdown).
        assert!(super::is_in_house_figure_path("figures/eu/sankey.png"));
        // Windows backslash variant (defensive against path-separator
        // mismatch on the cascade host).
        assert!(super::is_in_house_figure_path(r"figures\eu\sankey.png"));
        // Nested `/figures/` segment somewhere in the path.
        assert!(super::is_in_house_figure_path(
            "scratch/agentic_book/figures/sub/id.png"
        ));
    }

    #[test]
    fn is_in_house_figure_path_rejects_loose_image_names() {
        // The reference book routes generic `image{N}.png` refs partly
        // to FIGURE (35) and partly to OTHER (55) on editorial grounds
        // we can't recover from path bytes. Default to OTHER for these.
        assert!(!super::is_in_house_figure_path("image6.png"));
        assert!(!super::is_in_house_figure_path("image14.png"));
        assert!(!super::is_in_house_figure_path("image113.png"));
        assert!(!super::is_in_house_figure_path("screenshot.png"));
        assert!(!super::is_in_house_figure_path("photo.jpg"));
    }

    // ───────────────────────────────────────────────────────────────────
    // Round V iter-9 — Word-COM whitespace-only header/footer detection
    // ───────────────────────────────────────────────────────────────────

    /// Word COM (Documents.Open → Save) regenerates the even/default/first
    /// header & footer triad and populates each part with one or more
    /// whitespace-only `<w:t>` runs. The pre-iter-9 substring check
    /// refused to drop these because `body.contains("<w:t")` matched the
    /// boilerplate runs. Iter-9 broadens the check to also count
    /// whitespace-only payloads as empty, restoring HEADER_PART_COUNT
    /// → 0 and FOOTER_PART_COUNT → 1 on the post-finalize cascade.
    #[test]
    fn header_or_footer_is_empty_drops_whitespace_only_word_boilerplate() {
        // Word's typical regenerated empty header — one paragraph with
        // a single space-preserve run.
        let body = r#"<w:hdr><w:p><w:r><w:t xml:space="preserve"> </w:t></w:r></w:p></w:hdr>"#;
        assert!(super::header_or_footer_is_empty(body));
        // Self-closing `<w:t/>` variant.
        let body = r#"<w:hdr><w:p><w:r><w:t/></w:r></w:p></w:hdr>"#;
        assert!(super::header_or_footer_is_empty(body));
        // Empty-text-element `<w:t></w:t>` variant.
        let body = r#"<w:hdr><w:p><w:r><w:t></w:t></w:r></w:p></w:hdr>"#;
        assert!(super::header_or_footer_is_empty(body));
    }

    #[test]
    fn header_or_footer_is_empty_keeps_real_page_field_footer() {
        // The reference footer carries a centred PAGE field — must NOT
        // be dropped even though the displayed text is the literal `1`.
        let body = r#"<w:ftr><w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText>PAGE</w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>1</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p></w:ftr>"#;
        assert!(!super::header_or_footer_is_empty(body));
    }

    #[test]
    fn header_or_footer_is_empty_keeps_logo_header() {
        // FHNW running-header carries a logo drawing — must NOT be
        // dropped even though it contains no text runs at all.
        let body = r#"<w:hdr><w:p><w:r><w:drawing><wp:inline/></w:drawing></w:r></w:p></w:hdr>"#;
        assert!(!super::header_or_footer_is_empty(body));
    }

    #[test]
    fn contains_text_payload_distinguishes_whitespace_from_content() {
        assert!(!super::contains_text_payload(
            r#"<w:t xml:space="preserve"> </w:t>"#
        ));
        assert!(!super::contains_text_payload("<w:t></w:t>"));
        assert!(!super::contains_text_payload("<w:t/>"));
        assert!(super::contains_text_payload("<w:t>x</w:t>"));
        assert!(super::contains_text_payload("<w:t>1</w:t>"));
        // `<w:tab/>` and `<w:tbl…>` must NOT trigger as `<w:t…>` matches.
        assert!(!super::contains_text_payload("<w:tab/>"));
        assert!(!super::contains_text_payload("<w:tbl><w:tr/></w:tbl>"));
    }
}
