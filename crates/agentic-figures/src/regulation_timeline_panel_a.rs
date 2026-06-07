//! Panel A — enforcement starts per year, stacked by jurisdiction.
//!
//! Port of the python kit's `render_panel_A` (lines 985-1009 of
//! `_render_regulation_timeline_v3.py` v_130732). Renders the top
//! panel of the regulation-timeline figure: a stacked vertical bar
//! per year, coloured by jurisdiction, with count labels above bars
//! of height ≥ 3 and a "today" vertical line at 2026.42.
//!
//! This commit ships the renderer with **structural** parity to the
//! python (axis range, grid, today line, stacked bars, count labels);
//! pixel-perfect parity vs the kit's reference PNG comes in a
//! follow-up commit once we have the full 3-panel layout to compare
//! against.

#![allow(dead_code)] // Public entry points wired via lib.rs in a follow-up.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result, anyhow};
use plotters::prelude::*;
use plotters::style::text_anchor::{HPos, Pos, VPos};

use crate::regulation_timeline::{REGS, X_HI, X_LO, colour_for, t};

// ============================================================================
// Layout constants (panel-relative; the caller supplies the bounding box).
// ============================================================================

/// Years of empty space appended to the right of `X_HI` so Panel B's
/// per-row annotation has somewhere to live without overwriting bars.
/// Mirrors the python `right_pad=22`.
pub const RIGHT_PAD_YEARS: i32 = 22;

/// Default standalone-render canvas size. Aspect tracks the Panel-A
/// strip in the kit's `regulation_timeline_v3.png` (the top ~18 % of
/// a ~1440×3500 figure).
pub const STANDALONE_W: u32 = 1450;
pub const STANDALONE_H: u32 = 220;

// Inner padding of the panel (pixels between bounding-box edge and
// the plot area where bars are drawn).
const PAD_LEFT: i32 = 140;
const PAD_RIGHT: i32 = 40;
const PAD_TOP: i32 = 28;
const PAD_BOTTOM: i32 = 18;

/// Jurisdiction stacking order (matches the python's iteration order
/// — drives the legend order and the colour stripe stacking).
const JUR_ORDER: &[&str] = &["EU", "US", "DE", "FR", "CH", "IN", "Intl", "Global"];

/// Pre-defined cue colours (kept distinct from jurisdiction palette).
const GRID_LIGHT: RGBColor = RGBColor(0xEA, 0xEA, 0xEA);
const GRID_5YR: RGBColor = RGBColor(0xA0, 0xA0, 0xA0);
const TODAY_LINE: RGBColor = RGBColor(0x22, 0x22, 0x22);
const RIGHT_EDGE: RGBColor = RGBColor(0x88, 0x88, 0x88);
const INK: RGBColor = RGBColor(0x1A, 0x1A, 0x1A);
const LABEL_GREY: RGBColor = RGBColor(0x33, 0x33, 0x33);

// The 2026.42 "today" anchor matches the python — late-May / early-June 2026.
const TODAY_X_VAL: f64 = 2026.42;

// ============================================================================
// Public entry points.
// ============================================================================

/// Render Panel A as a standalone PNG (no surrounding panels). Useful
/// for incremental development + visual diff against the top region of
/// the kit's reference `regulation_timeline_v3.png`.
pub fn render_panel_a_only(out_png: &Path, lang: &str) -> Result<()> {
    let root = BitMapBackend::new(out_png, (STANDALONE_W, STANDALONE_H)).into_drawing_area();
    root.fill(&WHITE)
        .map_err(|e| anyhow!("fill canvas: {e}"))?;
    render_panel_a(
        &root,
        lang,
        0,
        0,
        STANDALONE_W as i32,
        STANDALONE_H as i32,
    )
    .with_context(|| "render_panel_a")?;
    root.present()
        .map_err(|e| anyhow!("present: {e}"))?;
    Ok(())
}

