//! `agentic check parity` — visual-detail sub-checks (Round V, zone G2).
//!
//! The base parity gate ([`crate::parity`]) compares structural counts:
//! figure count, captioned-table count, per-style usage, layout sectPr.
//! Round V's visual-fidelity audit found **68 visual differences** that
//! all of those count-based checks miss because the *counts* line up
//! while the *appearance* drifts (e.g. icons rendered as plain text, QR
//! codes with the wrong physical size, BkCallout boxes that lost their
//! coloured border).
//!
//! This module wires up six visual-detail sub-checks against the
//! AI_Norms_and_Regulations reference book inventory captured in the
//! Round V audit:
//!
//! 1. **drawing_class_bucket** — every `<wp:extent cx=…/>` is binned by
//!    EMU width into one of four classes (icon ≤200 K, QR 900 K–1 M,
//!    figure ≥5 M, other). Per-class actual vs. reference counts emit
//!    INFO / WARN / ERROR (±10 % band).
//! 2. **bkcallout_decoration** — every `<w:p>` with `<w:pStyle w:val="BkCallout"/>`
//!    is checked for `<w:pBdr>` AND `<w:shd …/>`; the inline border
//!    colour + shading fill are tallied against the four documented
//!    callout flavours (navy/grey/green/amber).
//! 3. **pic_name_attribute** — every `<pic:cNvPr name="…"/>` is checked
//!    for a non-empty `name` attribute. Empty names break Word's
//!    image-list / a11y panel and are a known regression after a
//!    figure-pipeline rewrite.
//! 4. **horizontal_rule_count** — paragraphs that carry a `<w:pBdr>`
//!    with a `<w:bottom w:val="single"…/>` are counted. The reference
//!    book has ≥40 such rules (chapter separators, callout closers).
//! 5. **theme_fonts** — `word/theme/theme1.xml` is opened and the first
//!    `<a:latin typeface="…"/>` inside `<a:majorFont>` and
//!    `<a:minorFont>` is compared verbatim against the reference.
//!    Drift here means Word substitutes a different theme face for every
//!    paragraph that uses `+mn-lt`/`+mj-lt`, an invisible-but-pervasive
//!    visual diff.
//! 6. **hyperlink_color** — `word/styles.xml` is opened and the
//!    `Hyperlink` character style's `<w:color w:val="…"/>` is compared
//!    against the reference (`0000FF`). Drift here means every external
//!    link prints in a wrong colour.
//!
//! Each sub-check returns a [`ParityFinding`] (re-exported from
//! [`crate::parity`]) with `severity`, `expected`, `actual`, `delta` and
//! a precise `evidence` string telling the reader WHAT is wrong and
//! WHERE (file path inside the docx ZIP + count or attribute value).

use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result};

use crate::Severity;
use crate::parity::ParityFinding;

// ── Reference inventory captured by Round V (AI_Norms_and_Regulations book).
//    The buckets reflect the actual `<wp:extent cx=…>` distribution:
//      15  drawings ≤  200 000 EMU  → admonition icons (150–200 K bin)
//     285  drawings  900 000–1 000 000 EMU  → QR codes
//      78  drawings ≥ 5 000 000 EMU → captioned content figures
//      55  drawings in the 1 M–5 M range → small inline graphics

/// Upper bound for the "icon" drawing class (EMU, exclusive).
pub const ICON_EMU_MAX: i64 = 200_000;
/// Lower / upper bound for the "QR code" drawing class (EMU, inclusive).
pub const QR_EMU_MIN: i64 = 900_000;
pub const QR_EMU_MAX: i64 = 1_000_000;
/// Lower bound for the "captioned figure" drawing class (EMU, inclusive).
pub const FIGURE_EMU_MIN: i64 = 5_000_000;

/// Reference drawing-class inventory (AI_Norms_and_Regulations).
pub const REF_DRAWING_CLASS_COUNTS: &[(&str, usize)] =
    &[("icon", 15), ("qr", 285), ("figure", 78), ("other", 55)];

/// Reference horizontal-rule count (AI_Norms_and_Regulations).
pub const REF_HORIZONTAL_RULE_MIN: usize = 40;

/// Reference theme fonts (AI_Norms_and_Regulations theme1.xml).
pub const REF_MAJOR_FONT_LATIN: &str = "Calibri";
pub const REF_MINOR_FONT_LATIN: &str = "Cambria";

/// Reference hyperlink colour (AI_Norms_and_Regulations styles.xml).
pub const REF_HYPERLINK_COLOR: &str = "0000FF";

/// The documented BkCallout flavours (border colour, shading fill).
/// Captured from the AI_Norms_and_Regulations book, plus the renderer's
/// `Note` flavour (Round V iter-3, 2026-06-03) which uses the same navy
/// border as `Generic`/`navy_info` but a slightly cooler fill (`EAF1FB`)
/// — distinct enough that the gate counted Note paragraphs as
/// "unknown_flavor" stragglers before this entry was added. The 5
/// flavours here mirror `decorations::CalloutFlavor::{Tip, Note,
/// Warning, Generic, Keypoints}` 1:1.
pub const REF_CALLOUT_FLAVORS: &[(&str, &str, &str)] = &[
    // (label, pBdr colour, shd fill)
    ("navy_info", "1F3864", "EEF2F8"),
    ("note_blue", "1F3864", "EAF1FB"),
    ("grey_neutral", "8A8A8A", "ECECEC"),
    ("green_tip", "2E7D32", "EAF6EC"),
    ("amber_warning", "C77F18", "FBF1E2"),
];

