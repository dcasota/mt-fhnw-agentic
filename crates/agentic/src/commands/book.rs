//! `agentic book` — render professional DOCX books (Rust book engine) and audit
//! render quality against the previous iteration. Replaces the Python
//! bookkit/build_book. Figures via `agentic-figures`, layout via
//! `agentic-export::book`.

use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

use agentic_core::worktree;
use agentic_export::book::{BookMeta, render_book};

use crate::cli::BookAction;

#[derive(Debug, Deserialize)]
struct Manifest {
    books: Vec<BookSpec>,
}
#[derive(Debug, Deserialize)]
struct BookSpec {
    key: String,
    title: String,
    #[serde(default)]
    subtitle: String,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    context: Option<String>,
    #[serde(default)]
    description: String,
    #[serde(default)]
    dedication: Option<String>,
    #[serde(default)]
    epigraph: Option<String>,
    #[serde(default)]
    epigraph_by: Option<String>,
    #[serde(default)]
    disclaimer: Option<String>,
    /// Imprint lines on the title page (version, place + date); one per line.
    #[serde(default)]
    imprint: Option<String>,
    /// Master-thesis numbering profile (ADR-0045): number body chapters,
    /// keep only true front/back-matter unnumbered.
    #[serde(default)]
    thesis_profile: bool,
    /// Companion-paper profile (ADR-0045, bookkit B): skip the book chrome
    /// (title page / disclaimer / inscription); plain title + contents only.
    #[serde(default)]
    companion: bool,
    #[serde(default)]
    index_terms: Vec<String>,
    /// Optional path (in the project worktree) to a TOML allowlist of
    /// auto-harvest index terms (Wave 9, AI-Norms parity, 2026-06-03).
    /// When set, the file is read from the project DB and parsed via
    /// [`agentic_export::index::IndexAllowlist::parse_toml`]; every term
    /// is appended to `index_terms` before the engine harvests the
    /// chapter prose. Lets the 113-entry `specs/books/ai_norms/
    /// index_allowlist.toml` file drive the index without inlining a
    /// 113-string array into the manifest JSON. `None` → no change.
    #[serde(default)]
    index_allowlist_path: Option<String>,
    /// Optional per-book chrome language (en|de|fr|it|rm|hi). Overrides the
    /// global `--lang`; absent → fall back to `--lang` then "en".
    #[serde(default)]
    lang: Option<String>,
    /// Optional content-image asset prefix in the working tree (e.g.
    /// `out/sources/norms/media`). When set, every blob under it is materialised
    /// (by basename) into the per-book scratch dir so `![caption](name.png)`
    /// image references resolve. Opt-in: books without it are unaffected.
    #[serde(default)]
    assets: Option<String>,
    /// Typography profile (ADR-0050). "designer" (default) keeps the
    /// Georgia/Calibri/navy aesthetic for every non-thesis book.
    /// "fhnw-proposal-parity" switches body/headings/captions to
    /// Arial/Arial/Times-New-Roman black for FHNW master-thesis parity.
    #[serde(default)]
    thesis_typography: Option<String>,
    /// Caption format (ADR-0050 §1). "period" (default) → "Figure 1.";
    /// "colon" → "Figure 1:" (English) / "Abbildung 1:" (German).
    #[serde(default)]
    caption_format: Option<String>,
    /// Optional DB path of a PNG logo to render in the FHNW running
    /// header (ADR-0050 §1 item 1). When set with `thesis_typography =
    /// "fhnw-proposal-parity"`, the engine renders the logo + the
    /// `header_lines` on every page header.
    #[serde(default)]
    header_logo: Option<String>,
    /// Optional header text lines (rendered right-aligned beneath the
    /// logo). e.g. `["Master of Advanced Studies", "Leadership in
    /// Cybersecurity"]`.
    #[serde(default)]
    header_lines: Vec<String>,
    /// T1.7 — Optional STANDALONE dedication page text (REF parity
    /// 2026-06-02). Distinct from `dedication`, which sits on the
    /// inscription page next to the epigraph: when this field is set, the
    /// engine emits a dedicated page BEFORE the inscription page with the
    /// text centred, italic, large. Leave unset (the historical default) to
    /// keep the single-inscription layout. Multi-line strings are honoured;
    /// each line becomes its own centred paragraph.
    #[serde(default)]
    dedication_page: Option<String>,
    /// T1.7 — Optional Antikythera NOTE for the inscription page footer
    /// (REF parity 2026-06-02). When set, the engine appends a small grey
    /// NOTE-style paragraph at the bottom of the inscription page (after
    /// the epigraph attribution). Used in the reference book to attribute
    /// the Antikythera-mechanism artwork. Plain text; rendered centred.
    #[serde(default)]
    antikythera_note: Option<String>,
    /// T1.7 — Optional standalone QR-linked URL block (REF parity
    /// 2026-06-02). Distinct from the per-chapter Sources & QR-codes box
    /// (which lists every link in a chapter): when this field is set, the
    /// engine emits a single centred QR + URL block as a standalone page
    /// right after the disclaimer/inscription chrome. Used to advertise
    /// the book's companion URL.
    #[serde(default)]
    qrlink: Option<String>,
    /// Wave-2 AI-Norms parity (ADR-0054 v1, 2026-06-03). When `true` the engine
    /// emits the bookkit `BkH1..4` heading family + `TableGrid` table style
    /// on body content, and replaces `word/styles.xml` in the finalize-pass
    /// with the verbatim reference styles document (186 style definitions).
    /// Defaults to `false` so non-parity books are unaffected.
    #[serde(default)]
    body_render_use_bk_styles: bool,
    // ---- Wave-4 AI-Norms parity (ADR-0054 v1, 2026-06-03) layout / chrome
    //      overrides (all optional). The engine treats `None`/`false`/empty
    //      as "no override", so books that don't declare these stay
    //      identical to the historical Designer output.
    #[serde(default)]
    header_distance_twips: Option<u32>,
    #[serde(default)]
    footer_distance_twips: Option<u32>,
    #[serde(default)]
    cols_space_twips: Option<u32>,
    #[serde(default)]
    doc_grid_line_pitch: Option<u32>,
    #[serde(default)]
    dedication_personal: Option<String>,
    #[serde(default)]
    closing_thought: Option<String>,
    #[serde(default)]
    byline_institution_full: Option<String>,
    #[serde(default)]
    tof_heading: Option<String>,
    #[serde(default)]
    tot_heading: Option<String>,
    #[serde(default)]
    inscription_page_enabled: bool,
    /// Wave-2 (Bookkit profile chrome suppression, REF parity 2026-06-04).
    /// Four boolean knobs the engine reads to suppress book chrome on a
    /// per-profile basis. Each defaults to `true` so historical books
    /// are unaffected; `master_thesis_bookkit` opts three of them out
    /// (`emit_index`, `emit_appendix_in_back_matter`,
    /// `emit_per_chapter_sources_box`) to match the reference thesis.
    /// `emit_chapter_dividers` stays `true` because the reference book
    /// carries ≥40 horizontal rules. See `BookMeta` for engine semantics.
    #[serde(default = "default_true")]
    emit_index: bool,
    #[serde(default = "default_true")]
    emit_appendix_in_back_matter: bool,
    #[serde(default = "default_true")]
    emit_per_chapter_sources_box: bool,
    #[serde(default = "default_true")]
    emit_chapter_dividers: bool,
    /// Wave-3 iter-D (REF parity 2026-06-04). When `true`, the bookkit
    /// thesis renderer emits a `<w:sectPr>` paragraph at every chapter
    /// close so the document carries one section break per chapter
    /// (reference master thesis: 19 in-body sectPrs + 1 doc-level = 20
    /// total). Defaults to `false` so non-bookkit profiles keep the
    /// historical single document-level sectPr layout.
    #[serde(default)]
    emit_per_chapter_sectpr: bool,
    /// Wave-3 iter-D (REF parity 2026-06-04). When `true` (historical
    /// default) the engine renders ```keypoints```, ```quiz``` and
    /// ```callout``` fenced blocks as the bookkit `chapter_extras`
    /// chrome. The `master_thesis_bookkit` manifest sets this to `false`
    /// so the FHNW reference-thesis output has no per-chapter
    /// key-topic / review-question / callout chrome.
    #[serde(default = "default_true")]
    emit_chapter_extras: bool,
    chapters: Vec<String>,
}

