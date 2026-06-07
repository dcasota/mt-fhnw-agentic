//! Panel B — hot-spots: same regulatory goal, mismatched country timelines.
//!
//! Port of the python kit's `render_panel_B` (lines 1011-1049 of
//! `_render_regulation_timeline_v3.py` v_130732). One row per goal in
//! [`GOAL_KEYS`] (12 rows, top→bottom matches the python's inverted
//! y-axis); for each row we draw:
//!
//! - A horizontal span line from the earliest `applies_year` to the
//!   latest milestone year in the group. Hot (≥3-yr spread across ≥2
//!   jurisdictions) → orange-red, thicker, more opaque. Cool → grey.
//! - A circle marker at each regulation's `applies_year`, coloured by
//!   the jurisdiction palette, white-edged.
//! - The applies year as a small label below the marker.
//! - A right-margin annotation: `span_lo–span_hi · jur_count jur (codes)`
//!   plus `  critical` if the row is hot. Bold + orange-red when hot.
//!
//! Same canvas-rectangle / pixel-space contract as
//! `regulation_timeline_panel_a` — the caller owns layout.

#![allow(dead_code)] // Public entry points wired via lib.rs in a follow-up.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result, anyhow};
use plotters::prelude::*;
use plotters::style::text_anchor::{HPos, Pos, VPos};

use crate::regulation_timeline::{GOAL_KEYS, REGS, X_HI, X_LO, colour_for, jur_abbrev, t};
use crate::regulation_timeline_panel_a::RIGHT_PAD_YEARS;

// ============================================================================
// Layout constants.
// ============================================================================

pub const STANDALONE_W: u32 = 1450;
pub const STANDALONE_H: u32 = 460;

const PAD_LEFT: i32 = 360; // wider than Panel A — needs room for goal names
const PAD_RIGHT: i32 = 40;
const PAD_TOP: i32 = 28;
const PAD_BOTTOM: i32 = 12;

// Cue colours (same families as Panel A, plus the hot-spot signal).
const GRID_LIGHT: RGBColor = RGBColor(0xEA, 0xEA, 0xEA);
const GRID_5YR: RGBColor = RGBColor(0xA0, 0xA0, 0xA0);
const TODAY_LINE: RGBColor = RGBColor(0x22, 0x22, 0x22);
const RIGHT_EDGE: RGBColor = RGBColor(0x88, 0x88, 0x88);
const INK: RGBColor = RGBColor(0x1A, 0x1A, 0x1A);
const LABEL_GREY: RGBColor = RGBColor(0x33, 0x33, 0x33);
const SPAN_COOL: RGBColor = RGBColor(0x88, 0x88, 0x88);
const SPAN_HOT: RGBColor = RGBColor(0xff, 0x45, 0x00);
const ANNOT_HOT_TEXT: RGBColor = RGBColor(0xA8, 0x32, 0x00);

const TODAY_X_VAL: f64 = 2026.42;

// Marker / span tunings (matches the python `linewidth` / `s=70`
// scaled down to look roughly equivalent at our 1450px width).
const SPAN_W_HOT: u32 = 5;
const SPAN_W_COOL: u32 = 3;
const MARKER_R: i32 = 5;

// ============================================================================
// Public entry points.
// ============================================================================

/// Render Panel B as a standalone PNG. Used for incremental visual diff
/// against the middle region of the kit's `regulation_timeline_v3.png`.
pub fn render_panel_b_only(out_png: &Path, lang: &str) -> Result<()> {
    let root = BitMapBackend::new(out_png, (STANDALONE_W, STANDALONE_H)).into_drawing_area();
    root.fill(&WHITE)
        .map_err(|e| anyhow!("fill: {e}"))?;
    render_panel_b(&root, lang, 0, 0, STANDALONE_W as i32, STANDALONE_H as i32)
        .with_context(|| "render_panel_b")?;
    root.present()
        .map_err(|e| anyhow!("present: {e}"))?;
    Ok(())
}

