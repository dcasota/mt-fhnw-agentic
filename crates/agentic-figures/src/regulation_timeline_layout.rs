//! Layout orchestration for the regulation-timeline figure.
//!
//! Combines the individual panel renderers (A, B, C) into the
//! composite PNGs the kit produces — primarily `render_abc`, which is
//! what the thesis §2.3 actually uses
//! (`regulation_timeline_v3_ABC.png`).
//!
//! Height ratios mirror the python kit's
//! `_render_regulation_timeline_v3.py` `render_abc` (lines 1261-1290):
//! `A_h = 2.4`, `B_h = 0.55*n_goals + 0.4`, `C_h = 0.46*n_conflict + 0.9`
//! — these are matplotlib inches; we translate to pixels at the
//! canvas-height level so the proportions match.

#![allow(dead_code)]

use std::path::Path;

use anyhow::{Context, Result, anyhow};
use plotters::prelude::*;
use plotters::style::text_anchor::{HPos, Pos, VPos};

use crate::regulation_timeline::{CONFLICTS, GOAL_KEYS, REGS, t};
use crate::regulation_timeline_panel_a::render_panel_a;
use crate::regulation_timeline_panel_b::render_panel_b;
use crate::regulation_timeline_panel_c::render_panel_c;

// ============================================================================
// Canvas + layout constants.
// ============================================================================

/// Canvas width. 1450 matches the per-panel standalone width so each
/// sub-region drops in without rescaling.
pub const W: u32 = 1450;

/// Title strip height at the top of the composite (`suptitle`).
const TITLE_H: i32 = 36;
/// Footer strip height at the bottom (legend + cheat-sheet box).
const FOOTER_H: i32 = 12;
/// Inter-panel vertical gap (python's `hspace` analogue).
const PANEL_GAP: i32 = 4;
/// Side padding (left/right of the canvas).
const SIDE_PAD: i32 = 0;

const INK: RGBColor = RGBColor(0x1A, 0x1A, 0x1A);

// ============================================================================
// Public entry points.
// ============================================================================

/// Render the A+B+C composite figure (matches the kit's
/// `regulation_timeline_v3_ABC.png`). Used by the thesis §2.3.
pub fn render_abc(out_png: &Path, lang: &str) -> Result<()> {
    let (h, regions) = compute_geometry();
    let root = BitMapBackend::new(out_png, (W, h)).into_drawing_area();
    root.fill(&WHITE).map_err(|e| anyhow!("fill: {e}"))?;

    // Suptitle band at the top.
    draw_suptitle(&root, lang)?;

    // Three panel sub-regions.
    render_panel_a(
        &root,
        lang,
        SIDE_PAD,
        regions.panel_a_y0,
        W as i32 - SIDE_PAD,
        regions.panel_a_y1,
    )
    .with_context(|| "compose panel A")?;
    render_panel_b(
        &root,
        lang,
        SIDE_PAD,
        regions.panel_b_y0,
        W as i32 - SIDE_PAD,
        regions.panel_b_y1,
    )
    .with_context(|| "compose panel B")?;
    render_panel_c(
        &root,
        lang,
        SIDE_PAD,
        regions.panel_c_y0,
        W as i32 - SIDE_PAD,
        regions.panel_c_y1,
    )
    .with_context(|| "compose panel C")?;

    root.present().map_err(|e| anyhow!("present: {e}"))?;
    Ok(())
}

/// Render the A+B composite (matches the kit's `regulation_timeline_v3_AB.png`).
/// Useful when the reader doesn't need Panel C's per-regulation Gantt.
pub fn render_ab(out_png: &Path, lang: &str) -> Result<()> {
    let (a_h, b_h, _c_h) = panel_heights();
    let title_h = TITLE_H;
    let h = (title_h + a_h + PANEL_GAP + b_h + FOOTER_H) as u32;
    let root = BitMapBackend::new(out_png, (W, h)).into_drawing_area();
    root.fill(&WHITE).map_err(|e| anyhow!("fill: {e}"))?;

    draw_suptitle(&root, lang)?;

    let panel_a_y0 = title_h;
    let panel_a_y1 = panel_a_y0 + a_h;
    let panel_b_y0 = panel_a_y1 + PANEL_GAP;
    let panel_b_y1 = panel_b_y0 + b_h;
    render_panel_a(
        &root,
        lang,
        SIDE_PAD,
        panel_a_y0,
        W as i32 - SIDE_PAD,
        panel_a_y1,
    )
    .with_context(|| "compose panel A")?;
    render_panel_b(
        &root,
        lang,
        SIDE_PAD,
        panel_b_y0,
        W as i32 - SIDE_PAD,
        panel_b_y1,
    )
    .with_context(|| "compose panel B")?;
    root.present().map_err(|e| anyhow!("present: {e}"))?;
    Ok(())
}

/// Render Panel C as a standalone composite (matches the kit's
/// `regulation_timeline_v3_C.png`). Includes the suptitle.
pub fn render_c_only(out_png: &Path, lang: &str) -> Result<()> {
    let (_a_h, _b_h, c_h) = panel_heights();
    let h = (TITLE_H + c_h + FOOTER_H) as u32;
    let root = BitMapBackend::new(out_png, (W, h)).into_drawing_area();
    root.fill(&WHITE).map_err(|e| anyhow!("fill: {e}"))?;
    draw_suptitle(&root, lang)?;
    render_panel_c(
        &root,
        lang,
        SIDE_PAD,
        TITLE_H,
        W as i32 - SIDE_PAD,
        TITLE_H + c_h,
    )
    .with_context(|| "compose panel C")?;
    root.present().map_err(|e| anyhow!("present: {e}"))?;
    Ok(())
}