/// Render Panel A into the given pixel-space bounding box on `area`.
/// The caller is responsible for layout (the 3-panel single mode and
/// the split mode compose this on top of a parent canvas).
pub fn render_panel_a(
    area: &DrawingArea<BitMapBackend<'_>, plotters::coord::Shift>,
    lang: &str,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
) -> Result<()> {
    let geom = Geometry::new(x0, y0, x1, y1);

    // 1. Aggregate REGS by (applies_year, jurisdiction). The applies
    //    field is the python tuple index 4 — "first year in force".
    let mut year_counts: HashMap<i32, HashMap<&'static str, u32>> = HashMap::new();
    for r in REGS {
        *year_counts
            .entry(r.applies_year)
            .or_default()
            .entry(r.jur)
            .or_insert(0) += 1;
    }

    // Tallest bar drives the Y axis. Add a small headroom (0.8) so the
    // count label above the tallest bar has space — matches python.
    let max_total: u32 = (X_LO..=X_HI)
        .map(|y| year_counts.get(&y).map_or(0, |m| m.values().sum::<u32>()))
        .max()
        .unwrap_or(1)
        .max(1);
    let y_top_val = max_total as f64 + 0.8;

    // 2. Background grid (every-year light, every-5-yr darker) plus
    //    the today + right-edge anchors.
    draw_grid(area, &geom)?;
    draw_today_dashed(area, &geom)?;
    draw_right_edge(area, &geom)?;

    // 3. Title.
    let title_style = ("sans-serif", 13)
        .into_font()
        .style(FontStyle::Bold)
        .color(&INK);
    area.draw_text(t(lang, "panel_a_title"), &title_style, (geom.plot_x0, y0 + 6))
        .map_err(|e| anyhow!("title: {e}"))?;

    // 4. Y-axis label (top-left of the panel; multi-line preserved
    //    because `regs_entering` contains a literal '\n').
    let ylabel_style = ("sans-serif", 9).into_font().color(&LABEL_GREY);
    let mut yy = geom.plot_y0 + 10;
    for line in t(lang, "regs_entering").lines() {
        area.draw_text(line, &ylabel_style, (x0 + 10, yy))
            .map_err(|e| anyhow!("ylabel: {e}"))?;
        yy += 11;
    }

    // 5. Stacked bars (one rectangle per non-empty (year, jurisdiction)
    //    cell). Tracks the running stack height in `bottoms`.
    let mut bottoms: HashMap<i32, u32> = HashMap::new();
    for jur in JUR_ORDER {
        let colour = hex_to_rgb(colour_for(jur));
        for yr in X_LO..=X_HI {
            let count = year_counts
                .get(&yr)
                .and_then(|m| m.get(jur))
                .copied()
                .unwrap_or(0);
            if count == 0 {
                continue;
            }
            let bottom = *bottoms.get(&yr).unwrap_or(&0);
            let new_top = bottom + count;
            let xc = geom.year_to_x(yr as f64);
            let bx0 = xc - geom.bar_half_px();
            let bx1 = xc + geom.bar_half_px();
            let by_top = geom.count_to_y(new_top as f64, y_top_val);
            let by_bot = geom.count_to_y(bottom as f64, y_top_val);
            area.draw(&Rectangle::new(
                [(bx0, by_top), (bx1, by_bot)],
                colour.filled(),
            ))
            .map_err(|e| anyhow!("bar fill: {e}"))?;
            // White 1px edge so adjacent jurisdiction stripes stay
            // visually distinct.
            area.draw(&Rectangle::new(
                [(bx0, by_top), (bx1, by_bot)],
                ShapeStyle::from(&WHITE).stroke_width(1),
            ))
            .map_err(|e| anyhow!("bar edge: {e}"))?;
            bottoms.insert(yr, new_top);
        }
    }

    // 6. Count labels above bars whose total is ≥ 3 (matches python
    //    threshold — quieter bars stay un-annotated).
    let count_style = ("sans-serif", 10)
        .into_font()
        .style(FontStyle::Bold)
        .color(&INK)
        .pos(Pos::new(HPos::Center, VPos::Bottom));
    for yr in X_LO..=X_HI {
        let total = *bottoms.get(&yr).unwrap_or(&0);
        if total >= 3 {
            let xc = geom.year_to_x(yr as f64);
            let yt = geom.count_to_y(total as f64 + 0.18, y_top_val);
            area.draw_text(&format!("{total}"), &count_style, (xc, yt - 2))
                .map_err(|e| anyhow!("count label: {e}"))?;
        }
    }

    // 7. "today" annotation next to the dashed line.
    let today_label_style = ("sans-serif", 9)
        .into_font()
        .style(FontStyle::Italic)
        .color(&TODAY_LINE);
    let today_px = geom.year_to_x(TODAY_X_VAL);
    area.draw_text(
        &format!(" {}", t(lang, "today")),
        &today_label_style,
        (today_px + 3, geom.plot_y0 + 2),
    )
    .map_err(|e| anyhow!("today annot: {e}"))?;

    Ok(())
}