/// Render Panel B into the given pixel-space bounding box on `area`.
pub fn render_panel_b(
    area: &DrawingArea<BitMapBackend<'_>, plotters::coord::Shift>,
    lang: &str,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
) -> Result<()> {
    let geom = Geometry::new(x0, y0, x1, y1);

    // 1. Group REGS by goal_key.
    let mut by_goal: HashMap<&'static str, Vec<&'static crate::regulation_timeline::Regulation>> =
        HashMap::new();
    for r in REGS {
        by_goal.entry(r.goal_key).or_default().push(r);
    }

    // 2. Background grid + today + right-edge.
    draw_grid(area, &geom)?;
    draw_today_dashed(area, &geom)?;
    draw_right_edge(area, &geom)?;

    // 3. Title.
    let title_style = ("sans-serif", 13)
        .into_font()
        .style(FontStyle::Bold)
        .color(&INK);
    area.draw_text(t(lang, "panel_b_title"), &title_style, (geom.plot_x0, y0 + 6))
        .map_err(|e| anyhow!("title: {e}"))?;

    // 4. Per-goal rows.
    let goal_label_style = ("sans-serif", 9)
        .into_font()
        .color(&LABEL_GREY)
        .pos(Pos::new(HPos::Right, VPos::Center));
    let year_label_style = ("sans-serif", 7)
        .into_font()
        .color(&LABEL_GREY)
        .pos(Pos::new(HPos::Center, VPos::Top));
    let jur_word = t(lang, "span_jur_word");

    for (i, goal_key) in GOAL_KEYS.iter().enumerate() {
        let row_y = geom.row_y(i);
        // Bind the lookup String to keep it alive across the borrow.
        let goal_lookup_key = goal_key_to_lookup(goal_key);
        let goal_text = t(lang, &goal_lookup_key);

        // 4a. Goal name on the left (right-aligned to plot_x0 - 8).
        area.draw_text(goal_text, &goal_label_style, (geom.plot_x0 - 8, row_y))
            .map_err(|e| anyhow!("goal label: {e}"))?;

        let items = match by_goal.get(goal_key) {
            Some(v) if !v.is_empty() => v,
            _ => continue,
        };

        // 4b. Compute span: applies_year .. max(milestones)
        let applies_years: Vec<i32> = items.iter().map(|r| r.applies_year).collect();
        let end_years: Vec<i32> = items
            .iter()
            .map(|r| {
                r.milestones
                    .iter()
                    .map(|m| m.0)
                    .max()
                    .unwrap_or(r.applies_year)
            })
            .collect();
        let span_lo = *applies_years.iter().min().unwrap_or(&X_LO);
        let span_hi = *end_years.iter().max().unwrap_or(&X_LO);
        let jur_set: std::collections::BTreeSet<&str> = items.iter().map(|r| r.jur).collect();
        let jur_count = jur_set.len();
        let is_hot = (span_hi - span_lo) >= 3 && jur_count >= 2;

        // 4c. Span line.
        let (span_colour, span_w, span_alpha) = if is_hot {
            (SPAN_HOT, SPAN_W_HOT, 0.55)
        } else {
            (SPAN_COOL, SPAN_W_COOL, 0.35)
        };
        let lo_px = geom.year_to_x(span_lo as f64);
        let hi_px = geom.year_to_x(span_hi as f64);
        let span_style =
            ShapeStyle::from(&span_colour.mix(span_alpha)).stroke_width(span_w);
        area.draw(&PathElement::new(
            vec![(lo_px, row_y), (hi_px, row_y)],
            span_style,
        ))
        .map_err(|e| anyhow!("span line: {e}"))?;

        // 4d. Markers + year labels.
        for it in items {
            let mx = geom.year_to_x(it.applies_year as f64);
            let mc = hex_to_rgb(colour_for(it.jur));
            area.draw(&Circle::new((mx, row_y), MARKER_R, mc.filled()))
                .map_err(|e| anyhow!("marker fill: {e}"))?;
            area.draw(&Circle::new(
                (mx, row_y),
                MARKER_R,
                ShapeStyle::from(&WHITE).stroke_width(1),
            ))
            .map_err(|e| anyhow!("marker edge: {e}"))?;
            area.draw_text(
                &format!("{}", it.applies_year),
                &year_label_style,
                (mx, row_y + MARKER_R + 2),
            )
            .map_err(|e| anyhow!("year label: {e}"))?;
        }

        // 4e. Right-margin annotation.
        let jurs_sorted: Vec<&str> = {
            let mut v: Vec<&str> = jur_set.into_iter().collect();
            v.sort();
            v
        };
        let jurs_displayed: Vec<&str> = jurs_sorted
            .iter()
            .map(|j| jur_abbrev(lang, j))
            .collect();
        let flag = if is_hot { t(lang, "hot_flag") } else { "" };
        let annot = format!(
            "{}\u{2013}{} \u{00B7} {} {} ({}){}",
            span_lo,
            span_hi,
            jur_count,
            jur_word,
            jurs_displayed.join(", "),
            flag,
        );
        // `style()` lives on `FontDesc` (before `.color()` upgrades it
        // to `TextStyle`), so build the bold and non-bold variants in
        // separate branches.
        let annot_x = geom.year_to_x(X_HI as f64 + 0.3);
        let annot_pos = Pos::new(HPos::Left, VPos::Center);
        if is_hot {
            let style = ("sans-serif", 9)
                .into_font()
                .style(FontStyle::Bold)
                .color(&ANNOT_HOT_TEXT)
                .pos(annot_pos);
            area.draw_text(&annot, &style, (annot_x, row_y))
                .map_err(|e| anyhow!("annotation: {e}"))?;
        } else {
            let style = ("sans-serif", 9)
                .into_font()
                .color(&LABEL_GREY)
                .pos(annot_pos);
            area.draw_text(&annot, &style, (annot_x, row_y))
                .map_err(|e| anyhow!("annotation: {e}"))?;
        }
    }

    Ok(())
}

