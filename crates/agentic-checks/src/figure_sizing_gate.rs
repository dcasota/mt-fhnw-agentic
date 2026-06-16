//! `agentic check figure-sizing` — verify every rendered figure PNG would
//! display at ≥ the on-page font-size floor (8.0 pt as of 2026-06-16).
//!
//! Background. The 2026-06-16 readability brief found that flow figures
//! with many nodes and long labels (canonical: `figPTC081_01` in the
//! Campaign 08 book — 7 layers × ~280 px boxes × ~220 px gaps ≈ 3 344 px
//! source canvas) were being Lanczos-clamped down to the ≤ landscape-cap
//! PNG width and rendering box text at ~2.6 pt on the printed page,
//! well below the readability floor. The renderer-side fix (canvas
//! budget in `render_flow` / forced Landscape for `render_quadrant` and
//! `render_matrix`) keeps the source canvas ≤ landscape cap so the
//! post-clamp shrink can't compound. This gate is the deterministic
//! verifier that no rendered PNG can violate the floor:
//!
//!   * `FIGURE_SIZING_OVER_CAP`     — PNG width exceeds the Landscape
//!     cap (`max_png_width_px(Landscape) = 1100` at the 8.0 pt floor) —
//!     ERROR. With the renderer changes this should never fire; if it
//!     does the renderer regressed.
//!   * `FIGURE_SIZING_NEEDS_LAND`   — PNG width exceeds the Portrait
//!     cap (691 px) but the markdown reference does not carry the
//!     `#landscape` fragment, so docx would embed it at the Portrait
//!     body width and the 13 pt source text would land below the floor
//!     — ERROR.
//!   * `FIGURE_SIZING_SUB_FLOOR`    — computed on-page pt for the
//!     rendered PNG is below `MIN_ON_PAGE_PT` (8.0) under the most
//!     generous assumption (Landscape body width) — ERROR.
//!   * `FIGURE_SIZING_SUMMARY`      — INFO summary with #PNGs scanned
//!     and the worst observed on-page pt.

use std::collections::HashMap;
use std::path::Path;
use std::sync::LazyLock;

use anyhow::Result;
use regex::Regex;
use rusqlite::Connection;

use agentic_figures::{Layout, expected_on_page_pt, on_page_pt_floor, renderer_min_pt};

use crate::{CheckReport, Finding, Severity};

/// Markdown image syntax. Captures `(alt, src)`; `src` may carry a
/// `#landscape` URL fragment that signals the docx exporter to rotate
/// the page for this image.
static IMG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"!\[([^\]]*)\]\(([^)]+)\)").unwrap());

/// Portrait PNG cap (back-solved from the 8.0 pt on-page floor + 13 pt
/// renderer text at 5.91 in body width). Hard-coded to avoid making
/// `max_png_width_px` public for downstream consumers; the figure-
/// sizing gate is the only crate that needs this number.
const PORTRAIT_CAP_PX: u32 = 691;
/// Landscape PNG cap (back-solved as `PORTRAIT_CAP_PX × 9.41 / 5.91`,
/// truncated). PNGs wider than this fail the gate outright.
const LANDSCAPE_CAP_PX: u32 = 1100;

/// One image-embed reference parsed from a source markdown blob.
#[derive(Debug, Clone)]
struct ImgRef {
    /// File path of the source markdown that contains the reference.
    src_path: String,
    /// 1-indexed line number of the markdown image syntax.
    src_line: usize,
    /// The image src verbatim, including any `#fragment` suffix.
    src: String,
    /// `true` iff `src` carries the `#landscape` URL fragment.
    is_landscape: bool,
    /// Bare PNG basename (`fig_xy.png`) — the key used to dedupe and
    /// to match against PNGs found on disk.
    basename: String,
}

fn parse_image_refs(text: &str, path: &str) -> Vec<ImgRef> {
    let mut out = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        for cap in IMG.captures_iter(line) {
            let src = cap.get(2).map_or("", |m| m.as_str()).to_string();
            let (path_part, frag) = src.split_once('#').unwrap_or((src.as_str(), ""));
            let basename = std::path::Path::new(path_part)
                .file_name()
                .and_then(std::ffi::OsStr::to_str)
                .unwrap_or(path_part)
                .to_string();
            out.push(ImgRef {
                src_path: path.to_string(),
                src_line: idx + 1,
                src: src.clone(),
                is_landscape: frag.eq_ignore_ascii_case("landscape"),
                basename,
            });
        }
    }
    out
}