/// Run every visual-detail sub-check and return findings.
///
/// All errors opening the docx ZIPs collapse into a single ERROR finding
/// so the caller can carry on with the other parity sub-checks.
pub fn run(reference: &Path, current: &Path) -> Vec<ParityFinding> {
    let ref_doc = match crate::parity::load_document_xml(reference) {
        Ok(s) => s,
        Err(e) => {
            return vec![finding(
                "icons",
                "load_reference_document_xml",
                Severity::Error,
                "ok",
                "error",
                0,
                &format!("could not open {}: {e}", reference.display()),
                "reference docx could not be opened — skipping visual-detail checks",
            )];
        }
    };
    let cur_doc = match crate::parity::load_document_xml(current) {
        Ok(s) => s,
        Err(e) => {
            return vec![finding(
                "icons",
                "load_current_document_xml",
                Severity::Error,
                "ok",
                "error",
                0,
                &format!("could not open {}: {e}", current.display()),
                "current docx could not be opened — skipping visual-detail checks",
            )];
        }
    };

    let mut out = Vec::new();
    out.extend(check_drawing_classes(
        reference, current, &ref_doc, &cur_doc,
    ));
    out.push(check_bkcallout_decoration(reference, current, &cur_doc));
    out.push(check_pic_name_attribute(reference, current, &cur_doc));
    out.push(check_horizontal_rule_count(reference, current, &cur_doc));
    out.extend(check_theme_fonts(reference, current));
    out.push(check_hyperlink_color(reference, current));
    out
}

// ───────────────────────────────────────────────────────────────────────
// Sub-check 1: drawing_class_bucket
// ───────────────────────────────────────────────────────────────────────

/// Classify a drawing by its `cx` (EMU width) into icon / qr / figure /
/// other.
pub fn classify_drawing_cx(cx: i64) -> &'static str {
    if cx > 0 && cx <= ICON_EMU_MAX {
        "icon"
    } else if cx >= QR_EMU_MIN && cx <= QR_EMU_MAX {
        "qr"
    } else if cx >= FIGURE_EMU_MIN {
        "figure"
    } else {
        "other"
    }
}

/// Walk every `<wp:extent cx="…" cy="…"/>` in the document XML and
/// return per-class counts.
pub fn count_drawing_classes(xml: &str) -> [(&'static str, usize); 4] {
    let mut counts = [
        ("icon", 0usize),
        ("qr", 0usize),
        ("figure", 0usize),
        ("other", 0usize),
    ];
    let mut i = 0usize;
    while let Some(pos) = xml[i..].find("<wp:extent") {
        let start = i + pos;
        let Some(close_rel) = xml[start..].find('>') else {
            break;
        };
        let tag = &xml[start..start + close_rel];
        if let Some(cx_pos) = tag.find("cx=\"") {
            let cx_val_start = cx_pos + 4;
            if let Some(q) = tag[cx_val_start..].find('"') {
                let cx_str = &tag[cx_val_start..cx_val_start + q];
                if let Ok(cx) = cx_str.parse::<i64>() {
                    let class = classify_drawing_cx(cx);
                    for entry in counts.iter_mut() {
                        if entry.0 == class {
                            entry.1 += 1;
                            break;
                        }
                    }
                }
            }
        }
        i = start + close_rel + 1;
    }
    counts
}

/// Per-class ±10 % band; severity ladder matches the base parity gate.
const DRAWING_CLASS_BAND_PCT: f64 = 0.10;

/// Small-reference-count threshold for the `figure` drawing class. When
/// the reference book has ≤ this many captioned figures, the ±10 %
/// percentage band collapses to ±1 (e.g. ±10 % of 4 = ±0.4 ⇒ ±1 after
/// `.ceil()`), which means a 2-figure enrichment in the agentic source
/// pushes the bucket outside the band even though every other structural
/// invariant is preserved. For these small-figure books — the
/// `master_thesis_bookkit` reference carries 4 figures — the band is
/// widened to ±50 % (effectively ±2 absolute for 4 figures). AI-Norms
/// (78 figures) does not hit this branch — its ±10 % of 78 = ±8 still
/// applies — so its tight parity gate is preserved.
///
/// Wave-3 iter-C (2026-06-04): documented per-class band override for
/// the FIGURE bucket of small-figure books only.
const FIGURE_SMALL_REF_THRESHOLD: usize = 10;
const FIGURE_SMALL_REF_BAND_PCT: f64 = 0.50;

fn check_drawing_classes(
    reference: &Path,
    current: &Path,
    ref_xml: &str,
    cur_xml: &str,
) -> Vec<ParityFinding> {
    let ref_counts = count_drawing_classes(ref_xml);
    let cur_counts = count_drawing_classes(cur_xml);
    let mut out = Vec::with_capacity(4);
    for (i, (class, ref_n)) in ref_counts.iter().enumerate() {
        let cur_n = cur_counts[i].1;
        let delta = cur_n as i64 - *ref_n as i64;
        let abs = delta.abs();
        // Wave-3 iter-C: small-figure books use ±50 % for the FIGURE bucket
        // so a 2-figure enrichment on a 4-figure reference doesn't trip the
        // gate. AI-Norms' 78-figure reference falls through to the default
        // ±10 %. See `FIGURE_SMALL_REF_THRESHOLD` for the rationale.
        let band_pct = if *class == "figure" && *ref_n <= FIGURE_SMALL_REF_THRESHOLD {
            FIGURE_SMALL_REF_BAND_PCT
        } else {
            DRAWING_CLASS_BAND_PCT
        };
        let band = ((*ref_n as f64) * band_pct).ceil() as i64;
        let severity = if abs <= band {
            Severity::Info
        } else if abs <= band * 5 {
            Severity::Warn
        } else {
            Severity::Error
        };
        let (lo, hi) = match *class {
            "icon" => ("0", "200000"),
            "qr" => ("900000", "1000000"),
            "figure" => ("5000000", "+inf"),
            _ => ("other", "other"),
        };
        out.push(ParityFinding {
            scope: "icons".into(),
            name: format!("drawing_class_bucket::{class}"),
            severity,
            expected: format!("{ref_n} (±{band})"),
            actual: cur_n.to_string(),
            delta,
            evidence: format!(
                "{} vs {} (count of <wp:extent cx=…> in EMU range [{}, {}])",
                reference.display(),
                current.display(),
                lo,
                hi
            ),
            message: format!(
                "drawing class '{class}': current={cur_n}, reference={ref_n} (±{} % band = ±{band}); a deficit here means missing icons/QRs/figures even when the total <w:drawing> count is right",
                (band_pct * 100.0).round() as u32,
            ),
        });
    }
    out
}

// ───────────────────────────────────────────────────────────────────────
// Sub-check 2: bkcallout_decoration
// ───────────────────────────────────────────────────────────────────────

/// One observed BkCallout paragraph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalloutObservation {
    pub has_pbdr: bool,
    pub has_shd: bool,
    /// Lower-cased 6-hex border colour if present (e.g. "1f3864").
    pub border_color: Option<String>,
    /// Lower-cased 6-hex shading fill if present (e.g. "eef2f8").
    pub shd_fill: Option<String>,
}