// ============================================================================
// Geometry — Y axis is one row per goal (inverted: i=0 at top).
// ============================================================================

struct Geometry {
    plot_x0: i32,
    plot_x1: i32,
    plot_y0: i32,
    plot_y1: i32,
    plot_w: f64,
    plot_h: f64,
    x_range: f64,
    n_rows: i32,
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
            n_rows: GOAL_KEYS.len() as i32,
        }
    }

    fn year_to_x(&self, yr: f64) -> i32 {
        self.plot_x0 + ((yr - X_LO as f64) / self.x_range * self.plot_w) as i32
    }

    fn row_y(&self, i: usize) -> i32 {
        // Inverted python axis: row i=0 at top of plot, i=n-1 at bottom.
        // Use centre-of-row positioning so each row gets ~plot_h/n vertical
        // space and the line/marker sits in the middle of that band.
        let band = self.plot_h / self.n_rows as f64;
        self.plot_y0 + (i as f64 * band + band / 2.0) as i32
    }
}

fn draw_grid(
    area: &DrawingArea<BitMapBackend<'_>, plotters::coord::Shift>,
    g: &Geometry,
) -> Result<()> {
    for yr in X_LO..=X_HI {
        let px = g.year_to_x(yr as f64);
        let colour = if yr % 5 == 0 { GRID_5YR } else { GRID_LIGHT };
        area.draw(&PathElement::new(
            vec![(px, g.plot_y0), (px, g.plot_y1)],
            ShapeStyle::from(&colour).stroke_width(1),
        ))
        .map_err(|e| anyhow!("grid: {e}"))?;
    }
    Ok(())
}