/// Serde helper for the Wave-2 chrome-suppression flags whose historical
/// default is `true` (engine emits the chrome) rather than `false`.
fn default_true() -> bool {
    true
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

/// Wave-9 polish (AI-Norms parity, 2026-06-03): drop the `## Figures` section
/// the Wave-5 ai_norms-figures-wave5 dispatch wrote into every
/// `out/sources/ai_norms/<chapter>.extras.md`. The sidecar holds 1–N
/// `image-embed` figspecs that point at the same raster the main chapter
/// markdown references via `![caption](name.png)`; without stripping the
/// section we end up rendering each figure twice — once from the chapter
/// `![]()` reference, once from the sidecar figspec — and the figure-count
/// parity gate is over by exactly the sidecar size.
///
/// The marker `<!-- ai_norms-figures-wave5 -->` is the insertion sentinel
/// the Wave-5 dispatcher wrote. The section to strip ends either at
/// end-of-file OR at the `<!-- wave3-table-figspec begin -->` sentinel
/// (Round-D, 2026-06-03) — the wave-3 dispatch authored table figspecs
/// AFTER the wave-5 marker in 9 of 10 chapters, so a naive cut-to-EOF
/// strips the very tables we need to preserve. We therefore slice
/// `[wave5 .. wave3-begin)` when the wave3 marker is present, splice the
/// preserved tail back on, and fall back to cut-to-EOF when there is no
/// wave3 block. Returns the input unchanged when the wave5 marker is
/// absent (every non-extras chapter, every non-AI-Norms book).
#[must_use]
pub fn strip_wave5_figures_section(md: &str) -> String {
    const WAVE5_MARKER: &str = "<!-- ai_norms-figures-wave5 -->";
    const WAVE3_BEGIN: &str = "<!-- wave3-table-figspec begin -->";
    let Some(wave5_at) = md.find(WAVE5_MARKER) else {
        return md.to_string();
    };
    let head = md[..wave5_at].trim_end();
    // If a wave3 table-figspec block sits after the wave5 marker, preserve
    // it (and everything after it). Without this carve-out we drop 20-of-22
    // figspec tables and the CAPTIONED_TABLE_PARITY gate fails.
    if let Some(rel) = md[wave5_at..].find(WAVE3_BEGIN) {
        let wave3_at = wave5_at + rel;
        let tail = &md[wave3_at..];
        if head.is_empty() {
            return tail.to_string();
        }
        return format!("{head}\n\n{tail}");
    }
    head.to_string()
}

/// Round-F (AI-Norms parity, 2026-06-03): drop ```keypoints``` and ```quiz```
/// fenced blocks from a chapter body. Used when the same chapter also has
/// an extras sidecar (`out/sources/ai_norms/<slug>.extras.md`) that owns
/// the authoritative `chapter_extras` chrome. Without this, every covered
/// chapter renders **two** keypoints boxes + two quiz blocks (32 chapters →
/// 64 "Key topics at a glance" titles vs the reference book's 32; 64
/// "Review questions" headings vs the reference's 32).
///
/// The strip is line-oriented and only removes a leading-of-line fence
/// (``` plus either `keypoints` or `quiz`); inline back-tick markup inside
/// prose is unaffected. A best-effort blank-line cleanup keeps the output
/// readable. Returns the input unchanged when neither fence appears.
#[must_use]
pub fn strip_keypoints_and_quiz_fences(md: &str) -> String {
    if !md.contains("```keypoints") && !md.contains("```quiz") {
        return md.to_string();
    }
    let mut out: Vec<&str> = Vec::with_capacity(md.lines().count());
    let mut skip_until_close = false;
    for line in md.lines() {
        let trimmed = line.trim_start();
        if skip_until_close {
            // Closing fence is the first ``` we encounter at line start.
            if trimmed.starts_with("```") {
                skip_until_close = false;
            }
            continue;
        }
        if trimmed.starts_with("```keypoints") || trimmed.starts_with("```quiz") {
            skip_until_close = true;
            continue;
        }
        out.push(line);
    }
    // Collapse 3+ consecutive blank lines (introduced by the strip) down to 2.
    let mut collapsed: Vec<&str> = Vec::with_capacity(out.len());
    let mut blank_run = 0usize;
    for line in &out {
        if line.trim().is_empty() {
            blank_run += 1;
            if blank_run <= 2 {
                collapsed.push(line);
            }
        } else {
            blank_run = 0;
            collapsed.push(line);
        }
    }
    collapsed.join("\n")
}

/// Round-F (AI-Norms parity, 2026-06-03): extract the chapter slug from a
/// main-chapter md path so it can be matched against an extras-sidecar
/// path. Returns `None` for non-matching paths.
///
/// Inputs honoured:
///   - `out/sources/norms/10_usa_EN.md`        → `Some("usa")`
///   - `out/sources/norms/30_humanoid_usecase_EN.md` → `Some("humanoid_usecase")`
///   - `out/sources/norms/05_introduction_EN.md` → `Some("introduction")`
///   - `out/sources/ai_norms/usa.extras.md`     → `None` (sidecar, not main)
///   - anything else                            → `None`
fn norms_chapter_slug(path: &str) -> Option<String> {
    let stem = path.strip_prefix("out/sources/norms/")?;
    // strip ".md" suffix
    let stem = stem.strip_suffix(".md")?;
    // strip trailing language suffix "_EN", "_DE", … (3-letter+underscore)
    let stem = stem
        .strip_suffix("_EN")
        .or_else(|| stem.strip_suffix("_DE"))
        .or_else(|| stem.strip_suffix("_FR"))
        .or_else(|| stem.strip_suffix("_IT"))
        .or_else(|| stem.strip_suffix("_RM"))
        .or_else(|| stem.strip_suffix("_HI"))
        .unwrap_or(stem);
    // strip leading "NN_" two-digit ordinal
    let stem = if stem.len() >= 3
        && stem.as_bytes()[0].is_ascii_digit()
        && stem.as_bytes()[1].is_ascii_digit()
        && stem.as_bytes()[2] == b'_'
    {
        &stem[3..]
    } else {
        stem
    };
    if stem.is_empty() {
        None
    } else {
        Some(stem.to_string())
    }
}

#[cfg(test)]
mod strip_tests {
    use super::{norms_chapter_slug, strip_keypoints_and_quiz_fences, strip_wave5_figures_section};

    #[test]
    fn strips_wave5_figures_section() {
        let input =
            "before\n\n<!-- ai_norms-figures-wave5 -->\n## Figures\n\n```figspec\n{}\n```\n";
        let out = strip_wave5_figures_section(input);
        assert_eq!(out, "before");
    }

    #[test]
    fn passes_through_when_marker_absent() {
        let input = "# Title\n\nBody.\n";
        assert_eq!(strip_wave5_figures_section(input), input);
    }

    #[test]
    fn preserves_keypoints_block_before_marker() {
        let input = "<!-- src -->\n\n```keypoints\n- one\n- two\n```\n\n<!-- ai_norms-figures-wave5 -->\n## Figures\n\n```figspec\n{}\n```\n";
        let out = strip_wave5_figures_section(input);
        assert!(out.contains("```keypoints"));
        assert!(!out.contains("## Figures"));
        assert!(!out.contains("figspec"));
    }

    /// Round-D regression: wave-3 table figspecs that sit AFTER the wave-5
    /// marker must survive the strip. Without this carve-out the strip
    /// dropped 20-of-22 AI-Norms tables and failed the captioned-table
    /// parity gate.
    #[test]
    fn preserves_wave3_table_figspec_block_after_wave5_marker() {
        let input = concat!(
            "<!-- src -->\n\n",
            "```keypoints\n- one\n```\n\n",
            "<!-- ai_norms-figures-wave5 -->\n",
            "## Figures\n\n",
            "```figspec\n{\"type\":\"image-embed\"}\n```\n\n",
            "<!-- wave3-table-figspec begin -->\n\n",
            "```figspec\n{\"type\":\"table\",\"id\":\"t1\"}\n```\n\n",
            "<!-- wave3-table-figspec end -->\n",
        );
        let out = strip_wave5_figures_section(input);
        assert!(out.contains("```keypoints"), "keypoints lost: {out}");
        assert!(
            !out.contains("\"image-embed\""),
            "image-embed not stripped: {out}"
        );
        assert!(
            out.contains("wave3-table-figspec begin"),
            "wave3 begin marker missing: {out}"
        );
        assert!(
            out.contains("\"type\":\"table\""),
            "table figspec dropped: {out}"
        );
        assert!(
            out.contains("wave3-table-figspec end"),
            "wave3 end marker missing: {out}"
        );
    }

    /// The wave-3 block can also stand alone (no wave-5 marker) — the
    /// `appendix_tables.extras.md`, `eid.extras.md`, `humanoid_taxonomy.extras.md`
    /// sidecars are like this. Strip is a no-op in that case.
    #[test]
    fn passes_through_when_only_wave3_block_present() {
        let input = "<!-- wave3-table-figspec begin -->\n\n```figspec\n{\"type\":\"table\"}\n```\n\n<!-- wave3-table-figspec end -->\n";
        assert_eq!(strip_wave5_figures_section(input), input);
    }

    /// Round-F: strip the ```keypoints``` fence from a main-chapter md when
    /// the sidecar also owns the canonical keypoints box.
    #[test]
    fn strip_keypoints_fence_removes_block() {
        let input = "Intro.\n\n```keypoints\n- one\n- two\n```\n\nMore prose.\n";
        let out = strip_keypoints_and_quiz_fences(input);
        assert!(
            !out.contains("```keypoints"),
            "keypoints fence not stripped: {out}"
        );
        assert!(!out.contains("- one"), "keypoints body not stripped: {out}");
        assert!(out.contains("Intro."), "intro lost: {out}");
        assert!(out.contains("More prose."), "trailing prose lost: {out}");
    }

    /// Round-F: strip the ```quiz``` fence (Q:/A: block).
    #[test]
    fn strip_quiz_fence_removes_block() {
        let input = "Body.\n\n```quiz\nQ: Why?\nA: Because.\n```\n\nEnd.\n";
        let out = strip_keypoints_and_quiz_fences(input);
        assert!(!out.contains("```quiz"), "quiz fence not stripped: {out}");
        assert!(!out.contains("Q: Why?"), "quiz body not stripped: {out}");
        assert!(out.contains("Body."), "body lost: {out}");
        assert!(out.contains("End."), "trailing lost: {out}");
    }

    /// Round-F: strip both fences when both are present.
    #[test]
    fn strip_both_fences_in_one_pass() {
        let input = concat!(
            "# Heading\n\n",
            "```keypoints\n- alpha\n- beta\n```\n\n",
            "Prose.\n\n",
            "```quiz\nQ: x\nA: y\n```\n",
        );
        let out = strip_keypoints_and_quiz_fences(input);
        assert!(!out.contains("```keypoints"));
        assert!(!out.contains("```quiz"));
        assert!(out.contains("# Heading"));
        assert!(out.contains("Prose."));
    }

    /// Round-F: leave non-keypoints/non-quiz fenced code blocks intact.
    #[test]
    fn strip_preserves_other_code_blocks() {
        let input = "```rust\nfn main() {}\n```\n";
        let out = strip_keypoints_and_quiz_fences(input);
        assert_eq!(out, input);
    }

    /// Round-F: no-op when neither fence is present.
    #[test]
    fn strip_is_noop_without_fences() {
        let input = "# Heading\n\nBody.\n";
        let out = strip_keypoints_and_quiz_fences(input);
        assert_eq!(out, input);
    }

    /// Round-F: slug extraction handles the standard
    /// `out/sources/norms/NN_<slug>_EN.md` layout.
    #[test]
    fn slug_extracts_from_numbered_en_md() {
        assert_eq!(
            norms_chapter_slug("out/sources/norms/10_usa_EN.md").as_deref(),
            Some("usa")
        );
        assert_eq!(
            norms_chapter_slug("out/sources/norms/30_humanoid_usecase_EN.md").as_deref(),
            Some("humanoid_usecase")
        );
        assert_eq!(
            norms_chapter_slug("out/sources/norms/05_introduction_EN.md").as_deref(),
            Some("introduction")
        );
    }

    /// Round-F: slug extraction returns None for non-norms paths so the
    /// dedupe never fires on an unrelated chapter.
    #[test]
    fn slug_returns_none_for_non_norms_paths() {
        assert!(norms_chapter_slug("out/sources/ai_norms/usa.extras.md").is_none());
        assert!(norms_chapter_slug("out/sources/projects/PT-C01-1.md").is_none());
        assert!(norms_chapter_slug("README.md").is_none());
    }

    /// Round V iter-5 (Word COM failure recovery, 2026-06-03).
    ///
    /// `post_finalize_collapse(path, true)` on a docx whose theme part is
    /// absent (the state cascade #16's `ai_norms_and_regulations.docx`
    /// was left in by a Word COM crash) MUST synthesise `word/theme/theme1.xml`
    /// with the reference Calibri/Cambria fonts. Before iter-5 this code
    /// path never executed for ai_norms because the orchestrator gated
    /// the collapse on `finalize_docs(...).is_ok()`, and the 41 MB doc
    /// reliably crashed the PowerShell subprocess. The orchestrator fix
    /// (build() now runs the collapse unconditionally) relies on this
    /// per-file pass being able to repair a theme-less docx — this test
    /// pins that contract at the unit boundary.
    #[test]
    fn post_finalize_collapse_injects_theme_when_word_com_left_it_absent() {
        use std::io::{Read, Write};
        // Build a minimal docx with NO theme part — same shape docx-rs
        // produces and that Word COM would have repaired if it hadn't
        // crashed. Write to a temp file because post_finalize_collapse
        // operates on a path.
        let mut buf = std::io::Cursor::new(Vec::<u8>::new());
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
        let pid = std::process::id();
        let path = std::env::temp_dir().join(format!("round_v_iter5_post_finalize_{pid}.docx"));
        std::fs::write(&path, buf.into_inner()).unwrap();

        // Sanity: input file has no theme part.
        {
            let f = std::fs::File::open(&path).unwrap();
            let mut z = zip::ZipArchive::new(f).unwrap();
            let mut has_theme = false;
            for i in 0..z.len() {
                if z.by_index(i).unwrap().name() == "word/theme/theme1.xml" {
                    has_theme = true;
                }
            }
            assert!(!has_theme, "fixture must start without a theme part");
        }

        super::post_finalize_collapse(&path, true).expect("collapse must succeed");

        // After the unconditional collapse: theme1.xml present with the
        // reference Calibri/Cambria majorFont/minorFont.
        let f = std::fs::File::open(&path).unwrap();
        let mut z = zip::ZipArchive::new(f).unwrap();
        let mut theme_body = String::new();
        let mut found_theme = false;
        let mut ct = String::new();
        for i in 0..z.len() {
            let mut entry = z.by_index(i).unwrap();
            let n = entry.name().to_string();
            if n == "word/theme/theme1.xml" {
                found_theme = true;
                entry.read_to_string(&mut theme_body).unwrap();
            } else if n == "[Content_Types].xml" {
                entry.read_to_string(&mut ct).unwrap();
            }
        }
        let _ = std::fs::remove_file(&path);
        assert!(
            found_theme,
            "post_finalize_collapse must inject theme1.xml even when Word COM left it absent"
        );
        assert!(
            theme_body.contains("Calibri"),
            "synthesised theme must reference Calibri (parity gate THEME::majorFont)"
        );
        assert!(
            theme_body.contains("Cambria"),
            "synthesised theme must reference Cambria (parity gate THEME::minorFont)"
        );
        assert!(
            ct.contains("/word/theme/theme1.xml"),
            "Content_Types must reference the synthesised theme part: {ct}"
        );
    }

    /// Wave-3 iter-E (REF parity 2026-06-04, master_thesis_bookkit sectPr
    /// wiring regression). Pins the END-TO-END flag flow that iter-D added
    /// but a stale cascade snapshot did not exhibit:
    ///   manifest JSON  →  `BookSpec` (serde, `#[serde(default)]`)
    ///                  →  `BookMeta` (`emit_per_chapter_sectpr` field)
    ///                  →  `render_book` (thesis branch, `render_thesis_chapter`)
    ///                  →  `per_chapter_sectpr_paragraph` at chapter close.
    ///
    /// The iter-D engine-level test (`thesis_emit_per_chapter_sectpr_lifts_
    /// section_count`) constructs `BookMeta` directly and so does not cover
    /// the serde + translation legs. This test wires the *manifest* leg —
    /// if a future refactor renames the field or forgets to forward it from
    /// `BookSpec` to `BookMeta`, this test fails before any cascade runs.
    ///
    /// Expectation: ≥10 `<w:sectPr` matches in `word/document.xml` (1 doc-
    /// level + N body chapters, N>9 for the 14-chapter fixture). Word-COM
    /// finalize is out of scope for a `cargo test` job; the cascade output
    /// is verified separately by the bookkit_reference_targets gate.
    #[test]
    fn manifest_emit_per_chapter_sectpr_flows_into_render_book() {
        use std::io::Read;
        // Minimal JSON modelled on the `master_thesis_bookkit` manifest
        // entry: just the structural fields the serde plumbing must carry.
        let json = r#"{
            "key": "manifest_wiring_probe",
            "title": "Per-chapter sectPr — manifest wiring probe",
            "chapters": [],
            "thesis_profile": true,
            "emit_per_chapter_sectpr": true
        }"#;
        let spec: super::BookSpec =
            serde_json::from_str(json).expect("BookSpec must deserialise the bookkit fields");
        assert!(
            spec.thesis_profile,
            "thesis_profile must round-trip from JSON"
        );
        assert!(
            spec.emit_per_chapter_sectpr,
            "emit_per_chapter_sectpr must round-trip from JSON (regression of iter-D's serde plumbing)"
        );

        // Mirror the BookSpec→BookMeta translation that `build_one` performs.
        // We only set the fields the renderer needs for the sectPr count
        // assertion; everything else inherits BookMeta::Default.
        let meta = agentic_export::book::BookMeta {
            title: spec.title.clone(),
            thesis_profile: spec.thesis_profile,
            emit_per_chapter_sectpr: spec.emit_per_chapter_sectpr,
            ..Default::default()
        };
        assert!(
            meta.emit_per_chapter_sectpr,
            "BookMeta must carry the spec's emit_per_chapter_sectpr=true (translation regression)"
        );

        // 14-chapter fixture mirroring the bookkit chapter shape; each
        // chapter is one paragraph long so the renderer's chapter loop
        // fires `per_chapter_sectpr_paragraph` once per chapter.
        let chapters: Vec<(String, String)> = (1..=14)
            .map(|i| {
                (
                    format!("ch{i}"),
                    format!("# Chapter {i}\n\nBody prose for chapter {i}.\n"),
                )
            })
            .collect();
        let bytes = agentic_export::book::render_book(&meta, &chapters, std::path::Path::new("."))
            .expect("render_book must succeed for the thesis profile");
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let mut xml = String::new();
        zip.by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut xml)
            .unwrap();
        let sect_prs = xml.matches("<w:sectPr").count();
        assert!(
            sect_prs >= 10,
            "manifest-driven render must emit ≥10 <w:sectPr (1 doc-level + N body); got {sect_prs}. \
             If this fails, check (1) BookSpec.emit_per_chapter_sectpr deserialisation, \
             (2) the BookSpec→BookMeta translation in build_one, \
             (3) render_book's thesis-profile branch reaching render_thesis_chapter."
        );
    }
}