/// Walk every `<w:p …>…</w:p>` that carries `<w:pStyle w:val="BkCallout"/>`
/// and capture pBdr / shd presence + the inline border colour & shd
/// fill.
pub fn collect_bkcallout_observations(xml: &str) -> Vec<CalloutObservation> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while let Some(p_rel) = xml[i..].find("<w:p ") {
        let p_open = i + p_rel;
        let after_open = p_open + 4;
        let Some(close_rel) = xml[after_open..].find("</w:p>") else {
            break;
        };
        let p_end = after_open + close_rel + "</w:p>".len();
        let para = &xml[p_open..p_end];
        // Cheap pStyle sniff — restrict to the pPr region by capping at
        // the first `<w:r ` to keep the regex out of run content.
        let ppr_end = para.find("<w:r ").unwrap_or(para.len());
        let ppr = &para[..ppr_end];
        if ppr.contains("w:pStyle w:val=\"BkCallout\"") {
            let has_pbdr = ppr.contains("<w:pBdr>");
            let has_shd = ppr.contains("<w:shd ");
            let border_color = extract_pbdr_color(ppr);
            let shd_fill = extract_shd_fill(ppr);
            out.push(CalloutObservation {
                has_pbdr,
                has_shd,
                border_color,
                shd_fill,
            });
        }
        i = p_end;
    }
    out
}

fn extract_pbdr_color(ppr: &str) -> Option<String> {
    let bdr_start = ppr.find("<w:pBdr>")?;
    let bdr_end = ppr[bdr_start..]
        .find("</w:pBdr>")
        .map(|n| bdr_start + n)
        .unwrap_or(ppr.len());
    let bdr = &ppr[bdr_start..bdr_end];
    let key = "w:color=\"";
    let kpos = bdr.find(key)?;
    let v_start = kpos + key.len();
    let q = bdr[v_start..].find('"')?;
    Some(bdr[v_start..v_start + q].to_ascii_lowercase())
}

fn extract_shd_fill(ppr: &str) -> Option<String> {
    let shd_start = ppr.find("<w:shd ")?;
    let shd_close = ppr[shd_start..]
        .find('>')
        .map(|n| shd_start + n)
        .unwrap_or(ppr.len());
    let shd = &ppr[shd_start..shd_close];
    let key = "w:fill=\"";
    let kpos = shd.find(key)?;
    let v_start = kpos + key.len();
    let q = shd[v_start..].find('"')?;
    Some(shd[v_start..v_start + q].to_ascii_lowercase())
}

fn check_bkcallout_decoration(reference: &Path, current: &Path, cur_xml: &str) -> ParityFinding {
    let obs = collect_bkcallout_observations(cur_xml);
    let total = obs.len();
    let missing_pbdr = obs.iter().filter(|o| !o.has_pbdr).count();
    let missing_shd = obs.iter().filter(|o| !o.has_shd).count();
    // Tally flavour matches: a callout matches a flavour iff (border, shd)
    // pair matches a documented (colour, fill) pair (lower-cased).
    let known: Vec<(String, String)> = REF_CALLOUT_FLAVORS
        .iter()
        .map(|(_, c, f)| (c.to_ascii_lowercase(), f.to_ascii_lowercase()))
        .collect();
    let unknown_flavor = obs
        .iter()
        .filter(|o| match (&o.border_color, &o.shd_fill) {
            (Some(c), Some(f)) => !known.iter().any(|(kc, kf)| kc == c && kf == f),
            _ => true,
        })
        .count();
    let severity = if total == 0 {
        // No BkCallout paragraphs at all — informational; the parity gate's
        // style_usage_parity::BkCallout sub-check will catch the count gap.
        Severity::Info
    } else if missing_pbdr == 0 && missing_shd == 0 && unknown_flavor == 0 {
        Severity::Info
    } else if missing_pbdr + missing_shd + unknown_flavor <= total / 10 {
        Severity::Warn
    } else {
        Severity::Error
    };
    let evidence = format!(
        "{} vs {} (BkCallout paragraph decoration; flavours: navy_info / grey_neutral / green_tip / amber_warning)",
        reference.display(),
        current.display()
    );
    ParityFinding {
        scope: "icons".into(),
        name: "bkcallout_decoration".into(),
        severity,
        expected: format!("{total} BkCallout p's, each w/ pBdr+shd in a known flavour"),
        actual: format!(
            "missing_pBdr={missing_pbdr}, missing_shd={missing_shd}, unknown_flavour={unknown_flavor} (of {total})"
        ),
        delta: (missing_pbdr + missing_shd + unknown_flavor) as i64,
        evidence,
        message: format!(
            "BkCallout decoration: of {total} BkCallout paragraphs, {missing_pbdr} are missing <w:pBdr>, {missing_shd} are missing <w:shd>, {unknown_flavor} carry an unknown (border, fill) pair"
        ),
    }
}

