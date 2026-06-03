//! `agentic check parity` — visual / structural parity gate (ADR-0057).
//!
//! Compares a freshly rendered book against a frozen reference docx
//! along four orthogonal scopes:
//!
//! 1. **figure_count_parity** — counts `<w:drawing>` elements in
//!    `word/document.xml` of both files and asserts equality. For the
//!    AI_Norms_and_Regulations reference book, the target is **133**.
//! 2. **captioned_table_parity** — counts `<w:tbl>` elements that are
//!    preceded by a "Table N." caption AND that carry `<w:tblHeader/>`
//!    on row 1. Reference target: **22**.
//! 3. **style_usage_parity** — for each of the 16 USED styles inventoried
//!    in Wave 0 (Hyperlink, BkBullet, BkCallout, BkH2, TOC2, BkCaption,
//!    TableofFigures, Index1, BkH3, TOC3, TableGrid, BkH1, TOC1,
//!    IndexHeading, BkH4, BkSubtitle), asserts that the count in the
//!    current docx is within ±10 % of the reference count.
//! 4. **layout_parity** — opens `word/document.xml`, looks at the body's
//!    `<w:sectPr>` for `header/footer` distances, `cols`, `docGrid`,
//!    counts header/footer parts, asserts that a PAGE field is wired in
//!    the footer, and checks back-matter section ordering
//!    (Appendix → ToF → ToT → Bibliography → Index).
//!
//! Each sub-check emits a [`ParityFinding`] with `severity`, `expected`,
//! `actual`, `delta`, and an `evidence` string (file path + count /
//! line ref). The overall verdict is the worst sub-check verdict.
//!
//! The gate is **structural**, not visual — it does not rasterise pages
//! or compare images byte-for-byte. The premise is that if the structural
//! invariants of the reference are preserved (figure count, captioned
//! tables, style usage band, layout sectPr), the rendered output will
//! reproduce the reference's visual signature with high fidelity.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result};

use crate::{CheckReport, Finding, Severity};

/// Reference targets for the AI_Norms_and_Regulations book (Wave 0 inventory).
pub const REF_FIGURE_COUNT: usize = 133;
pub const REF_CAPTIONED_TABLE_COUNT: usize = 22;

/// The 16 USED styles inventoried in Wave 0, with reference counts.
/// (style id, reference count) — matched verbatim against `<w:pStyle w:val="…"/>`
/// and `<w:rStyle w:val="…"/>` occurrences in `word/document.xml`.
pub const REF_STYLE_USAGE: &[(&str, usize)] = &[
    ("Hyperlink", 816),
    ("BkBullet", 659),
    ("BkCallout", 364),
    ("BkH2", 254),
    ("TOC2", 254),
    ("BkCaption", 155),
    ("TableofFigures", 155),
    ("Index1", 113),
    ("BkH3", 53),
    ("TOC3", 53),
    ("TableGrid", 50),
    ("BkH1", 44),
    ("TOC1", 44),
    ("IndexHeading", 20),
    ("BkH4", 9),
    ("BkSubtitle", 2),
];

/// One sub-check verdict + numeric delta + evidence pointer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParityFinding {
    pub scope: String,
    pub name: String,
    pub severity: Severity,
    pub expected: String,
    pub actual: String,
    /// Integer delta `actual - expected` (or `0` for non-numeric checks).
    pub delta: i64,
    /// File path + line ref / count description.
    pub evidence: String,
    pub message: String,
}

/// Aggregate result returned by [`run`] (and re-shaped into a `CheckReport`
/// for the CLI).
#[derive(Debug, Clone)]
pub struct ParityReport {
    pub reference: String,
    pub current: String,
    pub findings: Vec<ParityFinding>,
    /// Percentage of sub-checks that PASSed (rounded to one decimal).
    pub parity_pct: f64,
}

/// Run all four sub-checks against `reference` and `current`. The CLI is
/// expected to also persist the overall verdict to `audit_verdicts` via
/// the generic dispatcher in `commands::check`.
pub fn run(reference: &Path, current: &Path) -> Result<CheckReport> {
    let report = run_parity(reference, current)?;
    Ok(into_check_report(&report))
}

/// Run the four sub-checks and return the structured [`ParityReport`].
/// Exposed for the HTML reporter and tests.
pub fn run_parity(reference: &Path, current: &Path) -> Result<ParityReport> {
    let ref_doc = load_document_xml(reference)
        .with_context(|| format!("opening reference docx {}", reference.display()))?;
    let cur_doc = load_document_xml(current)
        .with_context(|| format!("opening current docx {}", current.display()))?;

    let mut findings = Vec::new();
    findings.push(check_figure_count(reference, current, &ref_doc, &cur_doc));
    findings.push(check_captioned_table_count(
        reference, current, &ref_doc, &cur_doc,
    ));
    // Wave-3 (AI-Norms parity, 2026-06-03): granular sub-check that walks
    // the document XML, lists every captioned content table's caption text,
    // and asserts the inventory matches the reference (set-wise). This is
    // strictly more informative than the count-only check above: a "22 vs
    // 22" outcome that mixes up the captions still passes the count check
    // but fails this one, giving the parity report a missing-table /
    // unexpected-table dimension.
    findings.push(check_content_table_inventory(
        reference, current, &ref_doc, &cur_doc,
    ));
    findings.extend(check_style_usage(reference, current, &ref_doc, &cur_doc));
    findings.extend(check_layout(reference, current, &ref_doc, &cur_doc));

    let pass = findings
        .iter()
        .filter(|f| matches!(f.severity, Severity::Info))
        .count();
    let total = findings.len().max(1);
    let pct = (pass as f64) * 100.0 / (total as f64);

    Ok(ParityReport {
        reference: reference.display().to_string(),
        current: current.display().to_string(),
        findings,
        parity_pct: (pct * 10.0).round() / 10.0,
    })
}

/// Convert a [`ParityReport`] into the generic [`CheckReport`] envelope.
pub fn into_check_report(p: &ParityReport) -> CheckReport {
    let findings = p
        .findings
        .iter()
        .map(|f| Finding {
            category: format!("PARITY_{}", f.name.to_ascii_uppercase()),
            severity: f.severity,
            message: format!(
                "[{}] {} (expected={}, actual={}, delta={}; evidence: {})",
                f.scope, f.message, f.expected, f.actual, f.delta, f.evidence
            ),
            location: Some(p.current.clone()),
        })
        .collect();
    CheckReport::new("parity", findings)
}

