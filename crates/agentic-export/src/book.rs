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
    AlignmentType, BorderType, BreakType, Docx, FieldCharType, Footer, Header, HeightRule,
    Hyperlink, HyperlinkType, InstrText, LineSpacing, LineSpacingType, PageMargin, PageNum,
    PageOrientationType, PageSize, Paragraph, Pic, Run, RunFonts, SectionProperty, Shading, Style,
    StyleType, Table, TableCell, TableCellBorder, TableCellBorderPosition, TableCellMargins,
    TableLayoutType, TableOfContents, TableRow, TextDirectionType, VAlignType, VertAlignType,
    WidthType,
};

use agentic_core::i18n::t;

use crate::markdown::{DocxBlock, DocxRun, to_docx_blocks};

const NAVY: &str = "1F3864"; // gold bookkit HEAD (book_build): headings + title
const HEAD2: &str = "2E4A7A";
const GREY: &str = "666666";
const ACCENT: &str = "0B5C9E"; // hyperlink blue
const HEADBG: &str = "1F3864";
const ALTBG: &str = "F4F6FA";
const RULE: &str = "C9D2E0";
const BODY: &str = "Georgia";
const HEADF: &str = "Calibri";
const MONO: &str = "Consolas";

const CONTENT_TWIPS: usize = 9298; // A4 (11906) − 2×1304 margins

// Relaxed readability (ADR-0030): 1.5 line spacing + breathing room after each
// block so text and tables are not packed edge-to-edge.
const LINE_15: i32 = 360; // 1.5× single (240 = single)
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

/// 1.5-spaced body paragraph spacing with a little room after the block.
fn body_spacing() -> LineSpacing {
    LineSpacing::new()
        .line_rule(LineSpacingType::Auto)
        .line(LINE_15)
        .after(SPACE_AFTER)
}

/// The standard page margins, shared by the body and every mid-document section.
fn std_margin() -> PageMargin {
    PageMargin::new()
        .top(1417)
        .bottom(1417)
        .left(1304)
        .right(1304)
}

/// `sectPr` for a portrait A4 section (default next-page break).
fn portrait_sectpr() -> SectionProperty {
    SectionProperty::new()
        .page_size(PageSize::new().size(11906, 16838))
        .page_margin(std_margin())
}

/// `sectPr` for a landscape A4 section (default next-page break).
fn landscape_sectpr() -> SectionProperty {
    SectionProperty::new()
        .page_size(
            PageSize::new()
                .size(16838, 11906)
                .orient(PageOrientationType::Landscape),
        )
        .page_margin(std_margin())
}

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

#[derive(Debug, Clone, Default)]
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

/// Body run-fonts for the active typography profile.
fn body_fonts_for(p: TypographyProfile) -> RunFonts {
    match p {
        TypographyProfile::Designer => body_fonts(),
        TypographyProfile::FhnwProposalParity => {
            RunFonts::new().ascii(FHNW_BODY).hi_ansi(FHNW_BODY)
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
    }
}

/// Caption run-fonts (Times New Roman for FHNW; Georgia for Designer).
fn caption_fonts_for(p: TypographyProfile) -> RunFonts {
    match p {
        TypographyProfile::Designer => body_fonts(),
        TypographyProfile::FhnwProposalParity => {
            RunFonts::new().ascii(FHNW_CAPTION).hi_ansi(FHNW_CAPTION)
        }
    }
}

/// Default body text colour ("000000" for both — both palettes ship black
/// running prose; the divergence is in the *accent* colours below).
fn body_color_for(_p: TypographyProfile) -> &'static str {
    "000000"
}

/// Primary heading colour. Designer = NAVY; FHNW = pure black.
fn heading_color_for(p: TypographyProfile) -> &'static str {
    match p {
        TypographyProfile::Designer => NAVY,
        TypographyProfile::FhnwProposalParity => FHNW_BLACK,
    }
}

/// Sub-heading (H3/H4) colour. Designer = HEAD2; FHNW = pure black.
fn subheading_color_for(p: TypographyProfile) -> &'static str {
    match p {
        TypographyProfile::Designer => HEAD2,
        TypographyProfile::FhnwProposalParity => FHNW_BLACK,
    }
}

/// Caption text colour. Designer = GREY; FHNW = pure black.
fn caption_color_for(p: TypographyProfile) -> &'static str {
    match p {
        TypographyProfile::Designer => GREY,
        TypographyProfile::FhnwProposalParity => FHNW_BLACK,
    }
}