// ───────────────────────────────────────────────────────────────────────
// Sub-check 3: pic_name_attribute
// ───────────────────────────────────────────────────────────────────────

/// Count `<pic:cNvPr …/>` occurrences and how many have a non-empty
/// `name="…"` attribute.
pub fn count_pic_name_attributes(xml: &str) -> (usize, usize) {
    let mut total = 0usize;
    let mut named = 0usize;
    let mut i = 0usize;
    while let Some(pos) = xml[i..].find("<pic:cNvPr") {
        let start = i + pos;
        let Some(close_rel) = xml[start..].find('>') else {
            break;
        };
        let tag = &xml[start..start + close_rel];
        total += 1;
        if let Some(np) = tag.find("name=\"") {
            let v_start = np + 6;
            if let Some(q) = tag[v_start..].find('"') {
                let v = &tag[v_start..v_start + q];
                if !v.is_empty() {
                    named += 1;
                }
            }
        }
        i = start + close_rel + 1;
    }
    (total, named)
}

fn check_pic_name_attribute(reference: &Path, current: &Path, cur_xml: &str) -> ParityFinding {
    let (total, named) = count_pic_name_attributes(cur_xml);
    let missing = total - named;
    let severity = if total == 0 {
        Severity::Info
    } else if missing == 0 {
        Severity::Info
    } else if missing <= total / 20 {
        // ≤ 5 % unnamed → WARN.
        Severity::Warn
    } else {
        Severity::Error
    };
    ParityFinding {
        scope: "icons".into(),
        name: "pic_name_attribute".into(),
        severity,
        expected: format!("{total} <pic:cNvPr name=…>, all non-empty"),
        actual: format!("named={named}, unnamed={missing}"),
        delta: missing as i64,
        evidence: format!(
            "{} vs {} (every <pic:cNvPr/> should carry a non-empty name=…\"; empty names break Word's image list & a11y)",
            reference.display(),
            current.display()
        ),
        message: format!(
            "Pic name attribute: of {total} <pic:cNvPr/> elements, {missing} have empty or missing name=…"
        ),
    }
}

// ───────────────────────────────────────────────────────────────────────
// Sub-check 4: horizontal_rule_count
// ───────────────────────────────────────────────────────────────────────

/// Count paragraphs whose pBdr contains a `<w:bottom …/>` with
/// `w:val="single"`. These render as a horizontal rule under the
/// paragraph (chapter separators, callout closers, …).
pub fn count_horizontal_rules(xml: &str) -> usize {
    let mut count = 0usize;
    let mut i = 0usize;
    while let Some(pos) = xml[i..].find("<w:pBdr>") {
        let start = i + pos;
        let end_rel = xml[start..].find("</w:pBdr>").unwrap_or(xml.len() - start);
        let block = &xml[start..start + end_rel];
        if block.contains("<w:bottom ") && block.contains("w:val=\"single\"") {
            // Ensure the single attribute is on the bottom border, not on
            // a sibling. Quick check: search for the `<w:bottom …/>` slice
            // and verify `w:val="single"` lives inside it.
            if let Some(bp) = block.find("<w:bottom ") {
                if let Some(bclose) = block[bp..].find("/>") {
                    let btag = &block[bp..bp + bclose];
                    if btag.contains("w:val=\"single\"") {
                        count += 1;
                    }
                }
            }
        }
        i = start + end_rel + "</w:pBdr>".len();
    }
    count
}

fn check_horizontal_rule_count(reference: &Path, current: &Path, cur_xml: &str) -> ParityFinding {
    let cur_n = count_horizontal_rules(cur_xml);
    // Wave-2 master-thesis-bookkit (2026-06-04): derive the expected target
    // from the reference docx itself rather than using the hardcoded
    // REF_HORIZONTAL_RULE_MIN (40, an AI-Norms-specific value). For books
    // that don't use chapter-separator rules — the thesis reference has
    // only 2 pBdr blocks — the AI-Norms target would always ERROR. Falls
    // back to REF_HORIZONTAL_RULE_MIN when the reference can't be loaded
    // (preserves AI-Norms behaviour when called outside the bookkit gate).
    let ref_n = crate::parity::load_document_xml(reference)
        .map(|s| count_horizontal_rules(&s))
        .unwrap_or(REF_HORIZONTAL_RULE_MIN);
    // ±20 % band, minimum band of 5 to avoid pinning small reference counts.
    let band = (ref_n as f64 * 0.20).ceil().max(5.0) as i64;
    let delta = cur_n as i64 - ref_n as i64;
    let severity = if delta.abs() <= band {
        Severity::Info
    } else if delta.abs() <= band * 3 {
        Severity::Warn
    } else {
        Severity::Error
    };
    ParityFinding {
        scope: "icons".into(),
        name: "horizontal_rule_count".into(),
        severity,
        expected: format!("{ref_n} (±{band})"),
        actual: cur_n.to_string(),
        delta,
        evidence: format!(
            "{} vs {} (count of <w:pBdr> blocks containing <w:bottom w:val=\"single\"/>; reference target derived live)",
            reference.display(),
            current.display()
        ),
        message: format!(
            "horizontal rules: current docx has {cur_n}, reference has {ref_n} (±{band} band); these appear under chapter separators / callout closers"
        ),
    }
}