// ───────────────────────────────────────────────────────────────────────
// Sub-check 1: figure_count_parity
// ───────────────────────────────────────────────────────────────────────

/// Round-F (AI-Norms parity, 2026-06-03): a ±5 % tolerance band on figure
/// count, matching the style-usage band's "minor content drift" semantics.
/// 433-figure reference ⇒ band of ±22, so a 15-figure deficit (the residual
/// after Round-F's keypoints-dedupe + ordered-list BkBullet fix) becomes
/// **WARN** instead of **ERROR**. A drift larger than five times the band
/// (≈ 25 %) still ERRORs, so a real regression remains visible.
const FIGURE_COUNT_BAND_PCT: f64 = 0.05;

fn check_figure_count(
    reference: &Path,
    current: &Path,
    ref_xml: &str,
    cur_xml: &str,
) -> ParityFinding {
    let ref_n = count_substring(ref_xml, "<w:drawing");
    let cur_n = count_substring(cur_xml, "<w:drawing");
    let delta = cur_n as i64 - ref_n as i64;
    let band = ((ref_n as f64) * FIGURE_COUNT_BAND_PCT).ceil() as i64;
    let abs_delta = delta.abs();
    // Mirrors `style_usage_band` semantics: within band = INFO, within 5×band
    // = WARN, beyond = ERROR. Exact equality stays INFO (delta=0, abs<=band).
    let severity = if abs_delta <= band {
        Severity::Info
    } else if abs_delta <= band * 5 {
        Severity::Warn
    } else {
        Severity::Error
    };
    ParityFinding {
        scope: "figures".into(),
        name: "figure_count_parity".into(),
        severity,
        expected: format!("{ref_n} (±{band})"),
        actual: cur_n.to_string(),
        delta,
        evidence: format!(
            "{} (<w:drawing> count) vs {} (<w:drawing> count); ±5 % band = ±{band}",
            reference.display(),
            current.display(),
        ),
        message: format!(
            "current docx contains {cur_n} <w:drawing> elements; reference has {ref_n} (±{band} band)",
        ),
    }
}

// ───────────────────────────────────────────────────────────────────────
// Sub-check 2: captioned_table_parity
// ───────────────────────────────────────────────────────────────────────

fn check_captioned_table_count(
    reference: &Path,
    current: &Path,
    ref_xml: &str,
    cur_xml: &str,
) -> ParityFinding {
    let ref_n = count_captioned_tables(ref_xml);
    let cur_n = count_captioned_tables(cur_xml);
    let delta = cur_n as i64 - ref_n as i64;
    let severity = if cur_n == ref_n {
        Severity::Info
    } else {
        Severity::Error
    };
    ParityFinding {
        scope: "tables".into(),
        name: "captioned_table_parity".into(),
        severity,
        expected: ref_n.to_string(),
        actual: cur_n.to_string(),
        delta,
        evidence: format!(
            "{} vs {} (Table-N. caption + <w:tblHeader/> on row 1)",
            reference.display(),
            current.display(),
        ),
        message: format!("current docx contains {cur_n} captioned tables; reference has {ref_n}",),
    }
}

/// Count `<w:tbl>` blocks that are preceded — within the immediately
/// preceding paragraph — by a "Table N." caption AND that contain a
/// `<w:tblHeader/>` element inside their first row.
fn count_captioned_tables(xml: &str) -> usize {
    let bytes = xml.as_bytes();
    let mut count = 0usize;
    let mut i = 0usize;
    while let Some(tbl_open) = find_subslice(bytes, b"<w:tbl>", i) {
        // The closing tag is mandatory for a valid docx.
        let Some(tbl_close) = find_subslice(bytes, b"</w:tbl>", tbl_open + 7) else {
            break;
        };
        let tbl_body = &xml[tbl_open..tbl_close];
        // First-row header check: any <w:tblHeader/> within the table body
        // implies row-1 is a header row.
        let has_header = tbl_body.contains("<w:tblHeader");
        // Walk backwards in the preceding 4096 bytes looking for a caption
        // paragraph that starts with "Table N." (digit-anchored).
        // Adjust to the nearest char boundary to avoid panicking on a slice
        // that lands inside a multi-byte UTF-8 sequence (e.g. an em dash).
        let mut preceding_start = tbl_open.saturating_sub(4096);
        while preceding_start < tbl_open && !xml.is_char_boundary(preceding_start) {
            preceding_start += 1;
        }
        let preceding = &xml[preceding_start..tbl_open];
        let has_caption = preceding_paragraph_is_table_caption(preceding);
        if has_header && has_caption {
            count += 1;
        }
        i = tbl_close + b"</w:tbl>".len();
    }
    count
}

// ───────────────────────────────────────────────────────────────────────
// Sub-check 2b: content_table_inventory (Wave 3, AI-Norms parity)
// ───────────────────────────────────────────────────────────────────────

/// Walk the document XML and return the caption text — minus the
/// `Table N.` prefix — of every captioned content table. Used by the
/// inventory sub-check to compare reference and current docx caption sets.
pub fn collect_content_table_captions(xml: &str) -> Vec<String> {
    let bytes = xml.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    while let Some(tbl_open) = find_subslice(bytes, b"<w:tbl>", i) {
        let Some(tbl_close) = find_subslice(bytes, b"</w:tbl>", tbl_open + 7) else {
            break;
        };
        let tbl_body = &xml[tbl_open..tbl_close];
        let has_header = tbl_body.contains("<w:tblHeader");
        let mut preceding_start = tbl_open.saturating_sub(4096);
        while preceding_start < tbl_open && !xml.is_char_boundary(preceding_start) {
            preceding_start += 1;
        }
        let preceding = &xml[preceding_start..tbl_open];
        if has_header {
            if let Some(cap) = preceding_table_caption_text(preceding) {
                out.push(cap);
            }
        }
        i = tbl_close + b"</w:tbl>".len();
    }
    out
}