// ============================================================================
// Geometry helper — converts year + count into pixel coordinates.
// ============================================================================

struct Geometry {
    plot_x0: i32,
    plot_x1: i32,
    plot_y0: i32,
    plot_y1: i32,
    plot_w: f64,
    plot_h: f64,
    x_range: f64,
}

impl Geometry {
    fn new(x0: i32, y0: i32, x1: i32, y1: i32) -> Self {
        let plot_x0 = x0 + PAD_LEFT;
        let plot_x1 = x1 - PAD_RIGHT;
        let plot_y0 = y0 + PAD_TOP;
        let plot_y1 = y1 - PAD_BOTTOM;
        let plot_w = (plot_x1 - plot_x0) as f64;
        let plot_h = (plot_y1 - plot_y0) as f64;
        let x_range = (X_HI + RIGHT_PAD_YEARS - X_LO) as f64;
        Self {
            plot_x0,
            plot_x1,
            plot_y0,
            plot_y1,
            plot_w,
            plot_h,
            x_range,
        }
    }

    fn year_to_x(&self, yr: f64) -> i32 {
        self.plot_x0 + ((yr - X_LO as f64) / self.x_range * self.plot_w) as i32
    }

    fn count_to_y(&self, count: f64, y_top_val: f64) -> i32 {
        self.plot_y1 - (count / y_top_val * self.plot_h) as i32
    }

    /// Half-width of one year bar in pixels. 0.74 matches the python
    /// `width=0.74` argument to `ax.bar`.
    fn bar_half_px(&self) -> i32 {
        ((0.74 * self.plot_w / self.x_range) / 2.0).max(1.0) as i32
    }
}

fn draw_grid(area: &DrawingArea<BitMapBackend<'_>, plotters::coord::Shift>, g: &Geometry) -> Result<()> {
    for yr in X_LO..=X_HI {
        let px = g.year_to_x(yr as f64);
        let (colour, width) = if yr % 5 == 0 {
            (GRID_5YR, 1u32)
        } else {
            (GRID_LIGHT, 1u32)
        };
        area.draw(&PathElement::new(
            vec![(px, g.plot_y0), (px, g.plot_y1)],
            ShapeStyle::from(&colour).stroke_width(width),
        ))
        .map_err(|e| anyhow!("grid line: {e}"))?;
    }
    Ok(())
}

fn draw_today_dashed(
    area: &DrawingArea<BitMapBackend<'_>, plotters::coord::Shift>,
    g: &Geometry,
) -> Result<()> {
    let today_px = g.year_to_x(TODAY_X_VAL);
    // Plotters doesn't ship dashed strokes for arbitrary lines —
    // approximate by drawing short 4px segments with 4px gaps.
    let mut y = g.plot_y0;
    while y < g.plot_y1 {
        let y_end = (y + 4).min(g.plot_y1);
        area.draw(&PathElement::new(
            vec![(today_px, y), (today_px, y_end)],
            ShapeStyle::from(&TODAY_LINE).stroke_width(2),
        ))
        .map_err(|e| anyhow!("today dash: {e}"))?;
        y += 8;
    }
    Ok(())
}

fn draw_right_edge(
    area: &DrawingArea<BitMapBackend<'_>, plotters::coord::Shift>,
    g: &Geometry,
) -> Result<()> {
    let edge_px = g.year_to_x(X_HI as f64 + 0.1);
    area.draw(&PathElement::new(
        vec![(edge_px, g.plot_y0), (edge_px, g.plot_y1)],
        ShapeStyle::from(&RIGHT_EDGE).stroke_width(1),
    ))
    .map_err(|e| anyhow!("right edge: {e}"))?;
    Ok(())
}