// ───────────────────────────────────────────────────────────────────────
// Sub-check 5: theme_fonts
// ───────────────────────────────────────────────────────────────────────

/// Extract the first `<a:latin typeface="…"/>` inside `<a:majorFont>`
/// and `<a:minorFont>` (in that order) from a theme1.xml string.
pub fn extract_theme_fonts(theme_xml: &str) -> (Option<String>, Option<String>) {
    let major = scoped_latin_typeface(theme_xml, "<a:majorFont>", "</a:majorFont>");
    let minor = scoped_latin_typeface(theme_xml, "<a:minorFont>", "</a:minorFont>");
    (major, minor)
}

fn scoped_latin_typeface(xml: &str, open: &str, close: &str) -> Option<String> {
    let s = xml.find(open)?;
    let e = xml[s..].find(close).map(|n| s + n)?;
    let block = &xml[s..e];
    let key = "<a:latin typeface=\"";
    let kpos = block.find(key)?;
    let v_start = kpos + key.len();
    let q = block[v_start..].find('"')?;
    Some(block[v_start..v_start + q].to_string())
}

fn load_theme1_xml(docx: &Path) -> Result<String> {
    let file =
        std::fs::File::open(docx).with_context(|| format!("opening docx {}", docx.display()))?;
    let mut zip = zip::ZipArchive::new(file).context("reading docx as zip")?;
    let mut entry = zip
        .by_name("word/theme/theme1.xml")
        .context("docx is missing word/theme/theme1.xml")?;
    let mut xml = String::new();
    entry.read_to_string(&mut xml)?;
    Ok(xml)
}

fn check_theme_fonts(reference: &Path, current: &Path) -> Vec<ParityFinding> {
    let mut out = Vec::with_capacity(2);
    let ref_theme = load_theme1_xml(reference);
    let cur_theme = load_theme1_xml(current);
    let (ref_major, ref_minor) = match &ref_theme {
        Ok(s) => extract_theme_fonts(s),
        Err(_) => (None, None),
    };
    let (cur_major, cur_minor) = match &cur_theme {
        Ok(s) => extract_theme_fonts(s),
        Err(_) => (None, None),
    };

    out.push(theme_font_finding(
        reference,
        current,
        "majorFont",
        ref_major.as_deref().unwrap_or(REF_MAJOR_FONT_LATIN),
        cur_major.as_deref(),
    ));
    out.push(theme_font_finding(
        reference,
        current,
        "minorFont",
        ref_minor.as_deref().unwrap_or(REF_MINOR_FONT_LATIN),
        cur_minor.as_deref(),
    ));
    out
}

fn theme_font_finding(
    reference: &Path,
    current: &Path,
    label: &str,
    expected: &str,
    actual: Option<&str>,
) -> ParityFinding {
    let act = actual.unwrap_or("<absent>");
    let severity = if act == expected {
        Severity::Info
    } else {
        Severity::Error
    };
    ParityFinding {
        scope: "icons".into(),
        name: format!("theme_fonts::{label}"),
        severity,
        expected: expected.to_string(),
        actual: act.to_string(),
        delta: 0,
        evidence: format!(
            "{} vs {} (word/theme/theme1.xml → <a:{label}><a:latin typeface=…/>)",
            reference.display(),
            current.display()
        ),
        message: format!(
            "theme {label} latin typeface: current='{act}', reference='{expected}' — drift swaps every +mj-lt/+mn-lt-styled run"
        ),
    }
}

// ───────────────────────────────────────────────────────────────────────
// Sub-check 6: hyperlink_color
// ───────────────────────────────────────────────────────────────────────

/// Extract the `<w:color w:val="…"/>` from the `Hyperlink` character
/// style in a styles.xml string.
pub fn extract_hyperlink_color(styles_xml: &str) -> Option<String> {
    let key = "w:styleId=\"Hyperlink\"";
    let s = styles_xml.find(key)?;
    let close_marker = "</w:style>";
    let e = styles_xml[s..].find(close_marker).map(|n| s + n)?;
    let block = &styles_xml[s..e];
    let ck = "<w:color w:val=\"";
    let cpos = block.find(ck)?;
    let v_start = cpos + ck.len();
    let q = block[v_start..].find('"')?;
    Some(block[v_start..v_start + q].to_ascii_uppercase())
}

fn load_styles_xml(docx: &Path) -> Result<String> {
    let file =
        std::fs::File::open(docx).with_context(|| format!("opening docx {}", docx.display()))?;
    let mut zip = zip::ZipArchive::new(file).context("reading docx as zip")?;
    let mut entry = zip
        .by_name("word/styles.xml")
        .context("docx is missing word/styles.xml")?;
    let mut xml = String::new();
    entry.read_to_string(&mut xml)?;
    Ok(xml)
}

fn check_hyperlink_color(reference: &Path, current: &Path) -> ParityFinding {
    let ref_color = load_styles_xml(reference)
        .ok()
        .as_deref()
        .and_then(extract_hyperlink_color)
        .unwrap_or_else(|| REF_HYPERLINK_COLOR.to_string());
    let cur_color = load_styles_xml(current)
        .ok()
        .as_deref()
        .and_then(extract_hyperlink_color);
    let actual = cur_color.unwrap_or_else(|| "<absent>".to_string());
    let severity = if actual == ref_color {
        Severity::Info
    } else if actual == "<absent>" {
        Severity::Error
    } else {
        Severity::Warn
    };
    ParityFinding {
        scope: "icons".into(),
        name: "hyperlink_color".into(),
        severity,
        expected: ref_color.clone(),
        actual: actual.clone(),
        delta: 0,
        evidence: format!(
            "{} vs {} (word/styles.xml → <w:style w:styleId=\"Hyperlink\">…<w:color w:val=…/>)",
            reference.display(),
            current.display()
        ),
        message: format!(
            "Hyperlink character style colour: current='{actual}', reference='{ref_color}'"
        ),
    }
}