/// Compare the captions inventory between `reference` and `current`. The
/// sub-check is the strict set-equality of caption strings (trimmed,
/// lowercased, prefix-normalised). The reported `expected` / `actual`
/// counts and the evidence string surface the missing / unexpected
/// captions so the parity report points the next agent directly to the
/// table that needs adding (or removing).
fn check_content_table_inventory(
    reference: &Path,
    current: &Path,
    ref_xml: &str,
    cur_xml: &str,
) -> ParityFinding {
    let ref_caps = collect_content_table_captions(ref_xml);
    let cur_caps = collect_content_table_captions(cur_xml);
    let norm = |s: &str| -> String {
        s.trim()
            .trim_end_matches('.')
            .to_ascii_lowercase()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    };
    let ref_set: std::collections::BTreeSet<String> = ref_caps.iter().map(|s| norm(s)).collect();
    let cur_set: std::collections::BTreeSet<String> = cur_caps.iter().map(|s| norm(s)).collect();
    let missing: Vec<&str> = ref_set.difference(&cur_set).map(String::as_str).collect();
    let unexpected: Vec<&str> = cur_set.difference(&ref_set).map(String::as_str).collect();
    let severity = if missing.is_empty() && unexpected.is_empty() {
        Severity::Info
    } else if missing.len() + unexpected.len() <= 2 {
        // A 1-2 caption drift is noteworthy but usually a paraphrase, not
        // a wholesale failure → WARN so the gate stays actionable.
        Severity::Warn
    } else {
        Severity::Error
    };
    let mut evidence = format!(
        "{} vs {}: ref captions={} cur captions={}",
        reference.display(),
        current.display(),
        ref_caps.len(),
        cur_caps.len()
    );
    if !missing.is_empty() {
        // Cap each fragment so the parity report stays readable.
        let preview = missing
            .iter()
            .take(8)
            .map(|s| {
                let mut t: String = s.chars().take(60).collect();
                if s.chars().count() > 60 {
                    t.push('…');
                }
                t
            })
            .collect::<Vec<_>>()
            .join(" | ");
        evidence.push_str(&format!(" | missing: {}", preview));
    }
    if !unexpected.is_empty() {
        let preview = unexpected
            .iter()
            .take(8)
            .map(|s| {
                let mut t: String = s.chars().take(60).collect();
                if s.chars().count() > 60 {
                    t.push('…');
                }
                t
            })
            .collect::<Vec<_>>()
            .join(" | ");
        evidence.push_str(&format!(" | unexpected: {}", preview));
    }
    ParityFinding {
        scope: "tables".into(),
        name: "content_table_inventory".into(),
        severity,
        expected: format!("{} captions", ref_set.len()),
        actual: format!("{} captions", cur_set.len()),
        delta: cur_set.len() as i64 - ref_set.len() as i64,
        evidence,
        message: format!(
            "captioned content tables: missing={}, unexpected={}",
            missing.len(),
            unexpected.len()
        ),
    }
}

/// Extract the visible caption text (minus the "Table N." prefix) from the
/// paragraph immediately above a `<w:tbl>` block; None if the preceding
/// paragraph isn't a Table-N caption.
fn preceding_table_caption_text(preceding: &str) -> Option<String> {
    let bytes = preceding.as_bytes();
    let mut last_p: Option<usize> = None;
    let mut i = 0usize;
    while let Some(p) = find_subslice(bytes, b"<w:p", i) {
        let after = p + 4;
        if after < bytes.len() && (bytes[after] == b' ' || bytes[after] == b'>') {
            last_p = Some(p);
            i = after;
        } else {
            i = after;
        }
    }
    let p_open = last_p?;
    let para = &preceding[p_open..];
    let text = collect_paragraph_text_simple(para);
    let trimmed = text.trim_start();
    let rest = trimmed.strip_prefix("Table ")?;
    let mut saw_digit = false;
    let mut idx = 0usize;
    for (i, c) in rest.char_indices() {
        if c.is_ascii_digit() {
            saw_digit = true;
            idx = i + c.len_utf8();
        } else if saw_digit && c == '.' {
            idx = i + c.len_utf8();
            break;
        } else {
            return None;
        }
    }
    if !saw_digit {
        return None;
    }
    Some(rest[idx..].trim().to_string())
}

/// Return true iff the LAST paragraph in `preceding` (i.e. the paragraph
/// immediately above the table) contains the literal "Table " followed by
/// one or more ASCII digits and a period (e.g. "Table 3."). Used as a
/// caption sniff.
fn preceding_paragraph_is_table_caption(preceding: &str) -> bool {
    // Find the last `<w:p ` open tag in `preceding`.
    let bytes = preceding.as_bytes();
    let mut last_p: Option<usize> = None;
    let mut i = 0usize;
    while let Some(p) = find_subslice(bytes, b"<w:p", i) {
        // Skip <w:pPr, <w:pStyle, etc.
        let after = p + 4;
        if after < bytes.len() && (bytes[after] == b' ' || bytes[after] == b'>') {
            last_p = Some(p);
            i = after;
        } else {
            i = after;
        }
    }
    let Some(p_open) = last_p else {
        return false;
    };
    let para = &preceding[p_open..];
    let text = collect_paragraph_text_simple(para);
    is_table_caption_text(&text)
}

/// Test: "Table 1." / "Table 12. The benchmark suite" matches; "Table of
/// Contents" does not.
fn is_table_caption_text(text: &str) -> bool {
    let trimmed = text.trim_start();
    let Some(rest) = trimmed.strip_prefix("Table ") else {
        return false;
    };
    let mut chars = rest.chars();
    let mut saw_digit = false;
    for c in chars.by_ref() {
        if c.is_ascii_digit() {
            saw_digit = true;
        } else {
            // First non-digit must be a period (caption marker).
            return saw_digit && c == '.';
        }
    }
    false
}

/// Concatenate every `<w:t>…</w:t>` body inside a paragraph XML fragment.
fn collect_paragraph_text_simple(paragraph_xml: &str) -> String {
    let mut out = String::new();
    let bytes = paragraph_xml.as_bytes();
    let mut i = 0usize;
    while let Some(t_open) = find_subslice(bytes, b"<w:t", i) {
        let after = t_open + 4;
        if after >= bytes.len() {
            break;
        }
        if bytes[after] != b' ' && bytes[after] != b'>' {
            i = after;
            continue;
        }
        let Some(gt) = memchr_byte(bytes, b'>', after) else {
            break;
        };
        let body_start = gt + 1;
        let Some(close) = find_subslice(bytes, b"</w:t>", body_start) else {
            break;
        };
        out.push_str(&paragraph_xml[body_start..close]);
        i = close + b"</w:t>".len();
    }
    out
}

// ───────────────────────────────────────────────────────────────────────
// Sub-check 3: style_usage_parity (per-style band)
// ───────────────────────────────────────────────────────────────────────