/// One audited PNG with its measured width + the layout it will be
/// embedded with (resolved from the source-markdown reference).
#[derive(Debug, Clone)]
struct AuditedPng {
    abs_path: String,
    basename: String,
    width_px: u32,
    /// Layout the docx exporter will use when embedding this PNG.
    /// Resolved from the `#landscape` fragment on the markdown
    /// reference; defaults to Portrait when the reference is absent.
    effective_layout: Layout,
    /// `true` if any source markdown references this PNG with the
    /// `#landscape` URL fragment.
    referenced_landscape: bool,
    /// `true` if any source markdown references this PNG at all.
    referenced: bool,
}

fn read_png_width(path: &Path) -> Option<u32> {
    let img = image::open(path).ok()?;
    Some(img.width())
}

/// Compute the worst-case (smallest) on-page font for this PNG. Uses
/// the *embedding* layout (Portrait unless the markdown reference
/// carries `#landscape`) because the docx exporter goes by the URL
/// fragment, not by the PNG's natural aspect ratio.
fn worst_on_page_pt(p: &AuditedPng) -> f64 {
    expected_on_page_pt(p.effective_layout, p.width_px, renderer_min_pt())
}

/// Build the per-PNG audit table. Walks `out/figures/**.png` on disk
/// (the rendered cache produced by `agentic cascade run`), measures
/// each PNG's width, and joins against the markdown references so the
/// `#landscape` fragment is observable.
fn audit_pngs(
    root: &Path,
    image_refs: &[ImgRef],
) -> Result<Vec<AuditedPng>> {
    let mut by_basename: HashMap<String, AuditedPng> = HashMap::new();
    // Index references by basename → (any_landscape, any_ref)
    let mut ref_landscape: HashMap<String, bool> = HashMap::new();
    let mut ref_seen: HashMap<String, bool> = HashMap::new();
    for r in image_refs {
        ref_seen.insert(r.basename.clone(), true);
        let prior = ref_landscape.get(&r.basename).copied().unwrap_or(false);
        ref_landscape.insert(r.basename.clone(), prior || r.is_landscape);
    }
    let figures_root = root.join("out").join("figures");
    if !figures_root.exists() {
        return Ok(Vec::new());
    }
    walk_pngs(&figures_root, &mut |abs_path| {
        let basename = abs_path
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or("")
            .to_string();
        let Some(width_px) = read_png_width(abs_path) else {
            return;
        };
        let referenced = ref_seen.get(&basename).copied().unwrap_or(false);
        let referenced_landscape = ref_landscape.get(&basename).copied().unwrap_or(false);
        let effective_layout = if referenced_landscape {
            Layout::Landscape
        } else {
            Layout::Portrait
        };
        by_basename.insert(
            basename.clone(),
            AuditedPng {
                abs_path: abs_path.to_string_lossy().to_string(),
                basename,
                width_px,
                effective_layout,
                referenced_landscape,
                referenced,
            },
        );
    });
    Ok(by_basename.into_values().collect())
}

fn walk_pngs(dir: &Path, cb: &mut dyn FnMut(&Path)) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_dir() {
            walk_pngs(&p, cb);
        } else if p.extension().and_then(std::ffi::OsStr::to_str) == Some("png") {
            cb(&p);
        }
    }
}

/// Collect findings for one PNG against the on-page floor.
fn sizing_findings(p: &AuditedPng) -> Vec<Finding> {
    let mut out = Vec::new();
    let floor = on_page_pt_floor();
    if p.width_px > LANDSCAPE_CAP_PX {
        out.push(Finding {
            category: "FIGURE_SIZING_OVER_CAP".into(),
            severity: Severity::Error,
            message: format!(
                "PNG width {} px exceeds Landscape cap {} px — even landscape rotation cannot bring on-page text to {:.1} pt; renderer canvas-budget regression",
                p.width_px, LANDSCAPE_CAP_PX, floor
            ),
            location: Some(format!("{} ({})", p.basename, p.abs_path)),
        });
        return out;
    }
    if p.width_px > PORTRAIT_CAP_PX && !p.referenced_landscape && p.referenced {
        out.push(Finding {
            category: "FIGURE_SIZING_NEEDS_LAND".into(),
            severity: Severity::Error,
            message: format!(
                "PNG width {} px > Portrait cap {} px but the markdown reference does not carry the #landscape fragment; docx will embed at Portrait body width and text will land at {:.2} pt (< {:.1} pt floor)",
                p.width_px,
                PORTRAIT_CAP_PX,
                expected_on_page_pt(Layout::Portrait, p.width_px, renderer_min_pt()),
                floor,
            ),
            location: Some(format!("{} ({})", p.basename, p.abs_path)),
        });
    }
    let on_pt = worst_on_page_pt(p);
    if on_pt < floor && p.referenced {
        out.push(Finding {
            category: "FIGURE_SIZING_SUB_FLOOR".into(),
            severity: Severity::Error,
            message: format!(
                "on-page text would render at {on_pt:.2} pt (< {floor:.1} pt floor) at width {} px under {:?} layout",
                p.width_px, p.effective_layout,
            ),
            location: Some(format!("{} ({})", p.basename, p.abs_path)),
        });
    }
    out
}