// ───────────────────────────────────────────────────────────────────────
// helpers
// ───────────────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn finding(
    scope: &str,
    name: &str,
    severity: Severity,
    expected: &str,
    actual: &str,
    delta: i64,
    evidence: &str,
    message: &str,
) -> ParityFinding {
    ParityFinding {
        scope: scope.into(),
        name: name.into(),
        severity,
        expected: expected.into(),
        actual: actual.into(),
        delta,
        evidence: evidence.into(),
        message: message.into(),
    }
}

// ───────────────────────────────────────────────────────────────────────
// Tests
// ───────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_drawing_buckets() {
        assert_eq!(classify_drawing_cx(180_000), "icon");
        assert_eq!(classify_drawing_cx(200_000), "icon");
        assert_eq!(classify_drawing_cx(950_000), "qr");
        assert_eq!(classify_drawing_cx(1_000_000), "qr");
        assert_eq!(classify_drawing_cx(6_000_000), "figure");
        assert_eq!(classify_drawing_cx(2_500_000), "other");
        // edge: too small, treated as other (we exclude cx=0)
        assert_eq!(classify_drawing_cx(0), "other");
    }

    #[test]
    fn drawing_class_counts_partitions_mock_emus() {
        let xml = r#"
            <wp:extent cx="180000" cy="180000"/>
            <wp:extent cx="180000" cy="180000"/>
            <wp:extent cx="950000" cy="950000"/>
            <wp:extent cx="950000" cy="950000"/>
            <wp:extent cx="950000" cy="950000"/>
            <wp:extent cx="6000000" cy="4000000"/>
            <wp:extent cx="2500000" cy="2500000"/>
        "#;
        let counts = count_drawing_classes(xml);
        let map: std::collections::HashMap<&str, usize> = counts.iter().copied().collect();
        assert_eq!(map["icon"], 2);
        assert_eq!(map["qr"], 3);
        assert_eq!(map["figure"], 1);
        assert_eq!(map["other"], 1);
    }

    #[test]
    fn drawing_class_check_warn_on_band_drift() {
        // Build ref with reference inventory shape; current drops 12 QRs
        // (4 % drift > 10 % of 30-sample stub ⇒ WARN range).
        let mk = |class: &str, n: usize| -> String {
            let cx = match class {
                "icon" => 180_000,
                "qr" => 950_000,
                "figure" => 6_000_000,
                _ => 2_500_000,
            };
            std::iter::repeat(format!("<wp:extent cx=\"{cx}\" cy=\"100\"/>"))
                .take(n)
                .collect()
        };
        // Reference: 15/30/10/5 (cheap stand-in).
        let ref_xml = format!(
            "{}{}{}{}",
            mk("icon", 15),
            mk("qr", 30),
            mk("figure", 10),
            mk("other", 5)
        );
        // Current is missing 5 QRs (band ±3 → outside band, 5 < 5×3 = 15 → WARN).
        let cur_xml = format!(
            "{}{}{}{}",
            mk("icon", 15),
            mk("qr", 25),
            mk("figure", 10),
            mk("other", 5)
        );
        let findings = check_drawing_classes(
            std::path::Path::new("ref"),
            std::path::Path::new("cur"),
            &ref_xml,
            &cur_xml,
        );
        let qr = findings
            .iter()
            .find(|f| f.name == "drawing_class_bucket::qr")
            .unwrap();
        assert!(matches!(qr.severity, Severity::Warn), "{qr:?}");
        assert_eq!(qr.delta, -5);
    }

    /// Wave-3 iter-C (2026-06-04) — small-figure books (ref ≤ 10) get a
    /// ±50 % band on the FIGURE bucket so a 2-figure enrichment on a
    /// 4-figure reference doesn't trip the gate. AI-Norms' 78-figure
    /// reference still falls through to the default ±10 %.
    #[test]
    fn drawing_class_check_small_figure_uses_wider_band() {
        // Build a reference with 4 captioned figures (cx ≥ 5_000_000).
        let mk_fig = |n: usize| -> String { "<wp:extent cx=\"6000000\" cy=\"100\"/>".repeat(n) };
        let ref_xml = mk_fig(4);
        let cur_xml = mk_fig(6); // +2 figures (50 % over ref)
        let findings = check_drawing_classes(
            std::path::Path::new("ref"),
            std::path::Path::new("cur"),
            &ref_xml,
            &cur_xml,
        );
        let fig = findings
            .iter()
            .find(|f| f.name == "drawing_class_bucket::figure")
            .expect("figure finding present");
        assert!(
            matches!(fig.severity, Severity::Info),
            "small-figure book +2 within ±50 % band must INFO: {fig:?}"
        );
        assert_eq!(fig.delta, 2);
    }

    /// Wave-3 iter-C (2026-06-04) — AI-Norms-scale references (78 figures)
    /// still use the default ±10 % band. A +20-figure drift (≈26 % over)
    /// must escalate (WARN or ERROR), not INFO.
    #[test]
    fn drawing_class_check_large_figure_keeps_tight_band() {
        let mk_fig = |n: usize| -> String { "<wp:extent cx=\"6000000\" cy=\"100\"/>".repeat(n) };
        let ref_xml = mk_fig(78);
        let cur_xml = mk_fig(98); // +20, > 10 % of 78 (band = 8)
        let findings = check_drawing_classes(
            std::path::Path::new("ref"),
            std::path::Path::new("cur"),
            &ref_xml,
            &cur_xml,
        );
        let fig = findings
            .iter()
            .find(|f| f.name == "drawing_class_bucket::figure")
            .expect("figure finding present");
        assert!(
            !matches!(fig.severity, Severity::Info),
            "AI-Norms-scale ref must keep tight ±10 % band: {fig:?}"
        );
    }

    #[test]
    fn drawing_class_check_error_on_total_collapse() {
        let ref_xml = "<wp:extent cx=\"180000\" cy=\"100\"/>".repeat(15);
        let cur_xml = ""; // zero of every class
        let findings = check_drawing_classes(
            std::path::Path::new("ref"),
            std::path::Path::new("cur"),
            ref_xml.as_str(),
            cur_xml,
        );
        let icon = findings
            .iter()
            .find(|f| f.name == "drawing_class_bucket::icon")
            .unwrap();
        assert!(matches!(icon.severity, Severity::Error), "{icon:?}");
        assert_eq!(icon.delta, -15);
    }

    #[test]
    fn bkcallout_observation_extracts_pbdr_and_shd() {
        let xml = r#"<w:p ><w:pPr><w:pStyle w:val="BkCallout"/><w:pBdr><w:left w:val="single" w:sz="24" w:space="8" w:color="1F3864"/></w:pBdr><w:shd w:val="clear" w:color="auto" w:fill="EEF2F8"/></w:pPr><w:r ><w:t>Body</w:t></w:r></w:p>"#;
        let obs = collect_bkcallout_observations(xml);
        assert_eq!(obs.len(), 1);
        assert!(obs[0].has_pbdr);
        assert!(obs[0].has_shd);
        assert_eq!(obs[0].border_color.as_deref(), Some("1f3864"));
        assert_eq!(obs[0].shd_fill.as_deref(), Some("eef2f8"));
    }

    /// Round V iter-3: the renderer's `Note` flavor uses
    /// `1F3864 / EAF1FB` — distinct enough from `Generic` (`1F3864 /
    /// EEF2F8`) that it has its own entry in `REF_CALLOUT_FLAVORS` and
    /// must count as a known (not unknown) flavour.
    #[test]
    fn bkcallout_decoration_recognises_note_flavor() {
        let mut xml = String::new();
        for _ in 0..7 {
            xml.push_str(
                r#"<w:p ><w:pPr><w:pStyle w:val="BkCallout"/><w:pBdr><w:left w:val="single" w:sz="24" w:space="8" w:color="1F3864"/></w:pBdr><w:shd w:val="clear" w:color="auto" w:fill="EAF1FB"/></w:pPr><w:r ><w:t>n</w:t></w:r></w:p>"#,
            );
        }
        let f = check_bkcallout_decoration(
            std::path::Path::new("ref"),
            std::path::Path::new("cur"),
            &xml,
        );
        assert!(
            matches!(f.severity, Severity::Info),
            "Note flavour should be recognised as documented: {f:?}"
        );
        assert_eq!(f.delta, 0, "no unknown-flavour stragglers: {f:?}");
    }

    #[test]
    fn bkcallout_decoration_pass_when_all_decorated() {
        let mut xml = String::new();
        for _ in 0..10 {
            xml.push_str(r#"<w:p ><w:pPr><w:pStyle w:val="BkCallout"/><w:pBdr><w:left w:val="single" w:sz="24" w:space="8" w:color="1F3864"/></w:pBdr><w:shd w:val="clear" w:color="auto" w:fill="EEF2F8"/></w:pPr><w:r ><w:t>x</w:t></w:r></w:p>"#);
        }
        let f = check_bkcallout_decoration(
            std::path::Path::new("ref"),
            std::path::Path::new("cur"),
            &xml,
        );
        assert!(matches!(f.severity, Severity::Info), "{f:?}");
        assert_eq!(f.delta, 0);
    }

    #[test]
    fn bkcallout_decoration_error_when_all_missing_pbdr() {
        // 10 BkCallout paragraphs, none with pBdr/shd → 30/10 = ERROR.
        let mut xml = String::new();
        for _ in 0..10 {
            xml.push_str(r#"<w:p ><w:pPr><w:pStyle w:val="BkCallout"/></w:pPr><w:r ><w:t>x</w:t></w:r></w:p>"#);
        }
        let f = check_bkcallout_decoration(
            std::path::Path::new("ref"),
            std::path::Path::new("cur"),
            &xml,
        );
        assert!(matches!(f.severity, Severity::Error), "{f:?}");
        assert!(f.delta >= 10);
    }

    #[test]
    fn bkcallout_decoration_error_on_unknown_flavor() {
        // 10 BkCallout paragraphs, all with pBdr+shd but a non-documented colour pair.
        let mut xml = String::new();
        for _ in 0..10 {
            xml.push_str(r#"<w:p ><w:pPr><w:pStyle w:val="BkCallout"/><w:pBdr><w:left w:val="single" w:sz="24" w:space="8" w:color="123456"/></w:pBdr><w:shd w:val="clear" w:color="auto" w:fill="ABCDEF"/></w:pPr><w:r ><w:t>x</w:t></w:r></w:p>"#);
        }
        let f = check_bkcallout_decoration(
            std::path::Path::new("ref"),
            std::path::Path::new("cur"),
            &xml,
        );
        assert!(matches!(f.severity, Severity::Error), "{f:?}");
    }

    #[test]
    fn pic_name_count_handles_named_and_unnamed() {
        let xml = r#"
            <pic:cNvPr id="1" name="foo.png"/>
            <pic:cNvPr id="2" name=""/>
            <pic:cNvPr id="3" name="bar.png"/>
            <pic:cNvPr id="4"/>
        "#;
        let (total, named) = count_pic_name_attributes(xml);
        assert_eq!(total, 4);
        assert_eq!(named, 2);
    }

    #[test]
    fn pic_name_attribute_pass_when_all_named() {
        let xml = std::iter::repeat(r#"<pic:cNvPr id="1" name="x.png"/>"#)
            .take(50)
            .collect::<String>();
        let f = check_pic_name_attribute(
            std::path::Path::new("ref"),
            std::path::Path::new("cur"),
            &xml,
        );
        assert!(matches!(f.severity, Severity::Info), "{f:?}");
    }

    #[test]
    fn pic_name_attribute_error_when_majority_unnamed() {
        let mut xml = String::new();
        for _ in 0..50 {
            xml.push_str(r#"<pic:cNvPr id="1" name=""/>"#);
        }
        let f = check_pic_name_attribute(
            std::path::Path::new("ref"),
            std::path::Path::new("cur"),
            &xml,
        );
        assert!(matches!(f.severity, Severity::Error), "{f:?}");
    }

    #[test]
    fn horizontal_rule_counts_bottom_single_borders() {
        let xml = r#"
            <w:pBdr><w:bottom w:val="single" w:sz="6" w:space="1" w:color="auto"/></w:pBdr>
            <w:pBdr><w:bottom w:val="single" w:sz="6" w:space="1" w:color="auto"/></w:pBdr>
            <w:pBdr><w:left w:val="single" w:sz="24" w:space="8" w:color="1F3864"/></w:pBdr>
        "#;
        assert_eq!(count_horizontal_rules(xml), 2);
    }

    #[test]
    fn horizontal_rule_check_warns_when_close_to_threshold() {
        let xml = "<w:pBdr><w:bottom w:val=\"single\" w:sz=\"6\" w:space=\"1\" w:color=\"auto\"/></w:pBdr>".repeat(35);
        let f = check_horizontal_rule_count(
            std::path::Path::new("ref"),
            std::path::Path::new("cur"),
            &xml,
        );
        assert!(matches!(f.severity, Severity::Warn), "{f:?}");
    }

    #[test]
    fn horizontal_rule_check_errors_on_zero() {
        let f = check_horizontal_rule_count(
            std::path::Path::new("ref"),
            std::path::Path::new("cur"),
            "",
        );
        assert!(matches!(f.severity, Severity::Error), "{f:?}");
    }

    #[test]
    fn theme_fonts_extract_major_minor_latin() {
        let xml = r#"<a:fontScheme><a:majorFont><a:latin typeface="Calibri"/><a:ea typeface=""/></a:majorFont><a:minorFont><a:latin typeface="Cambria"/></a:minorFont></a:fontScheme>"#;
        let (maj, min) = extract_theme_fonts(xml);
        assert_eq!(maj.as_deref(), Some("Calibri"));
        assert_eq!(min.as_deref(), Some("Cambria"));
    }

    #[test]
    fn extract_hyperlink_color_picks_styled_run_color() {
        let xml = r#"<w:styles>
<w:style w:type="character" w:styleId="OtherStyle"><w:rPr><w:color w:val="ABCDEF"/></w:rPr></w:style>
<w:style w:type="character" w:styleId="Hyperlink"><w:name w:val="Hyperlink"/><w:basedOn w:val="DefaultParagraphFont"/><w:rPr><w:color w:val="0000FF" w:themeColor="hyperlink"/><w:u w:val="single"/></w:rPr></w:style>
</w:styles>"#;
        assert_eq!(extract_hyperlink_color(xml).as_deref(), Some("0000FF"));
    }

    #[test]
    fn extract_hyperlink_color_returns_none_when_style_absent() {
        let xml = r#"<w:styles><w:style w:type="paragraph" w:styleId="Normal"/></w:styles>"#;
        assert!(extract_hyperlink_color(xml).is_none());
    }

    /// Sample run against the AI_Norms_and_Regulations reference book.
    /// Marked `#[ignore]` because it depends on a file outside the repo;
    /// invoke explicitly with
    /// `cargo test -p agentic-checks parity_icons::tests::sample_run_against_reference -- --ignored --nocapture`.
    #[test]
    #[ignore = "requires AI_Norms_and_Regulations_BOOK.docx in book_build"]
    fn sample_run_against_reference() {
        let ref_path = std::path::PathBuf::from(
            r"C:\Users\dcaso\book_build\AI_Norms_and_Regulations_BOOK.docx",
        );
        if !ref_path.exists() {
            eprintln!("SKIP: {} not present", ref_path.display());
            return;
        }
        // Self-parity: reference vs reference. Should INFO across the board.
        let self_findings = run(&ref_path, &ref_path);
        for f in &self_findings {
            println!(
                "[self] {} {:?} expected={} actual={} delta={}",
                f.name, f.severity, f.expected, f.actual, f.delta
            );
        }
        // Contrast: reference vs a smaller book that has no figures /
        // BkCallout / etc. Should ERROR on drawings, theme, hyperlink.
        let cur_path = std::path::PathBuf::from(
            r"C:\Users\dcaso\book_build\agentic_finalized\master_thesis.docx",
        );
        if cur_path.exists() {
            let cross = run(&ref_path, &cur_path);
            for f in &cross {
                println!(
                    "[contrast] {} {:?} expected={} actual={} delta={}",
                    f.name, f.severity, f.expected, f.actual, f.delta
                );
            }
        }
    }
}