fn hex_to_rgb(hex: &str) -> RGBColor {
    let h = hex.trim_start_matches('#');
    if h.len() == 6 {
        let r = u8::from_str_radix(&h[0..2], 16).unwrap_or(0x80);
        let g = u8::from_str_radix(&h[2..4], 16).unwrap_or(0x80);
        let b = u8::from_str_radix(&h[4..6], 16).unwrap_or(0x80);
        RGBColor(r, g, b)
    } else {
        RGBColor(0x80, 0x80, 0x80)
    }
}

// ============================================================================
// Tests.
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn hex_to_rgb_parses_jurisdiction_colours() {
        let eu = hex_to_rgb("#1f4e79");
        assert_eq!(eu, RGBColor(0x1f, 0x4e, 0x79));
        let global = hex_to_rgb("#6d4c41");
        assert_eq!(global, RGBColor(0x6d, 0x4c, 0x41));
    }

    #[test]
    fn hex_to_rgb_handles_unknown_input() {
        let grey = hex_to_rgb("not-a-hex");
        assert_eq!(grey, RGBColor(0x80, 0x80, 0x80));
    }

    #[test]
    fn geometry_year_to_x_maps_endpoints_into_plot_area() {
        let g = Geometry::new(0, 0, 1450, 220);
        let left = g.year_to_x(X_LO as f64);
        let right = g.year_to_x((X_HI + RIGHT_PAD_YEARS) as f64);
        assert_eq!(left, g.plot_x0);
        // Right endpoint maps to plot_x1 (with at most 1 px rounding).
        assert!((right - g.plot_x1).abs() <= 1);
    }

    #[test]
    fn geometry_count_to_y_flips_axis() {
        let g = Geometry::new(0, 0, 1450, 220);
        // count=0 → bottom of plot area.
        assert_eq!(g.count_to_y(0.0, 10.0), g.plot_y1);
        // count==y_top_val → top of plot area.
        assert_eq!(g.count_to_y(10.0, 10.0), g.plot_y0);
    }

    #[test]
    fn render_panel_a_only_writes_non_trivial_png_en() {
        let tmp = TempDir::new().unwrap();
        let out = tmp.path().join("panel_a_en.png");
        render_panel_a_only(&out, "en").expect("render should succeed");
        let size = std::fs::metadata(&out).expect("file exists").len();
        // A blank canvas at 1450x220 BitMapBackend PNG is roughly 1-2 KB.
        // A panel with grid + bars + labels is meaningfully larger.
        assert!(size > 4_000, "panel A png suspiciously small: {size} bytes");
        // Quick PNG signature sniff.
        let bytes = std::fs::read(&out).expect("readable");
        assert_eq!(&bytes[0..4], &[0x89, 0x50, 0x4E, 0x47], "PNG signature");
    }

    #[test]
    fn render_panel_a_only_writes_non_trivial_png_de() {
        let tmp = TempDir::new().unwrap();
        let out = tmp.path().join("panel_a_de.png");
        render_panel_a_only(&out, "de").expect("DE render should succeed");
        let size = std::fs::metadata(&out).expect("file exists").len();
        assert!(size > 4_000, "DE panel A png suspiciously small: {size} bytes");
    }

    #[test]
    fn render_panel_a_only_writes_non_trivial_png_hi() {
        let tmp = TempDir::new().unwrap();
        let out = tmp.path().join("panel_a_hi.png");
        render_panel_a_only(&out, "hi").expect("HI render should succeed");
        let size = std::fs::metadata(&out).expect("file exists").len();
        assert!(size > 4_000, "HI panel A png suspiciously small: {size} bytes");
    }

    #[test]
    fn render_panel_a_only_unknown_lang_falls_back_to_english() {
        // Unknown lang must not crash — the t() helper handles
        // fallback. The render should still produce a PNG (the same
        // English layout).
        let tmp = TempDir::new().unwrap();
        let out = tmp.path().join("panel_a_ja.png");
        render_panel_a_only(&out, "ja").expect("unknown-lang render should succeed");
        let size = std::fs::metadata(&out).expect("file exists").len();
        assert!(size > 4_000, "ja panel A png suspiciously small: {size} bytes");
    }
}