pub fn run(db_path: &Path, action: BookAction, lang: &str, json_out: bool) -> Result<()> {
    match action {
        BookAction::Build {
            project,
            manifest,
            out,
            only,
        } => build(
            db_path,
            &project,
            &manifest,
            &out,
            only.as_deref(),
            lang,
            json_out,
        ),
        BookAction::Audit { current, previous } => audit(&current, previous.as_deref(), json_out),
        BookAction::Finalize { path, pdf } => finalize(&path, pdf, json_out),
    }
}

/// Collect `.docx` targets: a single file, or every `.docx` in a directory.
fn collect_docx(path: &Path) -> Result<Vec<PathBuf>> {
    if path.is_dir() {
        let mut v: Vec<PathBuf> = std::fs::read_dir(path)
            .with_context(|| format!("read dir {}", path.display()))?
            .filter_map(std::result::Result::ok)
            .map(|e| e.path())
            .filter(|p| {
                p.extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("docx"))
            })
            .collect();
        v.sort();
        Ok(v)
    } else {
        Ok(vec![path.to_path_buf()])
    }
}

/// `book finalize` command: finalize the target(s) through Word. Explicit
/// invocation, so a missing Word is reported as an error.
fn finalize(path: &Path, pdf: bool, json_out: bool) -> Result<()> {
    let docs = collect_docx(path)?;
    if docs.is_empty() {
        anyhow::bail!("no .docx found at {}", path.display());
    }
    let results = finalize_docs(&docs, pdf)?;
    // Round D-C (AI-Norms parity, 2026-06-03): see comment in `build`
    // where this same pass runs after the automatic finalize.
    //
    // Round V zone B (AI-Norms parity, 2026-06-03): `book finalize` is the
    // standalone re-finalize entry point — we cannot tell from the on-disk
    // bytes whether the docx was rendered with `body_render_use_bk_styles`
    // (the flag is render-time only). Default to TRUE: re-injecting the
    // reference theme + styles on a docx that did NOT opt into them is a
    // small visual delta (modern Office-2016 theme → Office-2010
    // Calibri/Cambria pair) but is the safer default for the AI-Norms
    // cascade book renderer where this command is almost always invoked.
    for d in &docs {
        if let Err(e) = post_finalize_collapse(d, true) {
            eprintln!(
                "WARNING: post-finalize header/footer collapse failed for {}: {e}",
                d.display()
            );
        }
    }
    if json_out {
        let arr: Vec<_> = results
            .iter()
            .map(|(p, r)| serde_json::json!({"docx": p, "result": r}))
            .collect();
        println!("{}", serde_json::json!({"finalized": arr}));
    } else {
        for (p, r) in &results {
            println!("  + finalized {p} ({r})");
        }
        println!(
            "Finalized {} document(s) via Microsoft Word.",
            results.len()
        );
    }
    Ok(())
}