/// Run the figure-sizing gate over the project's worktree.
pub fn run(conn: &Connection, project: &str, root: &Path) -> Result<CheckReport> {
    use agentic_core::worktree;
    let mut image_refs = Vec::new();
    for (path, sha) in worktree::list(conn, project, agentic_core::paths::SOURCES_PREFIX)? {
        if !path.ends_with(".md") {
            continue;
        }
        let Ok(blob) = agentic_core::content::blob::get_blob(conn, &sha) else {
            continue;
        };
        let text = String::from_utf8_lossy(&blob.content);
        image_refs.extend(parse_image_refs(&text, &path));
    }

    let audited = audit_pngs(root, &image_refs)?;
    let total = audited.len();
    let mut findings = Vec::new();
    let mut worst_pt = f64::INFINITY;
    let mut worst_who: Option<String> = None;
    for p in &audited {
        let on = worst_on_page_pt(p);
        if p.referenced && on < worst_pt {
            worst_pt = on;
            worst_who = Some(p.basename.clone());
        }
        findings.extend(sizing_findings(p));
    }
    let worst_disp = if worst_pt.is_finite() {
        format!(
            "{worst_pt:.2} pt ({})",
            worst_who.as_deref().unwrap_or("<unknown>")
        )
    } else {
        "n/a (no referenced PNGs found)".to_string()
    };
    findings.push(Finding {
        category: "FIGURE_SIZING_SUMMARY".into(),
        severity: Severity::Info,
        message: format!(
            "{total} PNG(s) audited; floor = {:.1} pt (Portrait cap = {} px / Landscape cap = {} px); worst on-page pt = {worst_disp}",
            on_page_pt_floor(),
            PORTRAIT_CAP_PX,
            LANDSCAPE_CAP_PX
        ),
        location: Some("figure_sizing".into()),
    });
    Ok(CheckReport::new("figure_sizing", findings))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn over_cap_png_is_error() {
        let p = AuditedPng {
            abs_path: "out/figures/x/big.png".into(),
            basename: "big.png".into(),
            width_px: 2000,
            effective_layout: Layout::Landscape,
            referenced_landscape: true,
            referenced: true,
        };
        let f = sizing_findings(&p);
        assert!(f.iter().any(|x| x.category == "FIGURE_SIZING_OVER_CAP"
            && matches!(x.severity, Severity::Error)));
    }

    #[test]
    fn portrait_wide_without_landscape_frag_is_error() {
        let p = AuditedPng {
            abs_path: "out/figures/x/wide.png".into(),
            basename: "wide.png".into(),
            width_px: 1000,
            effective_layout: Layout::Portrait,
            referenced_landscape: false,
            referenced: true,
        };
        let f = sizing_findings(&p);
        assert!(f
            .iter()
            .any(|x| x.category == "FIGURE_SIZING_NEEDS_LAND"));
    }

    #[test]
    fn landscape_at_cap_is_ok() {
        let p = AuditedPng {
            abs_path: "out/figures/x/cap.png".into(),
            basename: "cap.png".into(),
            width_px: 1100,
            effective_layout: Layout::Landscape,
            referenced_landscape: true,
            referenced: true,
        };
        let f = sizing_findings(&p);
        assert!(f.iter().all(|x| !matches!(x.severity, Severity::Error)));
    }

    #[test]
    fn unreferenced_png_is_silent() {
        let p = AuditedPng {
            abs_path: "out/figures/x/orphan.png".into(),
            basename: "orphan.png".into(),
            width_px: 2000,
            effective_layout: Layout::Portrait,
            referenced_landscape: false,
            referenced: false,
        };
        // Over-cap still flags (renderer regression), but sub-floor /
        // needs-land DO NOT fire on unreferenced PNGs because they
        // don't reach a rendered docx.
        let f = sizing_findings(&p);
        assert!(f
            .iter()
            .any(|x| x.category == "FIGURE_SIZING_OVER_CAP"));
        assert!(f
            .iter()
            .all(|x| x.category != "FIGURE_SIZING_NEEDS_LAND"
                && x.category != "FIGURE_SIZING_SUB_FLOOR"));
    }

    #[test]
    fn parses_landscape_fragment() {
        let md = "x\n\n![cap](out/figures/sub/foo.png#landscape)\n\n![cap2](out/figures/sub/bar.png)\n";
        let refs = parse_image_refs(md, "c.md");
        assert_eq!(refs.len(), 2);
        assert!(refs[0].is_landscape);
        assert!(!refs[1].is_landscape);
        assert_eq!(refs[0].basename, "foo.png");
        assert_eq!(refs[1].basename, "bar.png");
    }
}