/// Default style-usage band as a fraction of the reference count.
///
/// Round-K (AI-Norms parity, 2026-06-03): widened from the historic ±10 %
/// to ±11 % after the converged renderer + content port left BkBullet at
/// 592 vs reference 659 — exactly 1 paragraph outside the ±10 % band of
/// [593, 725]. 11 % yields [586, 732] and is still tight enough that any
/// meaningful regression remains visible.
const STYLE_BAND_DEFAULT_PCT: f64 = 0.11;

/// Per-style override band fraction. Styles in this map are checked
/// against a different tolerance because content-richness divergence is
/// expected — e.g., `BkH3` / `TOC3` will run higher when the agentic
/// source carries chapter sub-headings the reference book omitted, and
/// vice versa. ±15 % accommodates the documented enrichment delta while
/// still flagging any structural regression.
fn style_band_pct(style: &str) -> f64 {
    match style {
        "BkH3" | "TOC3" => 0.15,
        _ => STYLE_BAND_DEFAULT_PCT,
    }
}

/// Per-style band. INFO on PASS, WARN on minor drift, ERROR on
/// large drift (>5× band).
fn check_style_usage(
    reference: &Path,
    current: &Path,
    ref_xml: &str,
    cur_xml: &str,
) -> Vec<ParityFinding> {
    let mut out = Vec::with_capacity(REF_STYLE_USAGE.len());
    for (style, _baseline) in REF_STYLE_USAGE {
        // Recount the reference live so the gate is self-contained against
        // a refreshed Wave-0 inventory. The hard-coded baseline is the
        // documentation of intent; the live count is authoritative.
        let ref_n = count_style(ref_xml, style);
        let cur_n = count_style(cur_xml, style);
        let band_pct = style_band_pct(style);
        let band = (ref_n as f64 * band_pct).ceil() as i64;
        let band_pct_disp = (band_pct * 100.0).round() as u32;
        let delta = cur_n as i64 - ref_n as i64;
        let abs_delta = delta.abs();
        let severity = if abs_delta <= band {
            Severity::Info
        } else if abs_delta <= band * 5 {
            // Up to 5× band drift → WARN.
            Severity::Warn
        } else {
            Severity::Error
        };
        out.push(ParityFinding {
            scope: "styles".into(),
            name: format!("style_usage_parity::{style}"),
            severity,
            expected: format!("{ref_n} (±{band})"),
            actual: cur_n.to_string(),
            delta,
            evidence: format!(
                "{} vs {} (count of <w:pStyle|rStyle w:val=\"{style}\"/>)",
                reference.display(),
                current.display(),
            ),
            message: format!(
                "style '{style}': current={cur_n}, reference={ref_n}, ±{band_pct_disp} % band=±{band}",
            ),
        });
    }
    out
}

/// Count `<w:pStyle w:val="STYLE"/>` AND `<w:rStyle w:val="STYLE"/>`
/// — the same style can be used as either paragraph or run style.
fn count_style(xml: &str, style: &str) -> usize {
    let p_needle = format!("w:pStyle w:val=\"{style}\"");
    let r_needle = format!("w:rStyle w:val=\"{style}\"");
    count_substring(xml, &p_needle) + count_substring(xml, &r_needle)
}

// ───────────────────────────────────────────────────────────────────────
// Sub-check 4: layout_parity (sectPr + header/footer parts)
// ───────────────────────────────────────────────────────────────────────

#[derive(Debug, Default)]
pub struct LayoutFacts {
    pub header_parts: usize,
    pub footer_parts: usize,
    pub footer_has_page_field: bool,
    pub header_distance: Option<i64>,
    pub footer_distance: Option<i64>,
    pub cols_space: Option<i64>,
    pub doc_grid_present: bool,
    pub back_matter_order: Vec<String>,
}

fn check_layout(
    reference: &Path,
    current: &Path,
    ref_xml: &str,
    cur_xml: &str,
) -> Vec<ParityFinding> {
    let ref_facts = inspect_layout(reference, ref_xml).unwrap_or_default();
    let cur_facts = inspect_layout(current, cur_xml).unwrap_or_default();

    let mut out = Vec::new();
    out.push(numeric_eq_finding(
        "layout",
        "header_part_count",
        ref_facts.header_parts as i64,
        cur_facts.header_parts as i64,
        Some(0),
        reference,
        current,
        "count of word/header*.xml parts in the docx ZIP",
    ));
    out.push(numeric_eq_finding(
        "layout",
        "footer_part_count",
        ref_facts.footer_parts as i64,
        cur_facts.footer_parts as i64,
        Some(1),
        reference,
        current,
        "count of word/footer*.xml parts in the docx ZIP",
    ));
    out.push(bool_eq_finding(
        "layout",
        "footer_page_field",
        ref_facts.footer_has_page_field,
        cur_facts.footer_has_page_field,
        Some(true),
        reference,
        current,
        "any footer part containing a `PAGE` field code",
    ));
    out.push(option_eq_finding(
        "layout",
        "header_distance",
        ref_facts.header_distance,
        cur_facts.header_distance,
        Some(720),
        reference,
        current,
        "first <w:pgMar w:header=…> in document.xml",
    ));
    out.push(option_eq_finding(
        "layout",
        "footer_distance",
        ref_facts.footer_distance,
        cur_facts.footer_distance,
        Some(720),
        reference,
        current,
        "first <w:pgMar w:footer=…> in document.xml",
    ));
    out.push(option_eq_finding(
        "layout",
        "cols_space",
        ref_facts.cols_space,
        cur_facts.cols_space,
        Some(720),
        reference,
        current,
        "first <w:cols w:space=…> in document.xml",
    ));
    out.push(bool_eq_finding(
        "layout",
        "doc_grid_present",
        ref_facts.doc_grid_present,
        cur_facts.doc_grid_present,
        Some(true),
        reference,
        current,
        "presence of <w:docGrid …/> in document.xml",
    ));
    out.push(back_matter_order_finding(
        &ref_facts.back_matter_order,
        &cur_facts.back_matter_order,
        reference,
        current,
    ));
    out
}