/// "Accent" colour used on the title-page rule and small flourishes.
/// Designer = ACCENT (blue); FHNW = pure black (no accent).
fn accent_color_for(p: TypographyProfile) -> &'static str {
    match p {
        TypographyProfile::Designer => ACCENT,
        TypographyProfile::FhnwProposalParity => FHNW_BLACK,
    }
}

/// Secondary subtitle / imprint colour. Designer = GREY; FHNW = black.
fn subtitle_color_for(p: TypographyProfile) -> &'static str {
    match p {
        TypographyProfile::Designer => GREY,
        TypographyProfile::FhnwProposalParity => FHNW_BLACK,
    }
}

/// Heading size (half-points) for level N (1..=4) under the active profile.
/// Designer keeps the existing 44/32/26/23 ladder (= 22/16/13/11.5 pt);
/// FHNW uses 28/28/28/28 (= 14/14/14/14 pt — flat as in the proposal).
fn heading_size_hp(p: TypographyProfile, level: u8) -> usize {
    match (p, level) {
        (TypographyProfile::Designer, 1) => 44,
        (TypographyProfile::Designer, 2) => 32,
        (TypographyProfile::Designer, 3) => 26,
        (TypographyProfile::Designer, _) => 23,
        // FHNW proposal: H1-H4 all 14pt = 28 half-points. Bold for H1/H2/H4,
        // regular for H3 (matches proposal Word inspection 2026-05-28).
        (TypographyProfile::FhnwProposalParity, _) => 28,
    }
}

/// Body default size (half-points) under the active profile.
/// Designer: 22 (= 11 pt). FHNW: 20 (= 10 pt — proposal body).
fn body_size_hp(p: TypographyProfile) -> usize {
    match p {
        TypographyProfile::Designer => 22,
        TypographyProfile::FhnwProposalParity => 20,
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
    meta.thesis_typography == TypographyProfile::FhnwProposalParity
        && (meta.header_logo.as_ref().is_some_and(|b| !b.is_empty())
            || meta.header_lines.iter().any(|l| !l.trim().is_empty()))
}

/// Sidecar metadata `agentic book finalize` reads to inject the FHNW
/// header via Word COM. The CLI writes this file next to the rendered
/// docx as `<docx_basename>.fhnw_header.json` when
/// `fhnw_header_sidecar_needed(&meta)` is true.
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
    /// on 2026-05-29).
    pub logo_height_cm: f32,
    /// Whether the same header should also appear on subsequent pages
    /// (FHNW convention: yes). Word's default is per-section primary
    /// header; we don't need different-first-page.
    pub apply_to_all_pages: bool,
}

impl FhnwHeaderSidecar {
    /// Build the sidecar struct from a BookMeta, with the proposal's
    /// measured defaults for the cosmetic fields.
    pub fn from_meta(meta: &BookMeta, logo_path_abs: Option<String>) -> Self {
        Self {
            logo_path_abs,
            lines: meta.header_lines.clone(),
            line_font: "Arial".to_string(),
            line_size_pt: 12,
            line_bold: true,
            logo_height_cm: 4.92,
            apply_to_all_pages: true,
        }
    }
}

fn page_break() -> Paragraph {
    Paragraph::new().add_run(Run::new().add_break(BreakType::Page))
}