fn draw_today_dashed(
    area: &DrawingArea<BitMapBackend<'_>, plotters::coord::Shift>,
    g: &Geometry,
) -> Result<()> {
    let today_px = g.year_to_x(TODAY_X_VAL);
    let mut y = g.plot_y0;
    while y < g.plot_y1 {
        let y_end = (y + 4).min(g.plot_y1);
        area.draw(&PathElement::new(
            vec![(today_px, y), (today_px, y_end)],
            ShapeStyle::from(&TODAY_LINE).stroke_width(2),
        ))
        .map_err(|e| anyhow!("today: {e}"))?;
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

/// Goal-key → LANGUAGES lookup key. The python `goal_text(g, lang)`
/// just does `t('goal_' + g, lang)`; we mirror that.
fn goal_key_to_lookup(goal_key: &str) -> String {
    format!("goal_{goal_key}")
}

// ============================================================================
// Tests.
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn geometry_row_y_inverts_so_row_zero_is_top() {
        let g = Geometry::new(0, 0, 1450, 460);
        let top = g.row_y(0);
        let bottom = g.row_y(GOAL_KEYS.len() - 1);
        assert!(top < bottom, "row 0 must be at top (smaller y): {top} < {bottom}");
        assert!(top >= g.plot_y0 && bottom <= g.plot_y1, "rows fit in plot");
    }

    #[test]
    fn goal_key_to_lookup_prefixes_with_goal_underscore() {
        assert_eq!(goal_key_to_lookup("data_protection"), "goal_data_protection");
        assert_eq!(goal_key_to_lookup("pqc_migration"), "goal_pqc_migration");
    }

    #[test]
    fn render_panel_b_only_writes_non_trivial_png_en() {
        let tmp = TempDir::new().unwrap();
        let out = tmp.path().join("panel_b_en.png");
        render_panel_b_only(&out, "en").expect("EN render should succeed");
        let bytes = std::fs::read(&out).unwrap();
        assert_eq!(&bytes[0..4], &[0x89, 0x50, 0x4E, 0x47]);
        // Should be meaningfully larger than blank canvas.
        assert!(bytes.len() > 8_000, "panel B png suspiciously small: {} bytes", bytes.len());
    }

    #[test]
    fn render_panel_b_only_writes_non_trivial_png_de() {
        let tmp = TempDir::new().unwrap();
        let out = tmp.path().join("panel_b_de.png");
        render_panel_b_only(&out, "de").expect("DE render should succeed");
        let size = std::fs::metadata(&out).unwrap().len();
        assert!(size > 8_000);
    }

    #[test]
    fn render_panel_b_only_writes_non_trivial_png_hi() {
        let tmp = TempDir::new().unwrap();
        let out = tmp.path().join("panel_b_hi.png");
        render_panel_b_only(&out, "hi").expect("HI render should succeed");
        let size = std::fs::metadata(&out).unwrap().len();
        assert!(size > 8_000);
    }

    #[test]
    fn render_panel_b_handles_unknown_lang_via_english_fallback() {
        let tmp = TempDir::new().unwrap();
        let out = tmp.path().join("panel_b_ja.png");
        render_panel_b_only(&out, "ja").expect("unknown-lang render should succeed");
        let size = std::fs::metadata(&out).unwrap().len();
        assert!(size > 8_000);
    }

    #[test]
    fn every_goal_in_GOAL_KEYS_has_a_translation_key() {
        // Smoke: `goal_<key>` lookup returns a non-empty string in every
        // language. Failing here means the LANGUAGES table is missing a
        // goal key that the renderer iterates over.
        for lang in ["en", "de", "fr", "it", "rm", "hi"] {
            for g in GOAL_KEYS {
                let key = goal_key_to_lookup(g);
                let v = t(lang, &key);
                assert!(
                    !v.is_empty() && v != key,
                    "missing translation for goal '{g}' in lang '{lang}' (got {v:?})"
                );
            }
        }
    }
}