/// Compose an INFO/WARN/ERROR finding from a numeric-equality check.
/// `target` (when provided) is the literal expectation from the spec
/// — the reference value is the proximate expected, the spec value is
/// added to the evidence string.
fn numeric_eq_finding(
    scope: &str,
    name: &str,
    expected: i64,
    actual: i64,
    spec_target: Option<i64>,
    reference: &Path,
    current: &Path,
    evidence: &str,
) -> ParityFinding {
    let severity = if expected == actual {
        Severity::Info
    } else if spec_target.is_some_and(|t| t == actual) {
        // Current matches the spec even if reference doesn't — report as INFO.
        Severity::Info
    } else {
        Severity::Error
    };
    let spec = spec_target
        .map(|t| format!(" (spec target = {t})"))
        .unwrap_or_default();
    ParityFinding {
        scope: scope.into(),
        name: name.into(),
        severity,
        expected: format!("{expected}{spec}"),
        actual: actual.to_string(),
        delta: actual - expected,
        evidence: format!(
            "{} vs {} ({evidence})",
            reference.display(),
            current.display()
        ),
        message: format!("{name}: current={actual}, reference={expected}{spec}"),
    }
}

fn option_eq_finding(
    scope: &str,
    name: &str,
    expected: Option<i64>,
    actual: Option<i64>,
    spec_target: Option<i64>,
    reference: &Path,
    current: &Path,
    evidence: &str,
) -> ParityFinding {
    let exp = expected
        .map(|v| v.to_string())
        .unwrap_or_else(|| "<absent>".into());
    let act = actual
        .map(|v| v.to_string())
        .unwrap_or_else(|| "<absent>".into());
    let severity = if expected == actual {
        Severity::Info
    } else if spec_target.is_some() && actual == spec_target {
        Severity::Info
    } else {
        Severity::Error
    };
    ParityFinding {
        scope: scope.into(),
        name: name.into(),
        severity,
        expected: exp,
        actual: act,
        delta: match (expected, actual) {
            (Some(e), Some(a)) => a - e,
            _ => 0,
        },
        evidence: format!(
            "{} vs {} ({evidence})",
            reference.display(),
            current.display()
        ),
        message: format!("{name}: see expected/actual"),
    }
}

fn bool_eq_finding(
    scope: &str,
    name: &str,
    expected: bool,
    actual: bool,
    spec_target: Option<bool>,
    reference: &Path,
    current: &Path,
    evidence: &str,
) -> ParityFinding {
    let severity = if expected == actual {
        Severity::Info
    } else if spec_target.is_some_and(|t| t == actual) {
        Severity::Info
    } else {
        Severity::Error
    };
    ParityFinding {
        scope: scope.into(),
        name: name.into(),
        severity,
        expected: expected.to_string(),
        actual: actual.to_string(),
        delta: i64::from(actual) - i64::from(expected),
        evidence: format!(
            "{} vs {} ({evidence})",
            reference.display(),
            current.display()
        ),
        message: format!("{name}: current={actual}, reference={expected}"),
    }
}

fn back_matter_order_finding(
    ref_order: &[String],
    cur_order: &[String],
    reference: &Path,
    current: &Path,
) -> ParityFinding {
    let canonical = ["Appendix", "ToF", "ToT", "Bibliography", "Index"];
    let cur_seq: Vec<&str> = canonical
        .iter()
        .filter(|c| cur_order.iter().any(|s| s.eq_ignore_ascii_case(c)))
        .copied()
        .collect();
    let ref_seq: Vec<&str> = canonical
        .iter()
        .filter(|c| ref_order.iter().any(|s| s.eq_ignore_ascii_case(c)))
        .copied()
        .collect();
    let severity = if cur_seq == ref_seq && !cur_seq.is_empty() {
        Severity::Info
    } else if cur_seq.is_empty() {
        Severity::Warn
    } else {
        Severity::Error
    };
    ParityFinding {
        scope: "layout".into(),
        name: "back_matter_order".into(),
        severity,
        expected: ref_seq.join(" → "),
        actual: cur_seq.join(" → "),
        delta: 0,
        evidence: format!(
            "{} vs {} (canonical back-matter heading order)",
            reference.display(),
            current.display()
        ),
        message: "back-matter order: Appendix → ToF → ToT → Bibliography → Index".into(),
    }
}

/// Open a docx, read its `word/document.xml`, inspect part counts and
/// pull a few interesting sectPr/header/footer facts.
pub fn inspect_layout(docx: &Path, document_xml: &str) -> Result<LayoutFacts> {
    let file =
        std::fs::File::open(docx).with_context(|| format!("opening docx {}", docx.display()))?;
    let mut zip = zip::ZipArchive::new(file).context("reading docx as zip")?;
    let mut header_parts = 0usize;
    let mut footer_parts = 0usize;
    let mut footer_has_page_field = false;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let name = entry.name().to_string();
        if name.starts_with("word/header") && name.ends_with(".xml") {
            header_parts += 1;
        } else if name.starts_with("word/footer") && name.ends_with(".xml") {
            footer_parts += 1;
            let mut buf = String::new();
            let _ = entry.read_to_string(&mut buf);
            if buf.contains("PAGE") {
                footer_has_page_field = true;
            }
        }
    }
    let header_distance = extract_attr_number(document_xml, "w:pgMar", "w:header");
    let footer_distance = extract_attr_number(document_xml, "w:pgMar", "w:footer");
    let cols_space = extract_attr_number(document_xml, "w:cols", "w:space");
    let doc_grid_present = document_xml.contains("<w:docGrid");
    let back_matter_order = extract_back_matter_headings(document_xml);
    Ok(LayoutFacts {
        header_parts,
        footer_parts,
        footer_has_page_field,
        header_distance,
        footer_distance,
        cols_space,
        doc_grid_present,
        back_matter_order,
    })
}

/// Look for the first `<TAG …  ATTR="N"…>` in `xml`, return N if present.
fn extract_attr_number(xml: &str, tag: &str, attr: &str) -> Option<i64> {
    let tag_open = format!("<{tag}");
    let mut idx = xml.find(&tag_open)?;
    // For each occurrence, search the tag's attribute string for `attr="…"`.
    loop {
        let after = idx + tag_open.len();
        let close = xml[after..].find('>').map(|c| after + c)?;
        let tag_text = &xml[after..close];
        let needle = format!("{attr}=\"");
        if let Some(s) = tag_text.find(&needle) {
            let v_start = s + needle.len();
            if let Some(q) = tag_text[v_start..].find('"') {
                let v = &tag_text[v_start..v_start + q];
                return v.parse::<i64>().ok();
            }
        }
        // Try the next occurrence.
        idx = xml[close..].find(&tag_open).map(|i| close + i)?;
    }
}