// ============================================================================
// Geometry.
// ============================================================================

struct AbcRegions {
    panel_a_y0: i32,
    panel_a_y1: i32,
    panel_b_y0: i32,
    panel_b_y1: i32,
    panel_c_y0: i32,
    panel_c_y1: i32,
}

/// Heights for panels A, B, C in pixels. Derived from the python
/// matplotlib inches with a 28 inch -> 1450 px width scaling
/// (~51.8 px per inch).
fn panel_heights() -> (i32, i32, i32) {
    let n_goals = GOAL_KEYS.len() as f64;
    let n_conflict = conflict_subset_count() as f64;
    // python: A_h = 2.4, B_h = 0.55 * n_goals + 0.4, C_h = 0.46 * n_conflict + 0.9
    // Scale: 28 in canvas -> 1450 px width gives ~51.8 px/in. The
    // per-inch height should match to preserve aspect, so use the
    // same scale.
    let pxpi: f64 = W as f64 / 28.0;
    let a = (2.4 * pxpi).round() as i32;
    let b = ((0.55 * n_goals + 0.4) * pxpi).round() as i32;
    let c = ((0.46 * n_conflict + 0.9) * pxpi).round() as i32;
    (a, b, c)
}

fn compute_geometry() -> (u32, AbcRegions) {
    let (a_h, b_h, c_h) = panel_heights();
    let title_h = TITLE_H;
    let total = title_h + a_h + PANEL_GAP + b_h + PANEL_GAP + c_h + FOOTER_H;
    let panel_a_y0 = title_h;
    let panel_a_y1 = panel_a_y0 + a_h;
    let panel_b_y0 = panel_a_y1 + PANEL_GAP;
    let panel_b_y1 = panel_b_y0 + b_h;
    let panel_c_y0 = panel_b_y1 + PANEL_GAP;
    let panel_c_y1 = panel_c_y0 + c_h;
    (
        total as u32,
        AbcRegions {
            panel_a_y0,
            panel_a_y1,
            panel_b_y0,
            panel_b_y1,
            panel_c_y0,
            panel_c_y1,
        },
    )
}

fn conflict_subset_count() -> usize {
    use std::collections::BTreeSet;
    let labels: BTreeSet<&str> = CONFLICTS.iter().map(|c| c.label).collect();
    REGS.iter().filter(|r| labels.contains(r.label)).count()
}

fn draw_suptitle(
    area: &DrawingArea<BitMapBackend<'_>, plotters::coord::Shift>,
    lang: &str,
) -> Result<()> {
    let style = ("sans-serif", 14)
        .into_font()
        .style(FontStyle::Bold)
        .color(&INK)
        .pos(Pos::new(HPos::Center, VPos::Center));
    area.draw_text(t(lang, "suptitle"), &style, (W as i32 / 2, TITLE_H / 2))
        .map_err(|e| anyhow!("suptitle: {e}"))?;
    Ok(())
}

// ============================================================================
// Tests.
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn panel_heights_track_python_ratios() {
        let (a, b, c) = panel_heights();
        // Expected at 51.8 px/in (1450 px / 28 in):
        // A = 2.4 × 51.8 ≈ 124, B = 7.0 × 51.8 ≈ 363, C = 5.04 × 51.8 ≈ 261.
        assert!((100..=160).contains(&a), "A height out of band: {a}");
        assert!((320..=420).contains(&b), "B height out of band: {b}");
        assert!((220..=320).contains(&c), "C height out of band: {c}");
        // Ordering invariant: A < C < B (B dominates because of 12 goal rows).
        assert!(a < c && c < b, "expected A < C < B: {a} < {c} < {b}");
    }

    #[test]
    fn render_abc_writes_non_trivial_png_en() {
        let tmp = TempDir::new().unwrap();
        let out = tmp.path().join("abc_en.png");
        render_abc(&out, "en").expect("EN ABC render");
        let bytes = std::fs::read(&out).unwrap();
        assert_eq!(&bytes[0..4], &[0x89, 0x50, 0x4E, 0x47]);
        // ABC = title + Panel A + Panel B + Panel C — should be
        // ≥ 25 KB (each panel standalone is ≥ 8-10 KB).
        assert!(
            bytes.len() > 25_000,
            "ABC PNG suspiciously small: {} bytes",
            bytes.len()
        );
    }

    #[test]
    fn render_abc_writes_non_trivial_png_de() {
        let tmp = TempDir::new().unwrap();
        let out = tmp.path().join("abc_de.png");
        render_abc(&out, "de").expect("DE ABC render");
        let size = std::fs::metadata(&out).unwrap().len();
        assert!(size > 25_000);
    }

    #[test]
    fn render_abc_writes_non_trivial_png_hi() {
        let tmp = TempDir::new().unwrap();
        let out = tmp.path().join("abc_hi.png");
        render_abc(&out, "hi").expect("HI ABC render");
        let size = std::fs::metadata(&out).unwrap().len();
        assert!(size > 25_000);
    }

    #[test]
    fn render_ab_writes_non_trivial_png_en() {
        let tmp = TempDir::new().unwrap();
        let out = tmp.path().join("ab_en.png");
        render_ab(&out, "en").expect("EN AB render");
        let size = std::fs::metadata(&out).unwrap().len();
        assert!(size > 15_000);
    }

    #[test]
    fn render_c_only_writes_non_trivial_png_en() {
        let tmp = TempDir::new().unwrap();
        let out = tmp.path().join("c_en.png");
        render_c_only(&out, "en").expect("EN C-only render");
        let size = std::fs::metadata(&out).unwrap().len();
        assert!(size > 10_000);
    }
}