/// Drive Microsoft Word (COM, via PowerShell) over all `docs` in ONE Word
/// session — update every field class, repaginate (real page numbers), hide
/// field codes, save — so each opens with no refresh prompt. Per-document
/// errors are reported, not aborted. Mirrors `book_build/finalize.ps1`.
#[cfg(windows)]
fn finalize_docs(docs: &[PathBuf], pdf: bool) -> Result<Vec<(String, String)>> {
    use std::process::Command;
    let mut items: Vec<String> = Vec::new();
    for d in docs {
        let abs = std::fs::canonicalize(d)
            .with_context(|| format!("resolve {}", d.display()))?
            .to_string_lossy()
            .replace(r"\\?\", ""); // strip the verbatim prefix Word rejects
        items.push(format!("'{}'", abs.replace('\'', "''"))); // single-quote escaped
    }
    let arr = items.join(","); // no trailing comma — PowerShell rejects @('x',)
    let pdf_block = if pdf {
        "if($pdf){ $d.SaveAs([ref]([System.IO.Path]::ChangeExtension($pth,'pdf')),[ref]17) }"
    } else {
        ""
    };
    let script = format!(
        r#"$ErrorActionPreference='Stop'
$paths=@({arr})
$pdf=${pdf}
$w = New-Object -ComObject Word.Application
# Round V iter-6 (AI-Norms parity, 2026-06-03): give Word's automation
# server time to fully initialize before the first Documents.Open. On a
# cold start the COM dispatch table is built lazily; firing Documents.Open
# at sub-second latency has been observed to leave the document object
# in a half-loaded state where Fields.Update silently no-ops on the 41 MB
# ai_norms book. 1500 ms is the empirical settle window.
Start-Sleep -Milliseconds 1500
$w.Visible=$false; $w.DisplayAlerts=0
# Round V iter-6: ScreenUpdating off cuts paint time per doc (no UI to
# render anyway, since Visible=$false); grammar/spell-as-you-type off
# stops Word reflowing on every Fields.Update tick — both reduce CPU
# pressure on the 41 MB doc's TOC/Field sweep.
try {{ $w.ScreenUpdating = $false }} catch {{}}
try {{ $w.Options.CheckGrammarAsYouType = $false }} catch {{}}
try {{ $w.Options.CheckSpellingAsYouType = $false }} catch {{}}
$w.Options.ConfirmConversions=$false; $w.Options.UpdateLinksAtOpen=$false
# Round V iter-6: loosen the strict-stop policy INSIDE the per-doc loop
# so a single recoverable non-terminating error (e.g. a Shape.Item index
# transient on the big doc) does not nuke the whole batch. The outer
# try/finally still ensures Quit() runs.
$ErrorActionPreference='Continue'
try {{
  foreach ($pth in $paths) {{
    try {{
      # Round V iter-6: size-scaled settle waits. File size drives how
      # long Word needs to materialize internal structures (paragraphs,
      # sections, runs, field codes). For the 41 MB ai_norms book Word
      # was being asked to Save/Close before its internal save queue
      # drained; the scaled sleep lets the queue land before we move on.
      $fi = Get-Item -LiteralPath $pth
      $sizeMB = [int][math]::Ceiling($fi.Length / 1MB)
      # Open-settle: 500 ms minimum, +25 ms per MB above 5 MB, capped 5000.
      $openSettleMs = [math]::Min(5000, 500 + [math]::Max(0, ($sizeMB - 5)) * 25)
      # Save-drain:  1500 ms minimum, +50 ms per MB above 5 MB, capped 8000.
      $saveDrainMs  = [math]::Min(8000, 1500 + [math]::Max(0, ($sizeMB - 5)) * 50)
      $d = $w.Documents.Open($pth, $false, $false, $false)
      Start-Sleep -Milliseconds $openSettleMs
      # ADR-0050 §1 item 1 (v0.1.15-engine, 2026-05-29): inject the FHNW
      # running header via Word's own API when a sidecar exists. The
      # docx-rs Pic-in-header path emits XML Word silently rejects on
      # open (verified 2026-05-29: header XML on disk is well-formed but
      # InlineShapes.Count == 0 after Documents.Open). Using
      # InlineShapes.AddPicture lets Word build XML Word itself will
      # accept.
      $sidePath = $pth + '.fhnw_header.json'
      if (Test-Path $sidePath) {{
        try {{
          # IMPORTANT: -Encoding UTF8. Windows PowerShell 5.1's Get-Content
          # defaults to the system code page (Windows-1252 in DE/CH locales),
          # which corrupts non-ASCII path characters (`ö` → `Ã¶`) when reading
          # the UTF-8 JSON sidecar written by Rust. Verified 2026-05-29:
          # without -Encoding UTF8 the OneDrive path `Persönlich` becomes
          # `PersÃ¶nlich` and Test-Path fails, silently skipping the logo
          # injection.
          $side = Get-Content $sidePath -Raw -Encoding UTF8 | ConvertFrom-Json
          $cr = [char]13  # Word paragraph mark = CR
          # 2026-05-29 fix: write to Section 1's primary header ONLY.
          # The docx-rs builder emits multiple sections (we saw 5 in the
          # rebuilt master_thesis.docx), all marked LinkToPrevious=True.
          # Setting `.Range.Text = ''` inside a `foreach ($sec in $d.Sections)`
          # loop wipes section 1's header on the second iteration (because
          # section 2 IS section 1 when linked) — and the wipe also drops
          # any InlineShape that was just added. Edit section 1 only; the
          # other sections inherit via LinkToPrevious.
          $sec1Hdrs = $d.Sections.Item(1).Headers
          # ADR-0064 iter11+iter13 (FHNW MT-Template, 2026-07-03): under the
          # MT-Template profile enable different-first-page on EVERY section
          # so chapter-opening pages get their own (bare page-number) header.
          # Section 1 first-page = title-page = empty (no header). Other
          # sections' first-pages get a bare PAGE field (populated below).
          if ([bool]$side.header_pagenum_styleref_enabled) {{
            foreach ($sec in $d.Sections) {{
              $sec.PageSetup.DifferentFirstPageHeaderFooter = -1
            }}
          }} else {{
            $d.Sections.Item(1).PageSetup.DifferentFirstPageHeaderFooter = 0
          }}
          $hdr = $sec1Hdrs.Item(1)
          # Wipe and right-align the whole header.
          $hdr.Range.Text = ''
          $hdr.Range.ParagraphFormat.Alignment = 2  # wdAlignParagraphRight
          # 2026-05-29 debug: surface the parsed logo path + Test-Path result
          # so we can see if non-ASCII characters survive the Rust→PowerShell
          # round-trip.
          $logoPath = $side.logo_path_abs
          $logoExists = if ($logoPath) {{ Test-Path -LiteralPath $logoPath }} else {{ $false }}
          Write-Output ("{{0}}`tHEADER_PROBE  logo=[{{1}}]  exists={{2}}" -f $pth, $logoPath, $logoExists)
            # Layout: FLOATING-anchor logo + text-line paragraphs.
            # The FHNW MAS proposal docx places the logo as a Shape (NOT
            # an InlineShape) anchored to the page at (-49.3, -59.8) pt
            # with wrap=BehindText, so the header text flows in its own
            # right-aligned paragraph while the logo overlays the top-left
            # corner (bleeding slightly off-page). This was the Fix-A
            # target deferred from v0.1.16-engine; the previous attempt
            # failed because it tried `InlineShape.ConvertToShape()` on
            # an already-injected inline picture (Word: "Parameter value
            # out of acceptable range" on header-anchored inlines).
            # `Headers.Shapes.AddPicture(...)` builds the floating shape
            # DIRECTLY — no convert step. Verified live 2026-05-29 against
            # the proposal coordinate dump: bit-for-bit match.
            if ($logoExists) {{
              # Delete any pre-existing inline shape in the header (left
              # over from the v0.1.15..16-engine inline path; defensive
              # so this code path is idempotent on re-runs).
              while ($hdr.Range.InlineShapes.Count -gt 0) {{
                $hdr.Range.InlineShapes.Item(1).Delete()
              }}
              # Add as floating Shape via the Header's Shapes collection
              # — anchor is implicit (the header range). 1 cm = 28.346 pt.
              $W_pt = [double]$side.logo_width_cm * 28.346
              $H_pt = [double]$side.logo_height_cm * 28.346
              $L_pt = [double]$side.logo_left_pt
              $T_pt = [double]$side.logo_top_pt
              $shape = $hdr.Shapes.AddPicture($logoPath, $false, $true, $L_pt, $T_pt, $W_pt, $H_pt)
              # Re-assert L/T after Word's auto-adjust (it sometimes
              # re-anchors to the paragraph on first add; setting the
              # absolute values + relH/relV pins it page-relative).
              $shape.WrapFormat.Type           = [int]$side.logo_wrap_type
              $shape.RelativeHorizontalPosition = [int]$side.logo_relh
              $shape.RelativeVerticalPosition   = [int]$side.logo_relv
              $shape.Left = $L_pt
              $shape.Top  = $T_pt
              Write-Output ("{{0}}`tHEADER_PIC_ADDED  floating-count={{1}}  L={{2}} T={{3}} W={{4}} H={{5}} wrap={{6}}" -f $pth, $hdr.Shapes.Count, [math]::Round($shape.Left,1), [math]::Round($shape.Top,1), [math]::Round($shape.Width,1), [math]::Round($shape.Height,1), $shape.WrapFormat.Type)
            }}
            # 2) Append each non-empty text line, terminated by a paragraph
            # mark. After all text is in place we style the whole header
            # uniformly (font/size/bold).
            foreach ($line in $side.lines) {{
              if ([string]::IsNullOrWhiteSpace($line)) {{ continue }}
              $hdr.Range.InsertAfter($line + $cr)
            }}
            # Apply uniform character formatting to every run in the header
            # (the picture is unaffected; this only touches text runs).
            $hdr.Range.Font.Name = $side.line_font
            $hdr.Range.Font.Size = $side.line_size_pt
            $hdr.Range.Font.Bold = [int]$side.line_bold
            $hdr.Range.ParagraphFormat.Alignment = 2  # right (re-assert)
            # ADR-0064 iter7 (FHNW MT-Template, 2026-07-03): append a
            # bottom-bordered paragraph with STYLEREF Heading 1 (left) +
            # PAGE field (right). Gated on the sidecar flag so proposal-
            # parity behaviour is unchanged.
            if ([bool]$side.header_pagenum_styleref_enabled) {{
              try {{
                if ([bool]$side.header_odd_even_mirrored) {{
                  # Document-level flag for the book-style layout.
                  foreach ($sec in $d.Sections) {{
                    $sec.PageSetup.OddAndEvenPagesHeaderFooter = -1
                  }}
                }}
                # Append a new paragraph at the end of the header range.
                $hdr.Range.InsertAfter($cr)
                $refPara = $hdr.Range.Paragraphs.Last
                $refPara.Alignment = 0  # left
                $refPara.Format.TabStops.ClearAll()
                # Right-aligned tab at 16.5 cm text-width (467 pt).
                $refPara.Format.TabStops.Add(467.0, 2, 0) | Out-Null
                $btm = $refPara.Borders.Item(-3)  # wdBorderBottom
                $btm.LineStyle = 1                # wdLineStyleSingle
                $btm.LineWidth = 4                # wdLineWidth050pt
                # STYLEREF Heading 1 (chapter title) at the paragraph end.
                $insRng = $refPara.Range
                $insRng.Collapse(0)  # wdCollapseEnd
                $d.Fields.Add($insRng, -1, 'STYLEREF "Heading 1"', $false) | Out-Null
                # Tab, then PAGE field.
                $tabRng = $refPara.Range
                $tabRng.Collapse(0)
                $tabRng.InsertAfter([char]9)
                $pgRng = $refPara.Range
                $pgRng.Collapse(0)
                $d.Fields.Add($pgRng, -1, "PAGE", $false) | Out-Null
                Write-Output ("{{0}}`tHEADER_MT_STYLEREF_ADDED  odd_even={{1}}" -f $pth, $side.header_odd_even_mirrored)
                # ADR-0064 iter10 (2026-07-03): also populate the even-page
                # header (Headers.Item(3)) with the mirrored layout: PAGE
                # field (left) + tab + STYLEREF "Heading 1" (right). Word
                # only materialises header3.xml when it has content; that
                # gives us book-style mirrored running heads on verso pages.
                if ([bool]$side.header_odd_even_mirrored) {{
                  $evenHdr = $d.Sections.Item(1).Headers.Item(3)  # wdHeaderFooterEvenPages
                  $evenHdr.Range.Text = ''
                  # Copy the same font styling as the primary header.
                  $evenHdr.Range.Font.Name = $side.line_font
                  $evenHdr.Range.Font.Size = $side.line_size_pt
                  $evenHdr.Range.Font.Bold = [int]$side.line_bold
                  # Insert first paragraph — the mirrored PAGE + STYLEREF line.
                  $evenHdr.Range.InsertAfter($cr)
                  $evenPara = $evenHdr.Range.Paragraphs.Last
                  $evenPara.Alignment = 0  # left
                  $evenPara.Format.TabStops.ClearAll()
                  $evenPara.Format.TabStops.Add(467.0, 2, 0) | Out-Null  # right tab at text-width
                  $evenBtm = $evenPara.Borders.Item(-3)
                  $evenBtm.LineStyle = 1
                  $evenBtm.LineWidth = 4
                  # PAGE field on the left, then tab, then STYLEREF Heading 1.
                  $eIns = $evenPara.Range
                  $eIns.Collapse(0)
                  $d.Fields.Add($eIns, -1, "PAGE", $false) | Out-Null
                  $eTab = $evenPara.Range
                  $eTab.Collapse(0)
                  $eTab.InsertAfter([char]9)
                  $eStyle = $evenPara.Range
                  $eStyle.Collapse(0)
                  $d.Fields.Add($eStyle, -1, 'STYLEREF "Heading 1"', $false) | Out-Null
                  Write-Output ("{{0}}`tHEADER_MT_EVEN_ADDED" -f $pth)
                }}
                # ADR-0064 iter12 (2026-07-03): ensure ALL sections 2+
                # inherit the Section 1 primary + even-page headers via
                # LinkToPrevious. Without this, Word may leave some
                # sections with empty headers on their odd/even pages.
                for ($s = 2; $s -le $d.Sections.Count; $s++) {{
                  try {{
                    $d.Sections.Item($s).Headers.Item(1).LinkToPrevious = $true
                    if ([bool]$side.header_odd_even_mirrored) {{
                      $d.Sections.Item($s).Headers.Item(3).LinkToPrevious = $true
                    }}
                  }} catch {{
                    # non-fatal — Word sometimes rejects LinkToPrevious on
                    # sections whose Break type is Continuous. Continue.
                  }}
                }}
                # ADR-0064 iter21 (2026-07-03): create a multilevel outline
                # ListTemplate and bind LinkedStyle at each level to Heading 1/2/3.
                # Word auto-numbers headings ("1", "1.1", "1.1.1") from this
                # binding. Ports MT-Template/build/post_process.ps1:113-180.
                # Then strip the list from front/back-matter H1s (they must stay
                # unnumbered) — same logic as post_process.ps1:184-211.
                try {{
                  $wdArabic = 0
                  $wdTrailingTab  = 0
                  $wdTrailingNone = 2
                  $lt = $null
                  foreach ($cand in $d.ListTemplates) {{
                    try {{
                      $l2 = $cand.ListLevels.Item(2)
                      if ($l2.LinkedStyle -eq "Heading 2" -and $l2.NumberFormat -like "*%1.%2*") {{ $lt = $cand; break }}
                    }} catch {{}}
                  }}
                  if ($lt -eq $null) {{
                    $lt = $d.ListTemplates.Add($true)  # OutlineNumbered
                  }}
                  $l = $lt.ListLevels.Item(1)
                  $l.NumberFormat = ""
                  $l.NumberStyle = $wdArabic
                  $l.TrailingCharacter = $wdTrailingNone
                  $l.NumberPosition = 0
                  $l.TextPosition = 0
                  $l.TabPosition = 0
                  $l = $lt.ListLevels.Item(2)
                  $l.NumberFormat = "%1.%2"
                  $l.NumberStyle = $wdArabic
                  $l.TrailingCharacter = $wdTrailingTab
                  $l.NumberPosition = 0
                  $l.TextPosition = 28.35
                  $l.TabPosition = 28.35
                  $l = $lt.ListLevels.Item(3)
                  $l.NumberFormat = "%1.%2.%3"
                  $l.NumberStyle = $wdArabic
                  $l.TrailingCharacter = $wdTrailingTab
                  $l.NumberPosition = 0
                  $l.TextPosition = 42.5
                  $l.TabPosition = 42.5
                  $lt.ListLevels.Item(1).LinkedStyle = "Heading 1"
                  $lt.ListLevels.Item(2).LinkedStyle = "Heading 2"
                  $lt.ListLevels.Item(3).LinkedStyle = "Heading 3"
                  # ADR-0064 iter25 (2026-07-03): the LinkedStyle binding above
                  # numbers H2/H3 at render time but doesn't always persist a
                  # numId per paragraph to the XML — Word emits the numId only
                  # if `ListFormat.ApplyListTemplate` is explicitly called on
                  # the paragraph. Force-apply here so H2/H3 numId references
                  # land in document.xml. `$false, $wdContinueNumbering (0),
                  # $wdWord10ListBehavior (2)` matches Word's default paste.
                  $wdContinueNumbering = 0
                  $wdWord10ListBehavior = 2
                  foreach ($para in $d.Paragraphs) {{
                    try {{
                      $sn = $para.Style.NameLocal
                      if ($sn -eq "Heading 2" -or $sn -eq "Heading 3" -or $sn -eq "Heading 1") {{
                        $para.Range.ListFormat.ApplyListTemplate(
                          $lt, $false, $wdContinueNumbering, $wdWord10ListBehavior
                        ) | Out-Null
                      }}
                    }} catch {{}}
                  }}
                  # Strip numbering from front/back-matter H1s using the
                  # ChapterNumber landmark (from iter16).
                  $wdNumberParagraph = 1
                  $stripped = 0
                  foreach ($para in $d.Paragraphs) {{
                    try {{
                      if ($para.Style.NameLocal -eq "Heading 1") {{
                        # Check whether this H1 is inside the main-matter (i.e.,
                        # a section that contains a ChapterNumber paragraph).
                        $paraSec = [int] $para.Range.Information(2)
                        if ($chapterSecs -and ($chapterSecs -contains $paraSec)) {{
                          continue  # main matter — keep the list numbering
                        }}
                        $para.Range.ListFormat.RemoveNumbers($wdNumberParagraph) | Out-Null
                        $stripped += 1
                      }}
                    }} catch {{}}
                  }}
                  Write-Output ("{{0}}`tLIST_TEMPLATE_APPLIED  stripped_frontback_h1s={{1}}" -f $pth, $stripped)
                }} catch {{
                  [Console]::Error.WriteLine(("FHNW-MT-LIST-FAIL [{{0}}]: {{1}}" -f $pth, $_.Exception.Message))
                }}
                # ADR-0064 iter16 + iter26 (2026-07-03): apply Roman-then-Arabic
                # page numbering per section. Iter26 prefers the emitted
                # bookmarks `fhnwFrontMatterEnd` + `fhnwBackMatterStart` over the
                # ChapterNumber landmark heuristic (matches
                # MT-Template/build/post_process.ps1:83-108 exactly). Falls back
                # to ChapterNumber detection when the bookmarks are absent.
                try {{
                  $wdPageNumberStyleLowercaseRoman = 2
                  $wdPageNumberStyleArabic = 0
                  $bmFrontEnd = $null
                  $bmBackStart = $null
                  try {{ $bmFrontEnd  = $d.Bookmarks.Item("fhnwFrontMatterEnd") }} catch {{}}
                  try {{ $bmBackStart = $d.Bookmarks.Item("fhnwBackMatterStart") }} catch {{}}
                  $chapterSecs = @()
                  if (($bmFrontEnd -ne $null) -and ($bmBackStart -ne $null)) {{
                    # Bookmarks landed: derive main-matter section range directly.
                    $frontEndSec  = [int] $bmFrontEnd.Range.Information(2)
                    $backStartSec = [int] $bmBackStart.Range.Information(2)
                    $chapterSecs = @(($frontEndSec + 1)..($backStartSec - 1))
                    Write-Output ("{{0}}`tROMAN_BOOKMARKS_OK  front_end={{1}}  back_start={{2}}" -f $pth, $frontEndSec, $backStartSec)
                  }} else {{
                    # Fallback to iter16 ChapterNumber landmark heuristic.
                    foreach ($para in $d.Paragraphs) {{
                      try {{
                        if ($para.Style.NameLocal -eq "ChapterNumber" -or $para.Style.NameLocal -eq "Chapter Number") {{
                          $chapterSecs += [int] $para.Range.Information(2)
                        }}
                      }} catch {{}}
                    }}
                  }}
                  if ($chapterSecs.Count -gt 0) {{
                    $mainStart = ($chapterSecs | Measure-Object -Minimum).Minimum
                    $mainEnd   = ($chapterSecs | Measure-Object -Maximum).Maximum
                    # ADR-0064 iter31 (2026-07-03): force per-section pgNumType
                    # in the saved XML by dirtying each section's PageSetup +
                    # explicitly calling RestartNumberingAtSection=$true with the
                    # computed StartingNumber. The reference EN.docx has 14
                    # pgNumType entries (front / main / back); my earlier logic
                    # let Word compress the format across sections into just 3.
                    # Reasserting StartingNumber per section forces the save.
                    $wdSectionNewPage = 2
                    $pageAccum = 0
                    for ($s = 1; $s -le $d.Sections.Count; $s++) {{
                      $sec = $d.Sections.Item($s)
                      $sec.PageSetup.SectionStart = $wdSectionNewPage  # dirty the sectPr
                      $pn = $sec.Headers.Item(1).PageNumbers
                      if ($s -lt $mainStart) {{
                        # Front matter → lowercase Roman with explicit start per section.
                        $pn.NumberStyle = $wdPageNumberStyleLowercaseRoman
                        $pn.RestartNumberingAtSection = $true
                        $pn.StartingNumber = 1 + $pageAccum
                        $pageAccum += 1
                      }} elseif ($s -le $mainEnd) {{
                        # Main matter → Arabic. Restart at section 1 = Arabic 1.
                        $pn.NumberStyle = $wdPageNumberStyleArabic
                        if ($s -eq $mainStart) {{
                          $pn.RestartNumberingAtSection = $true
                          $pn.StartingNumber = 1
                          $pageAccum = 0  # Arabic starts fresh
                        }} else {{
                          $pn.RestartNumberingAtSection = $false
                        }}
                      }} else {{
                        # Back matter → lowercase Roman continuing from front matter.
                        $pn.NumberStyle = $wdPageNumberStyleLowercaseRoman
                        $pn.RestartNumberingAtSection = $true
                        # Continue from front matter's final Roman.
                        # (Approximate: pageAccum reset above so this restarts —
                        # correct if the auto-tune post-pass computes the real
                        # value from `fhnwBackMatterStart` bookmark.)
                        $pn.StartingNumber = 1
                      }}
                    }}
                    Write-Output ("{{0}}`tHEADER_MT_ROMAN_ARABIC  main_start={{1}}  main_end={{2}}  sections={{3}}" -f $pth, $mainStart, $mainEnd, $d.Sections.Count)
                  }} else {{
                    Write-Output ("{{0}}`tHEADER_MT_ROMAN_ARABIC_SKIP  no ChapterNumber paragraphs found" -f $pth)
                  }}
                }} catch {{
                  [Console]::Error.WriteLine(("FHNW-MT-ROMAN-FAIL [{{0}}]: {{1}}" -f $pth, $_.Exception.Message))
                }}
                # ADR-0064 iter14 (2026-07-03): chapter-opening pages get a
                # bare right-aligned PAGE field on odd pages (book
                # convention: page-no on outer margin). Populate Section 2's
                # firstPageHeader (Headers.Item(2)); Sections 3+ inherit via
                # LinkToPrevious. Section 1's firstPageHeader remains empty
                # (the title page has no header at all — iter11).
                if ($d.Sections.Count -ge 2) {{
                  try {{
                    $fpHdr = $d.Sections.Item(2).Headers.Item(2)  # wdHeaderFooterFirstPage
                    $fpHdr.LinkToPrevious = $false
                    $fpHdr.Range.Text = ''
                    $fpHdr.Range.Font.Name = $side.line_font
                    $fpHdr.Range.Font.Size = $side.line_size_pt
                    $fpHdr.Range.Font.Bold = $false
                    $fpHdr.Range.ParagraphFormat.Alignment = 2  # right-aligned
                    $fpRng = $fpHdr.Range
                    $fpRng.Collapse(0)
                    $d.Fields.Add($fpRng, -1, "PAGE", $false) | Out-Null
                    # Sections 3+ inherit the firstPage header from Section 2.
                    for ($s = 3; $s -le $d.Sections.Count; $s++) {{
                      try {{ $d.Sections.Item($s).Headers.Item(2).LinkToPrevious = $true }} catch {{}}
                    }}
                    Write-Output ("{{0}}`tHEADER_MT_FIRSTPAGE_ADDED" -f $pth)
                  }} catch {{
                    [Console]::Error.WriteLine(("FHNW-MT-FP-FAIL [{{0}}]: {{1}}" -f $pth, $_.Exception.Message))
                  }}
                }}
              }} catch {{
                $hmsg = "HEADER_MT_STYLEREF_ERR {{0}} | trace: {{1}}" -f $_.Exception.Message, ($_.ScriptStackTrace -replace "`r?`n", " >> ")
                Write-Output ("{{0}}`t{{1}}" -f $pth, $hmsg)
                [Console]::Error.WriteLine(("FHNW-MT-HDR-FAIL [{{0}}]: {{1}}" -f $pth, $hmsg))
              }}
            }}
            Write-Output ("{{0}}`tHEADER_DONE  floating-count={{1}}  inline-count={{2}}  paras={{3}}" -f $pth, $hdr.Shapes.Count, $hdr.Range.InlineShapes.Count, $hdr.Range.Paragraphs.Count)
            # ADR-0050 §17 / ADR-0030 §37 — centred page-number footer.
            # docx-rs 0.4.20 attaches only ONE Footer per Document
            # (verified 2026-05-29 by unzipping the rendered docx: 4
            # footer*.xml files where only 1 has the PAGE field; the
            # others are Word-generated empty stubs for each section).
            # Result: most pages render without a footer. Fix: walk
            # every section, write the centred PAGE field into Sec1's
            # primary footer, then set LinkToPrevious=True on Sec2+.
            if ([bool]$side.footer_pagenum_enabled) {{
              try {{
                $sec1Ftr = $d.Sections.Item(1).Footers.Item(1)
                $sec1Ftr.Range.Text = ''
                # Use Word's NATIVE Footer.PageNumbers.Add — handles the
                # PAGE field, paragraph alignment, AND auto-computes the
                # display value on every page. Fields.Add with the raw
                # PAGE code created the field but Word didn't auto-
                # compute it (cached '0'); PageNumbers.Add does the
                # complete job. The alignment param is a
                # WdPageNumberAlignment: 0=Left 1=Center 2=Right
                # 3=Inside 4=Outside. We map the sidecar
                # footer_pagenum_alignment (paragraph wdAlignParagraph*,
                # 0=L 1=C 2=R 3=J) onto the page-number variant by
                # taking 0/1/2 directly; J (3) falls back to Center.
                $pnAlign = [int]$side.footer_pagenum_alignment
                if ($pnAlign -lt 0 -or $pnAlign -gt 2) {{ $pnAlign = 1 }}
                $sec1Ftr.PageNumbers.Add($pnAlign, $true) | Out-Null
                # Style the inserted page number (Arial 11pt black,
                # proposal-parity).
                $sec1Ftr.Range.Font.Name = $side.footer_pagenum_font
                $sec1Ftr.Range.Font.Size = [int]$side.footer_pagenum_size_pt
                $sec1Ftr.Range.Font.Bold = $false
                # Sections 2+ inherit from Sec1 via LinkToPrevious. This
                # replaces the Word-generated empty footers with the
                # Sec1 page-number content.
                for ($s = 2; $s -le $d.Sections.Count; $s++) {{
                  $d.Sections.Item($s).Footers.Item(1).LinkToPrevious = $true
                }}
                Write-Output ("{{0}}`tFOOTER_PAGENUM_ADDED  sections={{1}}  field-count={{2}}" -f $pth, $d.Sections.Count, $sec1Ftr.Range.Fields.Count)
              }} catch {{
                $fmsg = "FOOTER_INJECT_ERR {{0}} | trace: {{1}}" -f $_.Exception.Message, ($_.ScriptStackTrace -replace "`r?`n", " >> ")
                Write-Output ("{{0}}`t{{1}}" -f $pth, $fmsg)
                [Console]::Error.WriteLine(("FHNW-FTR-FAIL [{{0}}]: {{1}}" -f $pth, $fmsg))
              }}
            }}
        }} catch {{
          # Write to BOTH stdout (so the Rust filter sees it) AND stderr
          # (so the user sees it raw in case something else filters
          # stdout). The full ScriptStackTrace is included so we can find
          # which line failed without re-running the script manually.
          $msg = "HEADER_INJECT_ERR {{0}} | trace: {{1}}" -f $_.Exception.Message, ($_.ScriptStackTrace -replace "`r?`n", " >> ")
          Write-Output ("{{0}}`t{{1}}" -f $pth, $msg)
          [Console]::Error.WriteLine(("FHNW-HDR-FAIL [{{0}}]: {{1}}" -f $pth, $msg))
        }}
      }}
      $d.Fields.Update() | Out-Null
      # 2026-05-29 Fix-G1: $d.Fields.Update() only refreshes the
      # MAIN BODY story. Header/footer fields (specifically the
      # PAGE field newly injected by FOOTER_PAGENUM_ADDED) live in
      # separate StoryRanges and stay at their cached value (often
      # "0") unless explicitly updated. Walk every story and refresh
      # its fields so the page-number footer shows real numbers.
      try {{
        foreach ($story in $d.StoryRanges) {{
          $story.Fields.Update() | Out-Null
          $nxt = $story.NextStoryRange
          while ($nxt -ne $null) {{
            $nxt.Fields.Update() | Out-Null
            $nxt = $nxt.NextStoryRange
          }}
        }}
      }} catch {{
        # StoryRanges iteration can fail on docs with no linked
        # stories — non-fatal; the main update above already covered
        # the body fields, headers/footers may still show "0" but
        # Word will refresh them on user open via updateFields.
        [Console]::Error.WriteLine(("STORY_UPDATE_WARN [{{0}}]: {{1}}" -f $pth, $_.Exception.Message))
      }}
      foreach ($tof in $d.TablesOfFigures) {{ $tof.Update() }}
      foreach ($toc in $d.TablesOfContents) {{ $toc.Update() }}
      foreach ($ix in $d.Indexes) {{ $ix.Update() }}
      try {{ $d.ActiveWindow.View.ShowFieldCodes=$false }} catch {{}}
      $d.Repaginate()
      $pages=$d.ComputeStatistics(2)
      $finalHdr = $d.Sections.Item(1).Headers.Item(1)
      Write-Output ("{{0}}`tHEADER_PRE_SAVE  floating-count={{1}}  inline-count={{2}}" -f $pth, $finalHdr.Shapes.Count, $finalHdr.Range.InlineShapes.Count)
      # Round V iter-6: SaveAs2 with explicit wdFormatXMLDocument (12)
      # over plain Save(). Explicit format eliminates the rare case where
      # Word infers DOC instead of DOCX on a malformed in-memory state,
      # and gives a clearer COM signature (single positional + format)
      # than Save()'s zero-arg overload.
      $wdFormatXMLDocument = 12
      $d.SaveAs2($d.FullName, $wdFormatXMLDocument)
      # ADR-0064 iter27 (2026-07-03): also emit .dotx companion for the
      # FHNW MT-Template profile (ports MT-Template/build/post_process.ps1:230).
      # `wdFormatXMLTemplate = 14`. Only fires for master_thesis (where the
      # sidecar exists AND the odd/even flag is on = FhnwMtTemplate).
      if ((Test-Path $sidePath) -and ([bool]$side.header_odd_even_mirrored)) {{
        try {{
          $dotxPath = [System.IO.Path]::ChangeExtension($d.FullName, ".dotx")
          $wdFormatXMLTemplate = 14
          $d.SaveAs2($dotxPath, $wdFormatXMLTemplate)
          Write-Output ("{{0}}`tDOTX_SAVED  path={{1}}" -f $pth, $dotxPath)
        }} catch {{
          [Console]::Error.WriteLine(("FHNW-DOTX-FAIL [{{0}}]: {{1}}" -f $pth, $_.Exception.Message))
        }}
      }}
      # Let Word's internal save queue drain before we Close — without
      # this wait the 41 MB ai_norms doc has been observed to throw a
      # COM "file in use" race on Close($false) because the async save
      # thread had not yet released its file handle.
      Start-Sleep -Milliseconds $saveDrainMs
      {pdf_block}
      $d.Close($false)
      Start-Sleep -Milliseconds 250
      Write-Output ("{{0}}`tpages={{1}}" -f $pth, $pages)
    }} catch {{
      Write-Output ("{{0}}`tERROR {{1}}" -f $pth, $_.Exception.Message)
    }}
  }}
}} finally {{
  # Round V iter-6: give Word a 5 s window after the last Close before
  # we Quit — Word's internal pagination + autorecover daemons run
  # asynchronously after Close($false) and forcing Quit too early was
  # the empirical trigger for the non-zero exit on the AI-norms run.
  Start-Sleep -Milliseconds 5000
  $w.Quit()
  [System.Runtime.InteropServices.Marshal]::ReleaseComObject($w) | Out-Null
}}"#,
        pdf = if pdf { "true" } else { "false" }
    );
    // ADR-0064 iter28 (2026-07-03): the embedded PowerShell script has grown
    // past Windows' CreateProcess 32 KB command-line limit, causing
    // "os error 206: The filename or extension is too long" and skipping the
    // whole Word-COM finalize batch (0 headerRef, no .dotx in the snapshot).
    // Fix: write the script to a temp file and invoke `powershell -File`,
    // which reads the script through the file system and bypasses the
    // command-line limit entirely.
    let script_path =
        std::env::temp_dir().join(format!("agentic_finalize_{}.ps1", std::process::id()));
    // ADR-0064 iter29 (2026-07-03): PowerShell's `-File` reads the script
    // using the system codepage (Windows-1252 in DE/CH locales) unless
    // a UTF-8 BOM is present. Without it, non-ASCII path characters
    // (`Persönlich` in the deliverable path) corrupt into `PersÃ¶nlich` and
    // every downstream $pth reference fails ("ERROR Command failed" for
    // every book in the cascade). Prepend the BOM so PS reads as UTF-8.
    let mut script_bytes = Vec::with_capacity(script.len() + 3);
    script_bytes.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
    script_bytes.extend_from_slice(script.as_bytes());
    std::fs::write(&script_path, &script_bytes)
        .context("write PowerShell finalize script to temp file")?;
    let out = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(&script_path)
        .output()
        .context("launch Word via powershell (is Microsoft Word installed?)")?;
    // Best-effort delete of the temp script (leave on failure for post-mortem).
    if out.status.success() {
        let _ = std::fs::remove_file(&script_path);
    } else {
        eprintln!(
            "note: finalize PowerShell script kept for post-mortem at {}",
            script_path.display()
        );
    }
    if !out.status.success() {
        anyhow::bail!(
            "Word finalize failed (is Microsoft Word installed?): {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    // Surface any FHNW-HDR-FAIL / FHNW-FTR-FAIL diagnostics from
    // stderr to the user.
    let stderr = String::from_utf8_lossy(&out.stderr);
    for line in stderr.lines() {
        if line.contains("FHNW-HDR-FAIL") || line.contains("FHNW-FTR-FAIL") {
            eprintln!("{line}");
        }
    }
    let stdout = String::from_utf8_lossy(&out.stdout);

    // Clean up the FHNW header sidecar files (`*.docx.fhnw_header.json` +
    // `*.fhnw_logo.png`). They're written by `build_one` (see line ~668)
    // as input for THIS finalize step; once Word has injected the
    // header + footer they have no further value and only pollute the
    // snapshot directory. We delete them per-docx after the run
    // completes successfully (we don't reach this code if the whole
    // PowerShell pipeline failed).
    for doc in docs {
        let sidecar = doc.with_extension("docx.fhnw_header.json");
        let _ = std::fs::remove_file(&sidecar);
        // Logo file is named `<key>.fhnw_logo.png` (sibling of the
        // docx, not a `.docx.fhnw_logo.png` suffix). Derive the key
        // by stripping `.docx`.
        if let Some(stem) = doc.file_stem().and_then(|s| s.to_str()) {
            if let Some(parent) = doc.parent() {
                let logo = parent.join(format!("{stem}.fhnw_logo.png"));
                let _ = std::fs::remove_file(&logo);
            }
        }
    }

    Ok(stdout
        .lines()
        .filter_map(|l| {
            l.split_once('\t')
                .map(|(p, r)| (p.to_string(), r.to_string()))
        })
        .collect())
}

#[cfg(not(windows))]
fn finalize_docs(_docs: &[PathBuf], _pdf: bool) -> Result<Vec<(String, String)>> {
    anyhow::bail!(
        "Word finalize requires Microsoft Word on Windows; elsewhere the rendered \
         .docx carries updateFields so Word refreshes on open"
    )
}

/// Round D-C (AI-Norms parity, 2026-06-03): read the docx at `path`, run
/// the empty-header/footer-part collapse pass (see
/// [`agentic_export::book::collapse_empty_header_footer_parts`]), and
/// write the result back. Used by `build` after `finalize_docs` returns
/// to undo Word COM's regeneration of the default/even/firstPage
/// header & footer triad.
///
/// Round V zone B (AI-Norms parity, 2026-06-03) also re-injects the
/// reference Office-2010 theme1.xml and the 186-style styles.xml when
/// `restore_theme_and_styles=true` — Word COM silently regenerates both
/// every time it touches a docx, so the render-time replacement is
/// otherwise undone. The caller plumbs the
/// `BookMeta::body_render_use_bk_styles` flag through the finalize
/// boundary so non-parity books (where the docx-rs default styles are
/// intentional) skip the restoration.
///
/// Idempotent: re-running on an already-collapsed docx with already-
/// matching theme/styles is a no-op.
fn post_finalize_collapse(path: &Path, restore_theme_and_styles: bool) -> Result<()> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("read {} for collapse pass", path.display()))?;
    let collapsed = agentic_export::book::collapse_empty_header_footer_parts(bytes)
        .with_context(|| format!("collapse pass for {}", path.display()))?;
    // Wave-2-D bookkit routing: choose the styles fixture by docx basename. The
    // master_thesis_bookkit profile ships a 178-style FHNW thesis reference;
    // all other books (AI Norms, campaigns, master_thesis, etc.) use the 186-
    // style AI-Norms verbatim port. Decision is per-file because the cascade
    // loops over rendered docs after finalize_docs returns.
    let styles_profile = match path.file_name().and_then(|s| s.to_str()) {
        Some("master_thesis_bookkit.docx") | Some("master_thesis.docx") => {
            agentic_export::thesis_styles::StylesProfile::FhnwMasterThesis
        }
        _ => agentic_export::thesis_styles::StylesProfile::AiNorms,
    };
    let with_styles = if restore_theme_and_styles {
        // Round V zone B: re-inject theme1.xml + styles.xml AFTER Word COM
        // finalize so the Office-2010 Calibri/Cambria pair + the right styles
        // fixture (per profile) survive the round-trip. Without this step
        // Word's save silently overwrites both with its modern Office-2016
        // defaults.
        agentic_export::book::restore_reference_theme_and_styles(collapsed, styles_profile)
            .with_context(|| format!("theme/styles restore for {}", path.display()))?
    } else {
        collapsed
    };
    // ADR-0064 iter33 (2026-07-04): master_thesis only — post-COM XML
    // injection of `<w:pgNumType>` per sectPr. Word's save-time optimizer
    // strips per-section pgNumType when adjacent sections share the same
    // NumberStyle; the reference FHNW EN.docx has explicit pgNumType on
    // 14 of 21 sectPrs. Restore that explicit XML representation so the
    // Roman/Arabic/Roman scheme survives a downstream reopen.
    let final_bytes = match path.file_name().and_then(|s| s.to_str()) {
        Some("master_thesis.docx") => {
            agentic_export::book::inject_pgnumtype_per_section(with_styles)
                .with_context(|| format!("pgNumType injection for {}", path.display()))?
        }
        _ => with_styles,
    };
    std::fs::write(path, &final_bytes)
        .with_context(|| format!("write collapsed {}", path.display()))?;
    Ok(())
}

fn build(
    db_path: &Path,
    project: &str,
    manifest_path: &Path,
    out: &Path,
    only: Option<&str>,
    lang: &str,
    json_out: bool,
) -> Result<()> {
    let conn = agentic_core::db::open(db_path)?;
    let text = std::fs::read_to_string(manifest_path)
        .with_context(|| format!("reading manifest {}", manifest_path.display()))?;
    let manifest: Manifest = serde_json::from_str(&text).context("parsing manifest JSON")?;
    std::fs::create_dir_all(out)?;

    // Honor model-review exclusions (ADR-0049 ph3): a chapter whose current
    // model_review verdict is "exclude" is held out of the mainline build.
    // Append-only — a later "accept" review supersedes and re-includes it.
    let excluded = agentic_core::review::excluded_paths(&conn, project).unwrap_or_default();

    let mut report = Vec::new();
    let mut built = 0usize;
    let mut built_docs: Vec<PathBuf> = Vec::new();
    // Round V zone B (AI-Norms parity, 2026-06-03): track per-book whether
    // the bk-styles parity bundle was emitted, so the post-finalize restore
    // step (theme1.xml + styles.xml re-injection) only runs on books that
    // opted in. Same length / index as `built_docs`.
    let mut built_use_bk: Vec<bool> = Vec::new();
    for spec in &manifest.books {
        if let Some(k) = only {
            if spec.key != k {
                continue;
            }
        }
        // Per-book scratch dir in the system temp — created and DELETED within
        // this processing step so the output dir never accumulates intermediates
        // and a crash leaves at most one book's scratch (not a global wipe).
        let work = std::env::temp_dir().join(format!(
            "agentic_book_{}_{}",
            sanitize(&spec.key),
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&work);
        std::fs::create_dir_all(&work)?;

        let result = build_one(&conn, project, spec, &work, out, lang, &excluded);
        let _ = std::fs::remove_dir_all(&work); // always clean this step's scratch
        match result {
            Ok((figs, bytes, held)) => {
                built += 1;
                built_docs.push(out.join(format!("{}.docx", spec.key)));
                built_use_bk.push(spec.body_render_use_bk_styles);
                let included = spec.chapters.len() - held.len();
                report.push(serde_json::json!({
                    "key": spec.key, "chapters": included, "held_by_review": held,
                    "figures": figs, "docx_bytes": bytes
                }));
                if !json_out {
                    let held_note = if held.is_empty() {
                        String::new()
                    } else {
                        format!(", {} held by review", held.len())
                    };
                    println!(
                        "  + {}.docx  ({} figures, {included}/{} chapters{held_note})",
                        spec.key,
                        figs,
                        spec.chapters.len()
                    );
                }
            }
            // Show the full anyhow context chain (`{e:#}`) so a book-build
            // failure includes the inner cause (e.g. "rendering figspec
            // 'figXX_NN': …", "unterminated figspec block") rather than
            // only the outermost "resolving figspecs in chapter …" wrapper.
            // The 2026-06-07 single-line-dump regression went undiagnosed
            // through several cascades because the chain was hidden.
            Err(e) => eprintln!("  ! {} FAILED: {e:#}", spec.key),
        }
    }
    // Render report (used by `book audit` to compare iterations).
    std::fs::write(
        out.join("_render_report.json"),
        serde_json::to_string_pretty(&serde_json::json!({ "books": report }))?,
    )?;
    // Always finalize through Microsoft Word so the books open with no "update
    // fields" prompt (TOC / List of Figures/Tables / Index populated, real page
    // numbers). Generation itself never fails on this, but an unmet prerequisite
    // (Word absent / non-Windows) is WARNED loudly to stderr — never silently
    // skipped — because the resulting docx will then need a field refresh on open.
    if !built_docs.is_empty() {
        // Round V iter-6 (AI-Norms parity, 2026-06-03): size-ascending order
        // before handing the batch to Word COM. The ai_norms_and_regulations
        // docx is ~41 MB; warming Word's pagination / field-update engine on
        // the smaller books first means by the time it hits ai_norms its
        // internal caches (style table, theme parts, paragraph layout cache)
        // are populated, which materially reduces the chance Word stalls or
        // hangs during the full Fields.Update() sweep. `built_use_bk` is
        // positionally tied to `built_docs`, so sort the zipped pairs.
        let mut zipped: Vec<(PathBuf, bool)> = built_docs
            .iter()
            .cloned()
            .zip(built_use_bk.iter().copied())
            .collect();
        zipped.sort_by_key(|(p, _)| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0));
        built_docs = zipped.iter().map(|(p, _)| p.clone()).collect();
        built_use_bk = zipped.iter().map(|(_, b)| *b).collect();
        // Round V iter-5 (AI-Norms parity, 2026-06-03): drive Word COM, BUT
        // run the post-finalize collapse + theme/styles restoration step
        // UNCONDITIONALLY afterwards. Previously the collapse loop lived
        // inside the `Ok(res)` arm of the match, so when Word COM failed —
        // most notoriously on the 41 MB `ai_norms_and_regulations.docx`,
        // where the whole PowerShell pipeline exits non-zero — `theme1.xml`,
        // the 186-style `styles.xml`, and the empty header/footer collapse
        // were never re-applied, dropping the parity-gate THEME::majorFont
        // / THEME::minorFont assertions to zero. The collapse pass is
        // idempotent (`restore_reference_theme_and_styles` checks whether
        // the theme part is already present before patching CT + rels;
        // `collapse_empty_header_footer_parts` operates on the on-disk
        // bytes) so running it after a SUCCESSFUL Word save is identical
        // to running it after a FAILED one. The user-facing warnings about
        // unrefreshed fields are preserved in the failure branch.
        //
        // Round V iter-9 (AI-Norms parity, 2026-06-03): the per-book
        // `use_bk` flag previously decided whether to re-inject the
        // reference theme + styles. That was a misoptimisation: Word COM
        // regenerates `word/theme/theme1.xml` (Aptos / Aptos Display, the
        // Office 2024 default) and `word/styles.xml` UNCONDITIONALLY when
        // it saves a docx, regardless of which book ran. Gating the
        // restore on `use_bk` (which defaults to `false` for the ai_norms
        // manifest entry until iter-9) meant the parity-gate THEME::majorFont
        // (got `Aptos Display`, expected `Calibri`) and THEME::minorFont
        // (got `Aptos`, expected `Cambria`) sub-checks failed; the
        // Hyperlink color drifted to Word's `467886` instead of the
        // reference `0000FF`; and Word's default 3-headers + 4-footers
        // triad survived the collapse pass. Pass `true` UNCONDITIONALLY —
        // matches the `finalize` standalone command (line 597-601 comment)
        // and is safe for non-parity books (the Office-2010 Calibri/Cambria
        // pair is a minor cosmetic change vs Word's Office-2024 default).
        let finalize_result = finalize_docs(&built_docs, false);
        match &finalize_result {
            Ok(res) => {
                // 2026-05-29 DEBUG: surface per-doc HEADER_* diagnostics so
                // the FHNW header injection lifecycle is visible. Remove
                // when item D1 is resolved.
                for (p, r) in res {
                    if r.contains("HEADER_") {
                        eprintln!("[finalize-debug] {p}\t{r}");
                    }
                }
                let failed: Vec<_> = res.iter().filter(|(_, r)| r.starts_with("ERROR")).collect();
                if !json_out {
                    println!(
                        "Finalized {}/{} book(s) via Microsoft Word.",
                        res.len() - failed.len(),
                        res.len()
                    );
                }
                for (p, r) in failed {
                    eprintln!(
                        "WARNING: Word finalize failed for {p}: {r} — this book will need a field refresh on open."
                    );
                }
            }
            Err(e) => eprintln!(
                "WARNING: prerequisite not met — Microsoft Word unavailable, so the {} \
                 rendered book(s) were NOT finalized and WILL prompt to update fields \
                 (TOC / lists / index) on open. Finalize on Windows with Word installed \
                 (e.g. `agentic book finalize --path {}`). Cause: {e}",
                built_docs.len(),
                out.display()
            ),
        }
        // Round D-C (AI-Norms parity, 2026-06-03), hoisted out of the
        // match arm by Round V iter-5: Word COM `Documents.Open → … →
        // Save` regenerates the default/even/firstPage header & footer
        // triad even when only one part is non-empty — the W9-B
        // render-time collapse pass is silently undone. Round V zone B
        // additionally re-injects the Office-2010 reference theme1.xml
        // and the 186-style styles.xml when the book opted into
        // `body_render_use_bk_styles`. Either way, run the pass on
        // every built doc so the parity-gate HEADER_PART_COUNT,
        // FOOTER_PART_COUNT, THEME::majorFont, THEME::minorFont, and
        // bk-style-count assertions hold against the saved bytes —
        // INCLUDING the case where Word COM crashed and never wrote a
        // single field update.
        // Round V iter-9: pass `true` UNCONDITIONALLY (was `&use_bk`).
        // See the iter-9 comment block above the `finalize_docs` call for
        // the rationale. `built_use_bk` is retained because it positionally
        // ties to `built_docs` for the size-ascending sort step earlier.
        for path in &built_docs {
            if let Err(e) = post_finalize_collapse(path, true) {
                eprintln!(
                    "WARNING: post-finalize header/footer collapse failed for {}: {e}",
                    path.display()
                );
            }
        }
        // Round V iter-9: consume `built_use_bk` so the compiler doesn't
        // emit an `unused variable` warning. The vector is retained for
        // its sort-key role; the per-book flag is no longer a gating input.
        let _ = built_use_bk;
    }
    if json_out {
        println!(
            "{}",
            serde_json::json!({ "built": built, "out": out.display().to_string() })
        );
    } else {
        println!(
            "Built {built} book(s) into {} (no intermediates left)",
            out.display()
        );
    }
    Ok(())
}

fn build_one(
    conn: &rusqlite::Connection,
    project: &str,
    spec: &BookSpec,
    work: &Path,
    out: &Path,
    lang: &str,
    excluded: &std::collections::HashSet<String>,
) -> Result<(usize, u64, Vec<String>)> {
    // Pre-render the three admonition icons (gen_icons port) into the work dir
    // so the book renderer can embed icon_{tip,note,warning}.png in callouts.
    for kind in ["tip", "note", "warning"] {
        let json = format!(
            "{{\"id\":\"icon_{kind}\",\"type\":\"icon\",\"data\":{{\"variant\":\"{kind}\"}}}}"
        );
        let _ = agentic_figures::render_figspec(&json, &work.join(format!("icon_{kind}.png")));
    }
    // Materialise content-image assets (if any) into the scratch dir, by
    // basename, so `![caption](name.png)` references resolve at render time.
    if let Some(prefix) = &spec.assets {
        if let Ok(items) = worktree::list(conn, project, prefix) {
            for (path, _sha) in items {
                if let Ok(blob) = worktree::read_at(conn, project, &path) {
                    let base = path.rsplit('/').next().unwrap_or(&path);
                    let _ = std::fs::write(work.join(base), &blob.content);
                }
            }
        }
    }
    let mut chapters: Vec<(String, String)> = Vec::new();
    let mut figs = 0usize;
    let mut held: Vec<String> = Vec::new();
    // Round-F (AI-Norms parity, 2026-06-03): build the set of slugs that have
    // an extras sidecar registered as a chapter. When the same slug is also
    // present as a main `out/sources/norms/<NN>_<slug>_EN.md` chapter, the
    // keypoints + quiz fences in the MAIN md are duplicates of the sidecar's
    // canonical `chapter_extras` blocks (Wave-1/F5 ingested the sidecars as
    // the authoritative source). Without dedupe, each covered chapter emits
    // the keypoints box and quiz block twice, doubling
    // `BkCallout`/`BkBullet`/"Key topics at a glance"/"Review questions"
    // counts in the parity gate.
    let sidecar_slugs: std::collections::HashSet<String> = spec
        .chapters
        .iter()
        .filter_map(|p| p.strip_prefix("out/sources/ai_norms/"))
        .filter_map(|tail| tail.strip_suffix(".extras.md"))
        .map(|s| s.to_string())
        .collect();
    for ch in &spec.chapters {
        // ADR-0049 ph3 — skip chapters held out by a model_review "exclude" verdict.
        if excluded.contains(ch) {
            held.push(ch.clone());
            eprintln!("    ~ held by review: {ch}");
            continue;
        }
        let Ok(blob) = worktree::read_at(conn, project, ch) else {
            eprintln!("    ! missing chapter {ch}");
            continue;
        };
        let raw = String::from_utf8_lossy(&blob.content).to_string();
        // Wave-9 polish (AI-Norms parity, 2026-06-03): the per-chapter
        // `*.extras.md` sidecars carry both the keypoints/quiz blocks AND
        // (since Wave 5) a `## Figures` section with `image-embed` figspecs
        // pointing at the same raster the main chapter md already references
        // via `![](name.png)`. Without stripping that section we render every
        // sidecar figure a second time, doubling the figure count (530+ vs
        // the reference book's 133). Drop the marker-fenced section so the
        // sidecar contributes only its chapter_extras chrome.
        let mut md = strip_wave5_figures_section(&raw);
        // Round-F (AI-Norms parity, 2026-06-03): dedupe keypoints + quiz when
        // a sidecar for the same slug is also in the chapter list.
        if let Some(slug) = norms_chapter_slug(ch) {
            if sidecar_slugs.contains(&slug) {
                md = strip_keypoints_and_quiz_fences(&md);
            }
        }
        let subdir = sanitize(ch.rsplit('/').next().unwrap_or(ch));
        // Surface (don't swallow) a figure-resolution failure: a dropped figspec
        // must not vanish silently from a deliverable (non-repudiation).
        let (resolved, n) = agentic_figures::resolve_markdown(&md, work, &subdir)
            .with_context(|| format!("resolving figspecs in chapter {ch}"))?;
        figs += n;
        chapters.push((ch.clone(), resolved));
    }
    // ADR-0050: parse the manifest's typography / caption-format strings into
    // the typed enums; unknown values fall back to defaults (zero regression
    // for every manifest authored before v0.1.13).
    let thesis_typography = match spec.thesis_typography.as_deref() {
        Some("fhnw-proposal-parity") => agentic_export::book::TypographyProfile::FhnwProposalParity,
        Some("fhnw-mt-template") => agentic_export::book::TypographyProfile::FhnwMtTemplate,
        _ => agentic_export::book::TypographyProfile::Designer,
    };
    let caption_format = match spec.caption_format.as_deref() {
        Some("colon") => agentic_export::book::CaptionFormat::Colon,
        _ => agentic_export::book::CaptionFormat::Period,
    };
    // ADR-0050 §1 item 1: load the FHNW running-header logo bytes from the
    // project DB when the manifest specifies one. Failures are non-fatal
    // (the engine falls back to no header); we log a context-rich warning.
    let header_logo: Option<Vec<u8>> = if let Some(path) = spec.header_logo.as_deref() {
        match agentic_core::worktree::read_at(conn, project, path) {
            Ok(blob) => Some(blob.content),
            Err(e) => {
                eprintln!("warn: header_logo {path} not loadable ({e}) — rendering without logo");
                None
            }
        }
    } else {
        None
    };
    // Wave 9 (AI-Norms parity, 2026-06-03): when the manifest references a
    // TOML allowlist (e.g. `specs/books/ai_norms/index_allowlist.toml`), read
    // it from the project DB and merge its terms into `index_terms` BEFORE
    // the engine harvests chapter prose. Without this load, only the inline
    // `index_terms` array is forwarded to the engine — which is why the
    // ai_norms book rendered a 32-entry index (explicit `{{index:…}}`
    // markers only) instead of the 113-entry reference target.
    let mut effective_index_terms = spec.index_terms.clone();
    if let Some(path) = spec.index_allowlist_path.as_deref() {
        match agentic_core::worktree::read_at(conn, project, path) {
            Ok(blob) => {
                let src = String::from_utf8_lossy(&blob.content).to_string();
                match agentic_export::index::IndexAllowlist::parse_toml(&src) {
                    Ok(al) => {
                        eprintln!(
                            "    + index allowlist {path}: {} terms merged",
                            al.terms.len()
                        );
                        effective_index_terms.extend(al.terms);
                    }
                    Err(e) => eprintln!(
                        "warn: index_allowlist_path {path} parse error ({e}) — index falls back to inline `index_terms`"
                    ),
                }
            }
            Err(e) => eprintln!(
                "warn: index_allowlist_path {path} not loadable ({e}) — index falls back to inline `index_terms`"
            ),
        }
    }
    let meta = BookMeta {
        title: spec.title.clone(),
        subtitle: spec.subtitle.clone(),
        author: spec
            .author
            .clone()
            .unwrap_or_else(|| "Daniel Casota".into()),
        context: spec
            .context
            .clone()
            .unwrap_or_else(|| "MAS Cybersecurity, IWI, FHNW — May 2026".into()),
        description: spec.description.clone(),
        dedication: spec.dedication.clone(),
        epigraph: spec.epigraph.clone(),
        epigraph_by: spec.epigraph_by.clone(),
        disclaimer: spec.disclaimer.clone(),
        imprint: spec.imprint.clone(),
        thesis_profile: spec.thesis_profile,
        companion: spec.companion,
        index_terms: effective_index_terms,
        // Chrome language: per-book `lang` wins; else the global `--lang`; else
        // "en". The i18n layer normalises/falls back, but resolve a non-empty
        // default here so an empty global value still renders English.
        lang: spec.lang.clone().unwrap_or_else(|| {
            if lang.is_empty() {
                "en".to_string()
            } else {
                lang.to_string()
            }
        }),
        thesis_typography,
        // ADR-0064 iter15 (2026-07-03): FhnwMtTemplate implies Roman
        // front-matter → Arabic main-matter → Roman continuation in
        // back-matter (FHNW academic convention, ADR-0004). Other
        // profiles keep the historical Arabic-throughout default.
        page_numbering: if matches!(
            thesis_typography,
            agentic_export::book::TypographyProfile::FhnwMtTemplate
        ) {
            agentic_export::book::PageNumbering::FhnwRomanThenArabic
        } else {
            agentic_export::book::PageNumbering::default()
        },
        caption_format,
        header_logo,
        header_lines: spec.header_lines.clone(),
        // T1.7 (REF parity 2026-06-02): forward the three new chrome-block
        // manifest fields straight to the engine. `None` is the historical
        // default, so books that don't declare these stay unaffected.
        dedication_page: spec.dedication_page.clone(),
        antikythera_note: spec.antikythera_note.clone(),
        qrlink: spec.qrlink.clone(),
        body_render_use_bk_styles: spec.body_render_use_bk_styles,
        // Wave-4 AI-Norms parity (ADR-0054 v1, 2026-06-03): forward the
        // layout / chrome override fields to the engine. Each is a no-op
        // when unset, so books that don't declare them stay unaffected.
        header_distance_twips: spec.header_distance_twips,
        footer_distance_twips: spec.footer_distance_twips,
        cols_space_twips: spec.cols_space_twips,
        doc_grid_line_pitch: spec.doc_grid_line_pitch,
        dedication_personal: spec.dedication_personal.clone(),
        closing_thought: spec.closing_thought.clone(),
        byline_institution_full: spec.byline_institution_full.clone(),
        tof_heading: spec.tof_heading.clone(),
        tot_heading: spec.tot_heading.clone(),
        inscription_page_enabled: spec.inscription_page_enabled,
        // Wave-2 (bookkit chrome suppression, 2026-06-04): forward each
        // flag straight to the engine. Defaults are `true` via
        // `default_true()`, so books that don't declare these stay
        // identical to the historical Designer output.
        emit_index: spec.emit_index,
        emit_appendix_in_back_matter: spec.emit_appendix_in_back_matter,
        emit_per_chapter_sources_box: spec.emit_per_chapter_sources_box,
        emit_chapter_dividers: spec.emit_chapter_dividers,
        // Wave-3 iter-D (2026-06-04): forward the two new
        // structural-parity flags. Defaults preserve historical
        // behaviour for every existing book.
        emit_per_chapter_sectpr: spec.emit_per_chapter_sectpr,
        emit_chapter_extras: spec.emit_chapter_extras,
    };
    let bytes = render_book(&meta, &chapters, work)?;
    let path = out.join(format!("{}.docx", spec.key));
    std::fs::write(&path, &bytes).with_context(|| format!("writing {}", path.display()))?;

    // ADR-0050 §1 item 1 (v0.1.15-engine, 2026-05-29 fix): the engine
    // can't reliably produce a Word-acceptable header drawing via
    // docx-rs's `Pic` — Word's parser silently discards it on open.
    // Write a sidecar JSON next to the docx; the finalize step injects
    // the header via Word's own API. The logo bytes are materialised to
    // a sibling PNG so finalize doesn't need the project DB.
    if agentic_export::book::fhnw_header_sidecar_needed(&meta) {
        let logo_path_abs: Option<String> = if let Some(bytes) = meta.header_logo.as_ref() {
            let logo_path = out.join(format!("{}.fhnw_logo.png", spec.key));
            std::fs::write(&logo_path, bytes)
                .with_context(|| format!("writing FHNW logo to {}", logo_path.display()))?;
            // Use the absolute path so the PowerShell finalize step is
            // independent of the working directory.
            std::fs::canonicalize(&logo_path)
                .ok()
                .map(|p| p.to_string_lossy().replace(r"\\?\", ""))
        } else {
            None
        };
        let sidecar =
            agentic_export::book::FhnwHeaderSidecar::from_meta(&meta, logo_path_abs.clone());
        let sidecar_path = out.join(format!("{}.docx.fhnw_header.json", spec.key));
        let json = serde_json::to_string_pretty(&sidecar)
            .with_context(|| "serialising FHNW header sidecar")?;
        std::fs::write(&sidecar_path, json).with_context(|| {
            format!("writing FHNW header sidecar to {}", sidecar_path.display())
        })?;
    }

    Ok((figs, bytes.len() as u64, held))
}

// ---- render-quality audit (compare current vs previous iteration) ----
struct DocxFacts {
    media: usize,
    has_heading_styles: bool,
    has_page_size: bool,
    /// Count of the figure-spacing sentinel (ADR-0030 relaxed placement around
    /// figures: `w:after="220"`). Each figure should contribute at least one.
    fig_spacers: usize,
    bytes: u64,
}

fn inspect_docx(path: &Path) -> Result<DocxFacts> {
    let file = std::fs::File::open(path)?;
    let bytes = file.metadata()?.len();
    let mut zip = zip::ZipArchive::new(file)?;
    let mut media = 0usize;
    let mut styles = String::new();
    let mut document = String::new();
    for i in 0..zip.len() {
        let mut f = zip.by_index(i)?;
        let name = f.name().to_string();
        if name.starts_with("word/media/") {
            media += 1;
        } else if name == "word/styles.xml" {
            f.read_to_string(&mut styles).ok();
        } else if name == "word/document.xml" {
            f.read_to_string(&mut document).ok();
        }
    }
    Ok(DocxFacts {
        media,
        has_heading_styles: styles.contains("w:styleId=\"Heading1\""),
        has_page_size: document.contains("w:pgSz"),
        fig_spacers: document.matches("w:after=\"220\"").count(),
        bytes,
    })
}

fn audit(current: &Path, previous: Option<&Path>, json_out: bool) -> Result<()> {
    let books: Vec<PathBuf> = std::fs::read_dir(current)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "docx"))
        .collect();
    let mut fail = false;
    let mut rows = Vec::new();
    println!(
        "{:<28} {:>6} {:>5} {:>5} {:>5} {:>9}  {}",
        "BOOK", "FIGS", "HEAD", "PAGE", "SPC", "KB", "VS PREVIOUS"
    );
    for b in &books {
        let name = b.file_name().unwrap().to_string_lossy().to_string();
        let f = match inspect_docx(b) {
            Ok(f) => f,
            Err(e) => {
                println!("{name:<28}  inspect error: {e}");
                fail = true;
                continue;
            }
        };
        // Intra-book quality invariants.
        let mut notes = Vec::new();
        if !f.has_heading_styles {
            notes.push("NO Heading styles (TOC broken)".to_string());
            fail = true;
        }
        if !f.has_page_size {
            notes.push("no page size".to_string());
            fail = true;
        }
        // ADR-0030 relaxed placement: every figure must carry breathing-room
        // spacing. A book with figures but no figure-spacing sentinels means the
        // figures are cramped against surrounding text (the defect this gate now
        // enforces — previously unaudited).
        let figs_spaced = f.media == 0 || f.fig_spacers >= f.media;
        if !figs_spaced {
            notes.push(format!(
                "figures lack relaxed spacing (ADR-0030): {} figs, {} spacers",
                f.media, f.fig_spacers
            ));
            fail = true;
        }
        // Cross-iteration comparison.
        if let Some(prev) = previous {
            let pp = prev.join(&name);
            if pp.exists() {
                if let Ok(pf) = inspect_docx(&pp) {
                    if f.media + 1 < pf.media {
                        notes.push(format!("FIGURES regressed {}->{}", pf.media, f.media));
                        fail = true;
                    }
                    if f.bytes * 2 < pf.bytes {
                        notes.push(format!(
                            "size collapsed {}KB->{}KB",
                            pf.bytes / 1024,
                            f.bytes / 1024
                        ));
                        fail = true;
                    }
                    if !pf.has_heading_styles && f.has_heading_styles {
                        notes.push("heading styles RESTORED vs previous".to_string());
                    }
                }
            } else {
                notes.push("new (no previous)".to_string());
            }
        }
        println!(
            "{:<28} {:>6} {:>5} {:>5} {:>5} {:>9}  {}",
            name,
            f.media,
            if f.has_heading_styles { "yes" } else { "NO" },
            if f.has_page_size { "yes" } else { "NO" },
            if figs_spaced { "yes" } else { "NO" },
            f.bytes / 1024,
            notes.join("; ")
        );
        rows.push(serde_json::json!({ "book": name, "figures": f.media, "heading_styles": f.has_heading_styles, "figs_spaced": figs_spaced, "kb": f.bytes/1024, "notes": notes }));
    }
    if json_out {
        println!(
            "{}",
            serde_json::json!({ "books": rows, "verdict": if fail {"FAIL"} else {"PASS"} })
        );
    } else {
        println!(
            "--- render audit: {} ---",
            if fail {
                "FAIL (regressions found)"
            } else {
                "PASS"
            }
        );
    }
    if fail {
        std::process::exit(1);
    }
    Ok(())
}