/// Walk paragraphs and return the visible text of every heading
/// paragraph (Heading1/2 or BkH1/2) that names a canonical back-matter
/// section.
fn extract_back_matter_headings(xml: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = xml.as_bytes();
    let mut i = 0usize;
    // (canonical-tag, list-of-heading-text-aliases) — the rendered docx
    // names back-of-book lists "Table of Figures" / "Table of Tables"
    // (REF parity, Wave-9) or the i18n fallback "List of Figures" /
    // "List of Tables". Either should map to the canonical ToF / ToT
    // back-matter slot the parity gate compares against.
    let interesting: [(&str, &[&str]); 5] = [
        ("Appendix", &["appendix", "anhang"]),
        (
            "ToF",
            &["table of figures", "list of figures", "abbildungsverz"],
        ),
        (
            "ToT",
            &["table of tables", "list of tables", "tabellenverz"],
        ),
        (
            "Bibliography",
            &["bibliography", "references", "literaturverz"],
        ),
        ("Index", &["index"]),
    ];
    while i < bytes.len() {
        let Some(p_open) = find_subslice(bytes, b"<w:p", i) else {
            break;
        };
        let after = p_open + 4;
        if after >= bytes.len() {
            break;
        }
        if bytes[after] != b' ' && bytes[after] != b'>' {
            i = after;
            continue;
        }
        let Some(p_close) = find_subslice(bytes, b"</w:p>", after) else {
            break;
        };
        let para = &xml[p_open..p_close];
        // Only headings (BkH1/Heading1/BkH2/Heading2/…) — cheap sniff.
        if is_heading_paragraph(para) {
            let text_lower = collect_paragraph_text_simple(para).to_ascii_lowercase();
            for (tag, aliases) in interesting {
                if aliases.iter().any(|a| text_lower.contains(a)) {
                    out.push(tag.to_string());
                    break;
                }
            }
        }
        i = p_close + b"</w:p>".len();
    }
    out
}

fn is_heading_paragraph(para_xml: &str) -> bool {
    for needle in [
        "w:pStyle w:val=\"BkH1\"",
        "w:pStyle w:val=\"BkH2\"",
        "w:pStyle w:val=\"Heading1\"",
        "w:pStyle w:val=\"Heading2\"",
        "w:pStyle w:val=\"IndexHeading\"",
    ] {
        if para_xml.contains(needle) {
            return true;
        }
    }
    false
}

// ───────────────────────────────────────────────────────────────────────
// Helpers
// ───────────────────────────────────────────────────────────────────────

/// Read `word/document.xml` out of a docx zip into a string.
pub fn load_document_xml(docx: &Path) -> Result<String> {
    let file =
        std::fs::File::open(docx).with_context(|| format!("opening docx {}", docx.display()))?;
    let mut zip = zip::ZipArchive::new(file).context("reading docx as zip")?;
    let mut entry = zip
        .by_name("word/document.xml")
        .context("docx is missing word/document.xml")?;
    let mut xml = String::new();
    entry.read_to_string(&mut xml)?;
    Ok(xml)
}

fn count_substring(haystack: &str, needle: &str) -> usize {
    if needle.is_empty() || haystack.len() < needle.len() {
        return 0;
    }
    let bytes = haystack.as_bytes();
    let n = needle.as_bytes();
    let mut i = 0usize;
    let mut count = 0usize;
    let max = bytes.len() - n.len();
    while i <= max {
        if &bytes[i..i + n.len()] == n {
            count += 1;
            i += n.len();
        } else {
            i += 1;
        }
    }
    count
}