fn title_page(mut doc: Docx, m: &BookMeta) -> Docx {
    for _ in 0..3 {
        doc = doc.add_paragraph(Paragraph::new());
    }
    doc = doc.add_paragraph(
        Paragraph::new().align(AlignmentType::Center).add_run(
            Run::new()
                .add_text(&m.title)
                .bold()
                .size(72)
                .color(NAVY)
                .fonts(head_fonts()),
        ),
    );
    if !m.subtitle.is_empty() {
        doc = doc.add_paragraph(
            Paragraph::new().align(AlignmentType::Center).add_run(
                Run::new()
                    .add_text(&m.subtitle)
                    .size(30)
                    .color(GREY)
                    .fonts(head_fonts()),
            ),
        );
    }
    // Blue rule + descriptive line under the title (bookkit DESCRIPTION).
    if !m.description.is_empty() {
        doc = doc.add_paragraph(
            Paragraph::new()
                .align(AlignmentType::Center)
                .line_spacing(LineSpacing::new().before(160).after(120))
                .add_run(
                    Run::new()
                        .add_text("\u{2014}\u{2014}\u{2014}")
                        .color(ACCENT),
                ),
        );
        doc = doc.add_paragraph(
            Paragraph::new().align(AlignmentType::Center).add_run(
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
    doc = doc.add_paragraph(
        Paragraph::new().align(AlignmentType::Center).add_run(
            Run::new()
                .add_text(&m.context)
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
fn inscription_page(mut doc: Docx, m: &BookMeta) -> Docx {
    if m.dedication.is_none() && m.epigraph.is_none() {
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

fn heading_para(
    level: u8,
    text: &str,
    page_break_before: bool,
    typography: TypographyProfile,
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
        (TypographyProfile::FhnwProposalParity, 3)
    );
    let mut p = Paragraph::new()
        .style(&format!("Heading{}", level.min(4)))
        .line_spacing(
            LineSpacing::new()
                .before(SPACE_BEFORE_HEAD)
                .after(SPACE_AFTER_HEAD),
        );
    if page_break_before {
        p = p.add_run(Run::new().add_break(BreakType::Page));
    }
    let mut run = Run::new()
        .add_text(text)
        .size(size)
        .color(color)
        .fonts(head_fonts_for(typography));
    if bold {
        run = run.bold();
    }
    p.add_run(run)
}

fn run_of(r: &DocxRun, typography: TypographyProfile) -> Run {
    let mut run = Run::new()
        .add_text(&r.text)
        .size(body_size_hp(typography))
        .color(body_color_for(typography));
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

/// A true superscript bracketed reference-number run (bookkit `_superscript`),
/// pointing into the chapter Sources box. Uses `RunProperty::vert_align`
/// (`<w:vertAlign w:val="superscript"/>`); `Run` has no fluent setter, but its
/// `run_property` field is public.
fn superscript(n: usize) -> Run {
    let mut r = Run::new()
        .add_text(format!("[{n}]"))
        .size(15)
        .color(ACCENT)
        .fonts(body_fonts());
    r.run_property = r.run_property.vert_align(VertAlignType::SuperScript);
    r
}

/// Add a run sequence to a paragraph. Markdown links (`[label](url)`) render as
/// the label plus a superscript reference number and are registered in the
/// chapter's link registry (bookkit `add_inline` + `_register_link`); the URLs
/// then appear in the end-of-chapter Sources & QR-codes box.
fn add_runs(
    mut p: Paragraph,
    runs: &[DocxRun],
    links: &mut Vec<(String, String)>,
    typography: TypographyProfile,
) -> Paragraph {
    for r in runs {
        if let Some(url) = &r.link {
            // Register (de-dupe by URL) and emit label + superscript number.
            let n = match links.iter().position(|(_, u)| u == url) {
                Some(i) => i + 1,
                None => {
                    links.push((r.text.clone(), url.clone()));
                    links.len()
                }
            };
            let mut label = Run::new()
                .add_text(&r.text)
                .size(body_size_hp(typography))
                .color(accent_color_for(typography))
                .fonts(body_fonts_for(typography));
            if r.bold {
                label = label.bold();
            }
            p = p.add_run(label).add_run(superscript(n));
        } else {
            p = p.add_run(run_of(r, typography));
        }
    }
    p
}

/// Default body-paragraph alignment for the active typography profile.
///
/// ADR-0050 §1 item 3: the FHNW proposal direct-formats prose as JUSTIFY
/// even though its `Normal` style is LEFT. We mirror that in the
/// `FhnwProposalParity` profile by giving body paragraphs an explicit
/// Justify alignment; the Designer profile keeps the engine's historical
/// LEFT-via-style behaviour.
fn body_alignment_for(t: TypographyProfile) -> AlignmentType {
    match t {
        TypographyProfile::Designer => AlignmentType::Left,
        // docx-rs maps WordprocessingML `w:jc w:val="both"` (the canonical
        // OOXML "justify both edges" value, internally also called "Justified")
        // to `AlignmentType::Both`. Word renders both identically; we pick
        // `Both` because it is the one OOXML actually serialises and matches
        // the value found in the proposal docx.
        TypographyProfile::FhnwProposalParity => AlignmentType::Both,
    }
}

fn para_of(
    runs: &[DocxRun],
    links: &mut Vec<(String, String)>,
    typography: TypographyProfile,
) -> Paragraph {
    add_runs(
        Paragraph::new()
            .line_spacing(body_spacing())
            .align(body_alignment_for(typography)),
        runs,
        links,
        typography,
    )
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
                let mut cell = TableCell::new()
                    .shading(Shading::new().fill(HEADBG))
                    .width(col_widths[ci.min(col_widths.len() - 1)], WidthType::Dxa)
                    .vertical_align(VAlignType::Center);
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
            cells.push(
                TableCell::new()
                    .shading(Shading::new().fill(fill))
                    .width(cw, WidthType::Dxa)
                    .vertical_align(VAlignType::Center)
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
    Table::new(trows)
        .set_grid(col_widths.clone())
        .width(content_twips, WidthType::Dxa)
        // Fixed layout makes Word honour the grid and wrap text, so a wide table
        // can never expand past the page margins (ADR-0030).
        .layout(TableLayoutType::Fixed)
        // Cell padding so text never touches the borders.
        .margins(TableCellMargins::new().margin(60, 100, 60, 100))
}

/// chapter_extras.py "Key topics at a glance" box: a shaded single-column table
/// (navy header + zebra key-point rows), with breathing room around it.
fn keypoints_box(mut doc: Docx, body: &str) -> Docx {
    let spacer = || Paragraph::new().line_spacing(LineSpacing::new().after(SPACE_AROUND_TABLE));
    let mut rows = vec![TableRow::new(vec![
        TableCell::new()
            .shading(Shading::new().fill(HEADBG))
            .width(CONTENT_TWIPS, WidthType::Dxa)
            .add_paragraph(
                Paragraph::new().add_run(
                    Run::new()
                        .add_text("Key topics at a glance")
                        .bold()
                        .size(21)
                        .color("FFFFFF")
                        .fonts(head_fonts()),
                ),
            ),
    ])];
    for (i, line) in body
        .lines()
        .map(|l| l.trim().trim_start_matches(['-', '•', '*', ' ']).trim())
        .filter(|l| !l.is_empty())
        .enumerate()
    {
        let fill = if i % 2 == 0 { ALTBG } else { "FFFFFF" };
        rows.push(TableRow::new(vec![
            TableCell::new()
                .shading(Shading::new().fill(fill))
                .width(CONTENT_TWIPS, WidthType::Dxa)
                .add_paragraph(
                    Paragraph::new()
                        .line_spacing(LineSpacing::new().after(40))
                        .add_run(
                            Run::new()
                                .add_text("•  ")
                                .bold()
                                .size(21)
                                .color(NAVY)
                                .fonts(body_fonts()),
                        )
                        .add_run(Run::new().add_text(line).size(21).fonts(body_fonts())),
                ),
        ]));
    }
    doc = doc.add_paragraph(spacer());
    doc = doc.add_table(
        Table::new(rows)
            .set_grid(vec![CONTENT_TWIPS])
            .width(CONTENT_TWIPS, WidthType::Dxa)
            .layout(TableLayoutType::Fixed)
            .margins(TableCellMargins::new().margin(70, 120, 70, 120)),
    );
    doc.add_paragraph(spacer())
}

/// bookkit.py admonition: a colour-coded shaded box with a left accent border
/// and a bold label, for note / tip / warning asides. Rendered as a single-cell
/// table so the fill + left border survive in Word.
fn admonition_box(mut doc: Docx, kind: &str, body: &str, figdir: &Path, lang: &str) -> Docx {
    // Label is localised chrome; the SEQ-free admonition has no field name to
    // keep stable, so the visible word is translated directly.
    let (word, glyph, fill, edge) = match kind {
        "tip" => (t(lang, "tip"), "\u{2714}", "EAF6EC", "2E7D32"),
        "warning" => (t(lang, "warning"), "\u{26A0}", "FBF1E2", "C77F18"),
        _ => (t(lang, "note"), "\u{2139}", "EAF1FB", "1F3864"),
    };
    let text: String = body
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    // gen_icons PNG (icon_{kind}.png) if the book command rendered it into
    // figdir; otherwise fall back to a unicode glyph.
    let icon = std::fs::read(figdir.join(format!("icon_{kind}.png"))).ok();
    let mut label_para = Paragraph::new().line_spacing(body_spacing());
    if let Some(bytes) = &icon {
        let pic = Pic::new(bytes).size(150_000, 150_000); // ≈0.4 cm square
        label_para = label_para.add_run(Run::new().add_image(pic)).add_run(
            Run::new()
                .add_text(format!(" {word}  "))
                .bold()
                .size(21)
                .color(edge)
                .fonts(head_fonts()),
        );
    } else {
        label_para = label_para.add_run(
            Run::new()
                .add_text(format!("{glyph} {word}  "))
                .bold()
                .size(21)
                .color(edge)
                .fonts(head_fonts()),
        );
    }
    label_para = label_para.add_run(Run::new().add_text(text).size(22).fonts(body_fonts()));
    let spacer = || Paragraph::new().line_spacing(LineSpacing::new().after(SPACE_AROUND_TABLE));
    let cell = TableCell::new()
        .shading(Shading::new().fill(fill))
        .width(CONTENT_TWIPS, WidthType::Dxa)
        .set_border(
            TableCellBorder::new(TableCellBorderPosition::Left)
                .color(edge)
                .size(24)
                .border_type(BorderType::Single),
        )
        .add_paragraph(label_para);
    doc = doc.add_paragraph(spacer());
    doc = doc.add_table(
        Table::new(vec![TableRow::new(vec![cell])])
            .set_grid(vec![CONTENT_TWIPS])
            .width(CONTENT_TWIPS, WidthType::Dxa)
            .layout(TableLayoutType::Fixed)
            .margins(TableCellMargins::new().margin(70, 120, 70, 120)),
    );
    doc.add_paragraph(spacer())
}

/// bookkit.py generic callout: a navy left-bordered shaded box; the first line
/// (if it ends with a colon) is a bold navy title, the rest is the body.
fn callout_box(mut doc: Docx, body: &str) -> Docx {
    let lines: Vec<&str> = body
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let (title, rest) = match lines.split_first() {
        Some((first, tail)) if first.ends_with(':') => (Some(*first), tail.join(" ")),
        _ => (None, lines.join(" ")),
    };
    let spacer = || Paragraph::new().line_spacing(LineSpacing::new().after(SPACE_AROUND_TABLE));
    let mut cell = TableCell::new()
        .shading(Shading::new().fill("EEF2F8"))
        .width(CONTENT_TWIPS, WidthType::Dxa)
        .set_border(
            TableCellBorder::new(TableCellBorderPosition::Left)
                .color(NAVY)
                .size(24)
                .border_type(BorderType::Single),
        );
    if let Some(t) = title {
        cell = cell.add_paragraph(
            Paragraph::new().add_run(
                Run::new()
                    .add_text(t.trim_end_matches(':'))
                    .bold()
                    .size(21)
                    .color(NAVY)
                    .fonts(head_fonts()),
            ),
        );
    }
    cell = cell.add_paragraph(
        Paragraph::new()
            .line_spacing(body_spacing())
            .add_run(Run::new().add_text(rest).size(22).fonts(body_fonts())),
    );
    doc = doc.add_paragraph(spacer());
    doc = doc.add_table(
        Table::new(vec![TableRow::new(vec![cell])])
            .set_grid(vec![CONTENT_TWIPS])
            .width(CONTENT_TWIPS, WidthType::Dxa)
            .layout(TableLayoutType::Fixed)
            .margins(TableCellMargins::new().margin(70, 120, 70, 120)),
    );
    doc.add_paragraph(spacer())
}

/// chapter_extras.py per-chapter "Review questions": `Q:`/`A:` line pairs become
/// a numbered bold question + a grey italic answer.
fn quiz_block(mut doc: Docx, body: &str) -> Docx {
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
            doc = doc.add_paragraph(
                Paragraph::new()
                    .line_spacing(LineSpacing::new().before(80).after(30))
                    .add_run(
                        Run::new()
                            .add_text(format!("{qn}. {q}"))
                            .bold()
                            .size(22)
                            .color("1A1A1A")
                            .fonts(body_fonts()),
                    ),
            );
            doc = doc.add_paragraph(
                Paragraph::new().line_spacing(body_spacing()).add_run(
                    Run::new()
                        .add_text(a.trim())
                        .italic()
                        .size(21)
                        .color(GREY)
                        .fonts(body_fonts()),
                ),
            );
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
    doc = doc.add_table(
        Table::new(vec![TableRow::new(vec![cell])])
            .set_grid(vec![CONTENT_TWIPS])
            .width(CONTENT_TWIPS, WidthType::Dxa)
            .layout(TableLayoutType::Fixed)
            .margins(TableCellMargins::new().margin(70, 200, 70, 120)),
    );
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
fn postprocess_docx(docx: Vec<u8>) -> anyhow::Result<Vec<u8>> {
    use std::io::{Read, Write};
    let mut zin = zip::ZipArchive::new(Cursor::new(docx)).context("open docx zip")?;
    let mut out = Cursor::new(Vec::<u8>::new());
    {
        let mut zout = zip::ZipWriter::new(&mut out);
        for i in 0..zin.len() {
            let mut f = zin.by_index(i).context("read zip entry")?;
            let name = f.name().to_string();
            if name == "word/settings.xml" || name == "word/document.xml" {
                let mut s = String::new();
                f.read_to_string(&mut s).context("read xml part")?;
                let s = match name.as_str() {
                    "word/settings.xml" => inject_update_fields(s),
                    _ => mark_header_rows(&s),
                };
                zout.start_file(name, zip::write::SimpleFileOptions::default())
                    .context("start xml part")?;
                zout.write_all(s.as_bytes()).context("write xml part")?;
            } else {
                zout.raw_copy_file(f).context("copy zip entry")?;
            }
        }
        zout.finish().context("finish docx zip")?;
    }
    Ok(out.into_inner())
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
    if matches!(typography, TypographyProfile::FhnwProposalParity) {
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
/// that Word fills from the caption SEQ fields.
fn list_of(seq: &str, heading: &str, typography: TypographyProfile) -> [Paragraph; 2] {
    [
        Paragraph::new()
            .style("Heading1")
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
    const QR_COL: usize = 1700; // ≈3.0 cm
    let text_col = CONTENT_TWIPS - QR_COL;
    let mut rows = Vec::new();
    for (i, (label, url)) in links.iter().enumerate() {
        let n = i + 1;
        let left = TableCell::new()
            .width(text_col, WidthType::Dxa)
            .vertical_align(VAlignType::Center)
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
            Some(png) => Paragraph::new()
                .align(AlignmentType::Center)
                .add_run(Run::new().add_image(Pic::new(&png).size(900_000, 900_000))),
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
    doc = doc.add_table(
        Table::new(rows)
            .set_grid(vec![text_col, QR_COL])
            .width(CONTENT_TWIPS, WidthType::Dxa)
            .layout(TableLayoutType::Fixed)
            .margins(TableCellMargins::new().margin(60, 100, 60, 100)),
    );
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
            let shown = if *level == 1 && chapter_start && numbered {
                ctx.chapno += 1;
                format!("{}  {text}", ctx.chapno)
            } else {
                text.clone()
            };
            doc.add_paragraph(heading_para(
                *level,
                &shown,
                chapter_start && *level <= 2,
                ctx.typography,
            ))
        }
        DocxBlock::Paragraph(runs) => {
            let mut p = para_of(runs, &mut ctx.links, ctx.typography);
            let text: String = runs.iter().map(|r| r.text.as_str()).collect();
            for xe in index_marks(&text, &ctx.index_terms, &mut ctx.idx_seen, ctx.typography) {
                p = p.add_run(xe);
            }
            doc.add_paragraph(p)
        }
        DocxBlock::BulletItem(runs) => {
            let mut p = Paragraph::new()
                .line_spacing(body_spacing())
                .align(body_alignment_for(ctx.typography))
                .add_run(
                    Run::new()
                        .add_text("•  ")
                        .size(body_size_hp(ctx.typography))
                        .color(heading_color_for(ctx.typography))
                        .bold()
                        .fonts(body_fonts_for(ctx.typography)),
                );
            p = add_runs(p, runs, &mut ctx.links, ctx.typography);
            doc.add_paragraph(p)
        }
        DocxBlock::OrderedItem { number, runs } => {
            let mut p = Paragraph::new()
                .line_spacing(body_spacing())
                .align(body_alignment_for(ctx.typography))
                .add_run(
                    Run::new()
                        .add_text(format!("{number}.  "))
                        .size(body_size_hp(ctx.typography))
                        .color(heading_color_for(ctx.typography))
                        .bold()
                        .fonts(body_fonts_for(ctx.typography)),
                );
            p = add_runs(p, runs, &mut ctx.links, ctx.typography);
            doc.add_paragraph(p)
        }
        DocxBlock::CodeBlock { lang, body } => match lang.as_str() {
            // chapter_extras.py port: the "Key topics at a glance" box.
            "keypoints" => keypoints_box(doc, body),
            // chapter_extras.py port: the per-chapter "Review questions".
            "quiz" => quiz_block(doc, body),
            // bookkit.py port: note / tip / warning admonition callouts.
            "note" | "tip" | "warning" => admonition_box(doc, lang, body, ctx.figdir, ctx.lang),
            // bookkit.py port: a generic titled key-point callout box.
            "callout" => callout_box(doc, body),
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
            let italic_caption = !matches!(typography, TypographyProfile::FhnwProposalParity);
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
            doc = doc.add_paragraph(
                Paragraph::new()
                    .style("Caption") // ADR-0050 §1 item 8: native Word Caption style
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
                doc = doc.add_paragraph(Paragraph::new().section_property(portrait_sectpr()));
                doc = doc.add_table(table_block(
                    header,
                    rows,
                    LAND_CONTENT_TWIPS,
                    ctx.typography,
                ));
                doc.add_paragraph(Paragraph::new().section_property(landscape_sectpr()))
            } else {
                // Breathing room around the table (ADR-0030 relaxed placement).
                let spacer =
                    || Paragraph::new().line_spacing(LineSpacing::new().after(SPACE_AROUND_TABLE));
                // keep_next chains caption -> spacer -> table so the title never
                // strands at a page foot (the trailing spacer must NOT keep_next).
                doc = doc.add_paragraph(spacer().keep_next(true));
                doc = doc.add_table(table_block(header, rows, CONTENT_TWIPS, ctx.typography));
                doc.add_paragraph(spacer())
            }
        }
        DocxBlock::Image { path, caption } => {
            let full = ctx
                .figdir
                .join(path.replace('/', std::path::MAIN_SEPARATOR_STR));
            if let Ok(bytes) = std::fs::read(&full) {
                ctx.figno += 1;
                let target_w: u32 = 5_400_000; // 15 cm in EMU
                let h_emu = png_dims(&bytes)
                    .map(|(w, h)| {
                        ((u64::from(h) * u64::from(target_w)) / u64::from(w.max(1))) as u32
                    })
                    .unwrap_or(3_400_000);
                let pic = Pic::new(&bytes).size(target_w, h_emu);
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
                let italic_caption = !matches!(typography, TypographyProfile::FhnwProposalParity);
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
                doc.add_paragraph(
                    Paragraph::new()
                        .style("Caption") // ADR-0050 §1 item 8: native Word Caption style
                        .align(AlignmentType::Center)
                        .line_spacing(LineSpacing::new().after(SPACE_AROUND_FIG))
                        .add_run(cap_style(t(ctx.lang, "fig_prefix")))
                        .add_run(field_run(
                            "SEQ Figure \\* ARABIC",
                            &format!("{}", ctx.figno),
                            false,
                        ))
                        .add_run(cap_style(&format!("{sep} {caption}"))),
                )
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
/// field populates) + the caption style. docx-rs does not ship Heading styles,
/// so referencing them without defining them yields an empty TOC.
fn with_styles(mut doc: Docx) -> Docx {
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
    doc
}

/// Fold a `Table:`-prefixed paragraph into the caption of the table that
/// immediately follows it (bookkit caption-above convention for markdown).
fn fold_table_captions(blocks: Vec<DocxBlock>) -> Vec<DocxBlock> {
    let mut out: Vec<DocxBlock> = Vec::with_capacity(blocks.len());
    let mut pending: Option<String> = None;
    for b in blocks {
        match b {
            DocxBlock::Paragraph(ref runs) => {
                let text: String = runs.iter().map(|r| r.text.as_str()).collect();
                let t = text.trim();
                if let Some(rest) = t
                    .strip_prefix("Table:")
                    .or_else(|| t.strip_prefix("table:"))
                {
                    pending = Some(rest.trim().to_string());
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
                let cap = pending.take().or(caption);
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
    let mut doc = with_styles(
        Docx::new()
            .default_fonts(body_fonts())
            .default_size(22)
            .page_size(11906, 16838)
            .page_margin(std_margin()),
    )
    .footer(
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
        doc = inscription_page(doc, meta);
    }
    doc = doc.add_paragraph(
        Paragraph::new().add_run(
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
    };

    for (ci, (_label, md)) in chapters.iter().enumerate() {
        let blocks = fold_table_captions(to_docx_blocks(md));
        let numbered = chapter_is_numbered(md, meta.thesis_profile);
        let mut first = true;
        for b in &blocks {
            doc = render_block(doc, b, &mut ctx, first && ci > 0, numbered);
            first = false;
        }
        // End-of-chapter Sources & QR-codes box (bookkit flush_sources).
        doc = flush_sources(doc, &mut ctx.links, &meta.lang, ctx.typography);
    }

    // Appendix lists (filled from the caption SEQ fields on field update).
    doc = doc.add_paragraph(page_break());
    // `seq` (SEQ field name) stays English/stable for numbering; only the
    // visible heading is localised.
    for p in list_of(
        "Figure",
        t(&meta.lang, "list_of_figures"),
        meta.thesis_typography,
    ) {
        doc = doc.add_paragraph(p);
    }
    for p in list_of(
        "Table",
        t(&meta.lang, "list_of_tables"),
        meta.thesis_typography,
    ) {
        doc = doc.add_paragraph(p);
    }

    // Back-of-book index: the INDEX field, filled from XE entries on update.
    doc = doc.add_paragraph(page_break());
    doc = doc.add_paragraph(
        Paragraph::new().style("Heading1").add_run(
            Run::new()
                .add_text("Index")
                .bold()
                .size(32)
                .color(NAVY)
                .fonts(head_fonts()),
        ),
    );
    doc = doc.add_paragraph(
        Paragraph::new().add_run(
            Run::new()
                .add_text("Right-click and choose \u{201c}Update Field\u{201d} to build the index.")
                .italic()
                .size(18)
                .color(GREY)
                .fonts(body_fonts()),
        ),
    );
    doc = doc.add_paragraph(Paragraph::new().add_run(field_run("INDEX \\c 2", "", true)));

    let mut cur = Cursor::new(Vec::<u8>::new());
    doc.build().pack(&mut cur).context("pack book docx")?;
    postprocess_docx(cur.into_inner())
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
    flush_sources(doc, &mut ctx.links, &meta.lang, ctx.typography)
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
    let mut doc = with_styles(
        Docx::new()
            .default_fonts(body_fonts())
            .default_size(22)
            .page_size(11906, 16838)
            .page_margin(std_margin()),
    )
    .footer(
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
        TypographyProfile::FhnwProposalParity
    );
    let front_matter_slots = [
        ThesisSlot::DeclarationOriginality,
        ThesisSlot::ComplianceDeclaration,
        ThesisSlot::Declaration,
        ThesisSlot::MgmtSummary,
        ThesisSlot::Acronyms,
    ];
    for item in thesis_layout(chapters) {
        match item {
            ThesisItem::Chapter(i) => {
                let slot = thesis_slot(&chapters[i].1);
                let md_ref: String = if fhnw && slot == ThesisSlot::TitlePage {
                    strip_first_h1_line(&chapters[i].1)
                } else {
                    chapters[i].1.clone()
                };
                // D6: force a page break before each front-matter chapter
                // (under FHNW only). Non-thesis books and the Designer
                // profile keep the historical "chapter_break_before from
                // emitted-state" behaviour.
                if fhnw && front_matter_slots.contains(&slot) && emitted {
                    doc = doc.add_paragraph(page_break());
                }
                doc = render_thesis_chapter(doc, &md_ref, meta, &mut ctx, emitted);
                emitted = true;
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
                for p in list_of("Figure", t(&meta.lang, "list_of_figures"), ctx.typography) {
                    doc = doc.add_paragraph(p);
                }
            }
            ThesisItem::ListTables => {
                for p in list_of("Table", t(&meta.lang, "list_of_tables"), ctx.typography) {
                    doc = doc.add_paragraph(p);
                }
            }
            ThesisItem::Index => {
                // Back-of-book Index: skipped under FHNW typography (the
                // proposal docx has no Index section; emitting an empty
                // INDEX field would just add a blank "Index" page at the
                // end of the thesis). Designer profile keeps the standard
                // book Index.
                if matches!(ctx.typography, TypographyProfile::FhnwProposalParity) {
                    continue;
                }
                // Back-of-book Index: the INDEX field, filled from XE entries
                // on field update. Heading is "Index" so the thesis profile
                // closes with the same standard structural element as a book.
                doc = doc.add_paragraph(page_break());
                doc = doc.add_paragraph(
                    Paragraph::new().style("Heading1").add_run(
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
    postprocess_docx(cur.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // A table with no surrounding headings: the only keepNext comes from the
        // caption + pre-table spacer (gap #3), so we expect at least two.
        let md = "Intro text.\n\n| A | B |\n|---|---|\n| 1 | 2 |\n".to_string();
        let bytes = render_book(&meta, &[("c1".into(), md)], Path::new(".")).unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut d = String::new();
        zip.by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut d)
            .unwrap();
        assert!(
            d.matches("keepNext").count() >= 2,
            "table caption + pre-table spacer must keep_next so the title stays with the table"
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
        // ADR-0050 §1 item 3 (v0.1.14): body paragraphs under the FHNW
        // profile carry w:jc w:val="both" (= AlignmentType::Both, OOXML
        // "justify"). Designer profile body paragraphs do NOT carry a w:jc
        // and inherit Normal/LEFT.
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

        // Designer baseline: no w:jc=both anywhere (regression guard).
        let meta_designer = BookMeta {
            title: "T".into(),
            ..Default::default()
        };
        let bytes2 = render_book(
            &meta_designer,
            &[(
                "c1".into(),
                "# C\n\nA body paragraph that should NOT justify.\n".into(),
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
            !d2.contains("w:val=\"both\""),
            "Designer profile must NOT emit w:jc=both (regression guard)"
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
}