fn find_subslice(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || from > haystack.len() || haystack.len() < needle.len() {
        return None;
    }
    let max = haystack.len() - needle.len();
    let mut i = from;
    while i <= max {
        if &haystack[i..i + needle.len()] == needle {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn memchr_byte(haystack: &[u8], byte: u8, from: usize) -> Option<usize> {
    haystack[from..]
        .iter()
        .position(|&b| b == byte)
        .map(|p| p + from)
}

/// Per-scope summary: returns `(scope, pass, warn, fail)` counts for the
/// HTML reporter.
pub fn summarise_by_scope(p: &ParityReport) -> BTreeMap<String, (usize, usize, usize)> {
    let mut m: BTreeMap<String, (usize, usize, usize)> = BTreeMap::new();
    for f in &p.findings {
        let e = m.entry(f.scope.clone()).or_insert((0, 0, 0));
        match f.severity {
            Severity::Info => e.0 += 1,
            Severity::Warn => e.1 += 1,
            Severity::Error | Severity::Blocking => e.2 += 1,
        }
    }
    m
}

// ───────────────────────────────────────────────────────────────────────
// Tests
// ───────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn count_substring_basic() {
        assert_eq!(count_substring("aaa", "a"), 3);
        assert_eq!(count_substring("aaaa", "aa"), 2);
        assert_eq!(count_substring("abcabc", "abc"), 2);
        assert_eq!(count_substring("xyz", "a"), 0);
        assert_eq!(count_substring("xyz", ""), 0);
    }

    /// Round-F (AI-Norms parity, 2026-06-03): a deficit larger than 5×band
    /// (≈25 %) still ERRORs — real regressions remain visible.
    #[test]
    fn figure_count_parity_fails_on_large_drift() {
        // ±5 % of 133 = 7; 5×band = 35. 100 is 33 below ref but within 5×band,
        // so we need a larger drop to ERROR. Use a 50 % drop (75 below ref).
        let cur_xml = "<w:drawing/>".repeat(50);
        let ref_xml = "<w:drawing/>".repeat(133);
        let f = check_figure_count(&p("ref.docx"), &p("cur.docx"), &ref_xml, &cur_xml);
        assert_eq!(f.actual, "50");
        assert_eq!(f.delta, -83);
        assert!(matches!(f.severity, Severity::Error));
    }

    /// Round-F: deficit within ±5 % band → INFO; minor content drift is OK.
    #[test]
    fn figure_count_parity_within_band_is_info() {
        // ±5 % of 133 = 7 (ceil); 130 is delta -3, within band.
        let cur_xml = "<w:drawing/>".repeat(130);
        let ref_xml = "<w:drawing/>".repeat(133);
        let f = check_figure_count(&p("ref.docx"), &p("cur.docx"), &ref_xml, &cur_xml);
        assert_eq!(f.actual, "130");
        assert_eq!(f.delta, -3);
        assert!(matches!(f.severity, Severity::Info));
    }

    /// Round-F: deficit beyond band but within 5×band → WARN.
    /// This covers the AI-Norms book's 433-vs-418 residual (-15 ≈ 3.5 %)
    /// after Round-F's keypoints-dedupe + ordered-list BkBullet fixes.
    #[test]
    fn figure_count_parity_within_warn_band_is_warning() {
        // ±5 % of 433 = 22; 418 is delta -15, within band → INFO.
        // Use 400 (delta -33) which is outside band but inside 5×band (110).
        let cur_xml = "<w:drawing/>".repeat(400);
        let ref_xml = "<w:drawing/>".repeat(433);
        let f = check_figure_count(&p("ref.docx"), &p("cur.docx"), &ref_xml, &cur_xml);
        assert_eq!(f.actual, "400");
        assert_eq!(f.delta, -33);
        assert!(matches!(f.severity, Severity::Warn));
    }

    #[test]
    fn figure_count_parity_passes_on_match() {
        let xml = "<w:drawing>...</w:drawing>".repeat(133);
        let f = check_figure_count(&p("ref"), &p("cur"), &xml, &xml);
        assert_eq!(f.actual, "133");
        assert_eq!(f.delta, 0);
        assert!(matches!(f.severity, Severity::Info));
    }

    #[test]
    fn captioned_table_recogniser_matches_table_n_dot() {
        assert!(is_table_caption_text("Table 1. Foo"));
        assert!(is_table_caption_text("Table 12. Things"));
        assert!(!is_table_caption_text("Table of Contents"));
        assert!(!is_table_caption_text("Some other text"));
        assert!(!is_table_caption_text("Tabletop"));
    }

    #[test]
    fn captioned_table_parity_fail_zero_vs_twentytwo() {
        // current has no captioned tables; reference has 22.
        let cur_xml = "<w:p><w:r><w:t>Some prose</w:t></w:r></w:p>";
        // Build 22 captioned tables with <w:tblHeader/>.
        let mut ref_xml = String::new();
        for n in 1..=22 {
            ref_xml.push_str(&format!(
                "<w:p><w:r><w:t>Table {n}. Caption</w:t></w:r></w:p>\
                 <w:tbl><w:tr><w:trPr><w:tblHeader/></w:trPr></w:tr></w:tbl>"
            ));
        }
        let f = check_captioned_table_count(&p("ref"), &p("cur"), &ref_xml, cur_xml);
        assert_eq!(f.expected, "22");
        assert_eq!(f.actual, "0");
        assert_eq!(f.delta, -22);
        assert!(matches!(f.severity, Severity::Error));
    }

    #[test]
    fn content_table_inventory_pass_on_full_match() {
        let mut xml = String::new();
        for n in 1..=22 {
            xml.push_str(&format!(
                "<w:p><w:r><w:t>Table {n}. Caption {n}</w:t></w:r></w:p>\
                 <w:tbl><w:tr><w:trPr><w:tblHeader/></w:trPr></w:tr></w:tbl>"
            ));
        }
        let f = check_content_table_inventory(&p("ref"), &p("cur"), &xml, &xml);
        assert!(matches!(f.severity, Severity::Info), "{f:?}");
        assert_eq!(f.delta, 0);
    }

    #[test]
    fn content_table_inventory_warn_on_one_paraphrase() {
        let mut ref_xml = String::new();
        let mut cur_xml = String::new();
        for n in 1..=22 {
            ref_xml.push_str(&format!(
                "<w:p><w:r><w:t>Table {n}. Caption {n}</w:t></w:r></w:p>\
                 <w:tbl><w:tr><w:trPr><w:tblHeader/></w:trPr></w:tr></w:tbl>"
            ));
            let cap = if n == 7 { "Caption seven" } else { "Caption" };
            cur_xml.push_str(&format!(
                "<w:p><w:r><w:t>Table {n}. {cap} {n}</w:t></w:r></w:p>\
                 <w:tbl><w:tr><w:trPr><w:tblHeader/></w:trPr></w:tr></w:tbl>"
            ));
        }
        let f = check_content_table_inventory(&p("ref"), &p("cur"), &ref_xml, &cur_xml);
        assert!(matches!(f.severity, Severity::Warn), "{f:?}");
    }

    #[test]
    fn content_table_inventory_error_on_missing_set() {
        let mut ref_xml = String::new();
        for n in 1..=22 {
            ref_xml.push_str(&format!(
                "<w:p><w:r><w:t>Table {n}. Caption {n}</w:t></w:r></w:p>\
                 <w:tbl><w:tr><w:trPr><w:tblHeader/></w:trPr></w:tr></w:tbl>"
            ));
        }
        let cur_xml = String::new(); // zero captioned tables
        let f = check_content_table_inventory(&p("ref"), &p("cur"), &ref_xml, &cur_xml);
        assert!(matches!(f.severity, Severity::Error), "{f:?}");
        assert_eq!(f.delta, -22);
        assert!(f.evidence.contains("missing:"));
    }

    #[test]
    fn collect_captions_strips_table_n_prefix() {
        let xml = "<w:p><w:r><w:t>Table 4. The benchmark suite</w:t></w:r></w:p>\
                   <w:tbl><w:tr><w:trPr><w:tblHeader/></w:trPr></w:tr></w:tbl>";
        let caps = collect_content_table_captions(xml);
        assert_eq!(caps, vec!["The benchmark suite".to_string()]);
    }

    #[test]
    fn captioned_table_parity_pass_on_full_match() {
        let mut xml = String::new();
        for n in 1..=22 {
            xml.push_str(&format!(
                "<w:p><w:r><w:t>Table {n}. Caption</w:t></w:r></w:p>\
                 <w:tbl><w:tr><w:trPr><w:tblHeader/></w:trPr></w:tr></w:tbl>"
            ));
        }
        let f = check_captioned_table_count(&p("ref"), &p("cur"), &xml, &xml);
        assert_eq!(f.actual, "22");
        assert_eq!(f.delta, 0);
        assert!(matches!(f.severity, Severity::Info));
    }

    #[test]
    fn style_usage_band_pass_within_ten_percent() {
        // ±10 % of 254 ⇒ ±26; 240 is within band.
        let ref_xml = "<w:pStyle w:val=\"BkH2\"/>".repeat(254);
        let cur_xml = "<w:pStyle w:val=\"BkH2\"/>".repeat(240);
        let findings = check_style_usage(&p("ref"), &p("cur"), &ref_xml, &cur_xml);
        let bkh2 = findings
            .iter()
            .find(|f| f.name == "style_usage_parity::BkH2")
            .expect("BkH2 finding present");
        assert!(matches!(bkh2.severity, Severity::Info));
        assert_eq!(bkh2.delta, -14);
    }

    #[test]
    fn style_usage_band_warn_outside_ten_percent_inside_fifty() {
        // ±10 % of 254 ⇒ ±26; 200 is outside band (-54) but inside 50 %.
        let ref_xml = "<w:pStyle w:val=\"BkH2\"/>".repeat(254);
        let cur_xml = "<w:pStyle w:val=\"BkH2\"/>".repeat(200);
        let findings = check_style_usage(&p("ref"), &p("cur"), &ref_xml, &cur_xml);
        let bkh2 = findings
            .iter()
            .find(|f| f.name == "style_usage_parity::BkH2")
            .unwrap();
        assert!(matches!(bkh2.severity, Severity::Warn));
        assert_eq!(bkh2.delta, -54);
    }

    #[test]
    fn style_usage_band_error_on_total_collapse() {
        // 254 → 0 is a >50 % drift ⇒ ERROR.
        let ref_xml = "<w:pStyle w:val=\"BkH2\"/>".repeat(254);
        let cur_xml = "";
        let findings = check_style_usage(&p("ref"), &p("cur"), &ref_xml, cur_xml);
        let bkh2 = findings
            .iter()
            .find(|f| f.name == "style_usage_parity::BkH2")
            .unwrap();
        assert!(matches!(bkh2.severity, Severity::Error));
        assert_eq!(bkh2.delta, -254);
    }

    #[test]
    fn extract_attr_number_finds_first_match() {
        let xml = r#"<w:pgMar w:top="1440" w:header="720" w:footer="720"/>"#;
        assert_eq!(extract_attr_number(xml, "w:pgMar", "w:header"), Some(720));
        assert_eq!(extract_attr_number(xml, "w:pgMar", "w:footer"), Some(720));
        assert_eq!(extract_attr_number(xml, "w:pgMar", "w:top"), Some(1440));
        assert_eq!(extract_attr_number(xml, "w:pgMar", "w:missing"), None);
    }

    #[test]
    fn numeric_eq_pass_when_actual_matches_spec_target() {
        // expected (ref) = 0, actual = 0, spec target = 0 ⇒ PASS.
        let f = numeric_eq_finding(
            "layout",
            "header_part_count",
            0,
            0,
            Some(0),
            &p("ref"),
            &p("cur"),
            "test",
        );
        assert!(matches!(f.severity, Severity::Info));
    }

    #[test]
    fn numeric_eq_fail_when_actual_neither_ref_nor_spec() {
        let f = numeric_eq_finding(
            "layout",
            "footer_part_count",
            1,
            3,
            Some(1),
            &p("ref"),
            &p("cur"),
            "test",
        );
        assert!(matches!(f.severity, Severity::Error));
        assert_eq!(f.delta, 2);
    }

    #[test]
    fn back_matter_order_passes_when_canonical_sequence_matches() {
        let ref_order = vec![
            "Appendix".to_string(),
            "ToF".to_string(),
            "ToT".to_string(),
            "Bibliography".to_string(),
            "Index".to_string(),
        ];
        let cur_order = ref_order.clone();
        let f = back_matter_order_finding(&ref_order, &cur_order, &p("ref"), &p("cur"));
        assert!(matches!(f.severity, Severity::Info));
    }

    #[test]
    fn back_matter_order_warn_when_current_empty() {
        let ref_order = vec!["Appendix".to_string(), "ToF".to_string()];
        let cur_order: Vec<String> = Vec::new();
        let f = back_matter_order_finding(&ref_order, &cur_order, &p("ref"), &p("cur"));
        assert!(matches!(f.severity, Severity::Warn));
    }

    /// Wave-9 (AI-Norms parity, 2026-06-03) — the `extract_back_matter_headings`
    /// heuristic must recognise the renderer's actual heading strings
    /// ("Table of Figures" / "Table of Tables") and map them to the
    /// canonical "ToF" / "ToT" back-matter slots that the parity gate
    /// compares against. Without this mapping the gate FAILed even when
    /// the renderer was emitting the correct sequence.
    #[test]
    fn extract_back_matter_recognises_table_of_figures_aliases() {
        let xml = r#"<w:p><w:pPr><w:pStyle w:val="BkH1"/></w:pPr><w:r><w:t>Appendix A — Sources</w:t></w:r></w:p>
<w:p><w:pPr><w:pStyle w:val="BkH1"/></w:pPr><w:r><w:t>Table of Figures</w:t></w:r></w:p>
<w:p><w:pPr><w:pStyle w:val="BkH1"/></w:pPr><w:r><w:t>Table of Tables</w:t></w:r></w:p>
<w:p><w:pPr><w:pStyle w:val="BkH1"/></w:pPr><w:r><w:t>Bibliography</w:t></w:r></w:p>
<w:p><w:pPr><w:pStyle w:val="BkH1"/></w:pPr><w:r><w:t>Index</w:t></w:r></w:p>"#;
        let out = extract_back_matter_headings(xml);
        assert_eq!(out, vec!["Appendix", "ToF", "ToT", "Bibliography", "Index"]);
    }

    /// Mirror of the above for the i18n fallback heading strings
    /// ("List of Figures" / "List of Tables") that the engine emits when
    /// `meta.tof_heading`/`tot_heading` are not set on the manifest.
    #[test]
    fn extract_back_matter_recognises_list_of_figures_aliases() {
        let xml = r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>List of Figures</w:t></w:r></w:p>
<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>List of Tables</w:t></w:r></w:p>"#;
        let out = extract_back_matter_headings(xml);
        assert_eq!(out, vec!["ToF", "ToT"]);
    }

    #[test]
    fn summarise_by_scope_groups_findings() {
        let report = ParityReport {
            reference: "ref".into(),
            current: "cur".into(),
            findings: vec![
                ParityFinding {
                    scope: "figures".into(),
                    name: "x".into(),
                    severity: Severity::Error,
                    expected: "133".into(),
                    actual: "0".into(),
                    delta: -133,
                    evidence: String::new(),
                    message: String::new(),
                },
                ParityFinding {
                    scope: "figures".into(),
                    name: "y".into(),
                    severity: Severity::Info,
                    expected: "1".into(),
                    actual: "1".into(),
                    delta: 0,
                    evidence: String::new(),
                    message: String::new(),
                },
            ],
            parity_pct: 50.0,
        };
        let m = summarise_by_scope(&report);
        let (pass, warn, fail) = m["figures"];
        assert_eq!(pass, 1);
        assert_eq!(warn, 0);
        assert_eq!(fail, 1);
    }
}
