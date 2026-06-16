//! Panel C — per-regulation detail Gantt (conflict-involved subset).
//!
//! Port of the python kit's `render_panel_C` + `_render_detail`
//! (lines 1051-1189 of `_render_regulation_timeline_v3.py` v_130732).
//!
//! Renders the regulations that participate in at least one of the 5
//! conflict pairs (10 markers / ~9 distinct regulations) as a stacked
//! Gantt with rich per-row metadata:
//! - **Bars**: light fade-in (pub_year → applies_year, alpha 0.25),
//!   solid in-force (applies_year → sunset or X_HI-0.2, alpha 0.85),
//!   open-ended arrow when no sunset.
//! - **Milestones**: small black filled circles at milestone years.
//! - **Conflict markers**: red ✖ at the conflict year + truncated
//!   conflict text below; vertical dashed lines link same-year pairs
//!   so the contradiction is unmissable.
//! - **Meta-methodology columns** to the right of the Gantt area:
//!   sector codes / extraterritoriality flag / enforcement teeth dots
//!   / update cadence / chapter citations / one-line summary.
//! - **Goal-group separator lines** between rows whose `goal_key`
//!   changes (so the reader sees the goal grouping the rows inherit).
//!
//! **Deferred to a follow-up**: matplotlib's `FancyArrowPatch`
//! curved mutual-recognition arcs on the left margin. Plotters has no
//! native arc primitive — porting needs a polyline-bezier approximation
//! that pixel-matches the kit. Tracked in the regulation_timeline
//! follow-up.

#![allow(dead_code)] // Public entry points wired via lib.rs in a follow-up.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use anyhow::{Context, Result, anyhow};
use plotters::prelude::*;
use plotters::style::text_anchor::{HPos, Pos, VPos};

use crate::regulation_timeline::{
    CONFLICTS, GOAL_KEYS, REGS, Regulation, X_HI, X_LO, colour_for, sector_label, t,
};
use crate::regulation_timeline_panel_a::RIGHT_PAD_YEARS;

// ============================================================================
// Layout constants.
// ============================================================================

pub const STANDALONE_W: u32 = 1450;
pub const STANDALONE_H: u32 = 480;

const PAD_LEFT: i32 = 280; // wide enough for regulation row labels
const PAD_RIGHT: i32 = 480; // wide enough for the 6 metadata columns
const PAD_TOP: i32 = 36;
const PAD_BOTTOM: i32 = 24;

const GRID_LIGHT: RGBColor = RGBColor(0xEA, 0xEA, 0xEA);
const GRID_5YR: RGBColor = RGBColor(0xA0, 0xA0, 0xA0);
const TODAY_LINE: RGBColor = RGBColor(0x22, 0x22, 0x22);
const RIGHT_EDGE: RGBColor = RGBColor(0x88, 0x88, 0x88);
const INK: RGBColor = RGBColor(0x1A, 0x1A, 0x1A);
const LABEL_GREY: RGBColor = RGBColor(0x33, 0x33, 0x33);
const SEPARATOR: RGBColor = RGBColor(0xBB, 0xBB, 0xBB);
const CONFLICT_MARK: RGBColor = RGBColor(0xFF, 0x00, 0x66);
const CONFLICT_TEXT: RGBColor = RGBColor(0xA3, 0x00, 0x40);
const ET_RED: RGBColor = RGBColor(0xC0, 0x00, 0x00);

const TODAY_X_VAL: f64 = 2026.42;

// Meta-methodology column anchors (year-data units past X_HI; matches
// python `X_HI + 0.5 / 4.3 / 5.6 / 7.0 / 9.5 / 14.5`).
const COL_SECTOR: f64 = 0.5;
const COL_REACH: f64 = 4.3;
const COL_TEETH: f64 = 5.6;
const COL_CAD: f64 = 7.0;
const COL_CHAP: f64 = 9.5;
const COL_NOTE: f64 = 14.5;

// ============================================================================
// Public entry points.
// ============================================================================

pub fn render_panel_c_only(out_png: &Path, lang: &str) -> Result<()> {
    let root = BitMapBackend::new(out_png, (STANDALONE_W, STANDALONE_H)).into_drawing_area();
    root.fill(&WHITE).map_err(|e| anyhow!("fill: {e}"))?;
    render_panel_c(&root, lang, 0, 0, STANDALONE_W as i32, STANDALONE_H as i32)
        .with_context(|| "render_panel_c")?;
    root.present().map_err(|e| anyhow!("present: {e}"))?;
    Ok(())
}

/// Render Panel C (conflict-involved subset only) into the given
/// pixel-space bounding box on `area`.
pub fn render_panel_c(
    area: &DrawingArea<BitMapBackend<'_>, plotters::coord::Shift>,
    lang: &str,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
) -> Result<()> {
    let conflict_labels: BTreeSet<&str> = CONFLICTS.iter().map(|c| c.label).collect();
    let subset: Vec<&'static Regulation> = REGS
        .iter()
        .filter(|r| conflict_labels.contains(r.label))
        .collect();
    render_detail(area, lang, &subset, "panel_c_title", x0, y0, x1, y1)
}

// ============================================================================
// Shared per-row detail-Gantt renderer (used by Panel C; Panel D will
// share this when it lands).
// ============================================================================

fn render_detail(
    area: &DrawingArea<BitMapBackend<'_>, plotters::coord::Shift>,
    lang: &str,
    regs_subset: &[&'static Regulation],
    title_key: &str,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
) -> Result<()> {
    // 1. Sort the subset by (goal taxonomy position, applies_year, label).
    let goal_pos: HashMap<&str, usize> =
        GOAL_KEYS.iter().enumerate().map(|(i, g)| (*g, i)).collect();
    let mut detail: Vec<&Regulation> = regs_subset.to_vec();
    detail.sort_by_key(|r| {
        (
            *goal_pos.get(r.goal_key).unwrap_or(&99),
            r.applies_year,
            r.label,
        )
    });

    let label_to_row: HashMap<&str, usize> = detail
        .iter()
        .enumerate()
        .map(|(i, r)| (r.label, i))
        .collect();

    let geom = Geometry::new(x0, y0, x1, y1, detail.len());

    // 2. Background grid + today + right edge.
    draw_grid(area, &geom)?;
    draw_today_dashed(area, &geom)?;
    draw_right_edge(area, &geom)?;

    // 3. Title.
    let title_style = ("sans-serif", 13)
        .into_font()
        .style(FontStyle::Bold)
        .color(&INK);
    area.draw_text(t(lang, title_key), &title_style, (geom.plot_x0, y0 + 6))
        .map_err(|e| anyhow!("title: {e}"))?;

    // 4. Goal-group separators (horizontal grey lines between rows
    //    whose goal_key differs from the previous row).
    let mut prev_goal: Option<&str> = None;
    for (i, r) in detail.iter().enumerate() {
        if let Some(pg) = prev_goal
            && pg != r.goal_key
        {
            let sep_y = geom.row_top(i);
            area.draw(&PathElement::new(
                vec![(geom.plot_x0, sep_y), (geom.plot_x1, sep_y)],
                ShapeStyle::from(&SEPARATOR).stroke_width(1),
            ))
            .map_err(|e| anyhow!("separator: {e}"))?;
        }
        prev_goal = Some(r.goal_key);
    }

    // 5. Per-row content.
    let row_label_style = ("sans-serif", 9)
        .into_font()
        .color(&LABEL_GREY)
        .pos(Pos::new(HPos::Right, VPos::Center));
    let meta_style = ("sans-serif", 8)
        .into_font()
        .color(&INK)
        .pos(Pos::new(HPos::Left, VPos::Center));
    let chap_style = ("sans-serif", 8)
        .into_font()
        .color(&RGBColor(0x1F, 0x4E, 0x79))
        .pos(Pos::new(HPos::Left, VPos::Center));
    let note_style = ("sans-serif", 8)
        .into_font()
        .color(&LABEL_GREY)
        .pos(Pos::new(HPos::Left, VPos::Center));
    let et_style = ("sans-serif", 8)
        .into_font()
        .style(FontStyle::Bold)
        .color(&ET_RED)
        .pos(Pos::new(HPos::Left, VPos::Center));

    for (i, r) in detail.iter().enumerate() {
        let row_y = geom.row_y(i);
        let bar_top = geom.row_y_offset(i, -0.30);
        let bar_bot = geom.row_y_offset(i, 0.30);
        let colour = hex_to_rgb(colour_for(r.jur));

        // 5a. Row label on the left.
        area.draw_text(r.label, &row_label_style, (geom.plot_x0 - 8, row_y))
            .map_err(|e| anyhow!("row label: {e}"))?;

        // 5b. Fade-in band (pub_year → applies_year) at 0.25 alpha.
        if r.applies_year > r.pub_year {
            let x_left = geom.year_to_x(r.pub_year as f64);
            let x_right = geom.year_to_x(r.applies_year as f64);
            area.draw(&Rectangle::new(
                [(x_left, bar_top), (x_right, bar_bot)],
                colour.mix(0.25).filled(),
            ))
            .map_err(|e| anyhow!("fade-in: {e}"))?;
        }

        // 5c. In-force band (applies_year → end) at 0.85 alpha.
        let end = r.sunset_year.map_or((X_HI as f64) - 0.2, |s| s as f64);
        if end > r.applies_year as f64 {
            let x_left = geom.year_to_x(r.applies_year as f64);
            let x_right = geom.year_to_x(end);
            area.draw(&Rectangle::new(
                [(x_left, bar_top), (x_right, bar_bot)],
                colour.mix(0.85).filled(),
            ))
            .map_err(|e| anyhow!("in-force: {e}"))?;
        }

        // 5d. Open-ended arrow when no sunset (pointing right just
        // inside the right edge of the plot area).
        if r.sunset_year.is_none() {
            let arrow_x_end = geom.year_to_x(X_HI as f64 - 0.1);
            let arrow_x_start = geom.year_to_x(X_HI as f64 - 0.8);
            draw_right_arrow(area, arrow_x_start, arrow_x_end, row_y, &colour)?;
        }

        // 5e. Milestone dots (small black circles).
        for (myr, _txt) in r.milestones {
            let mx = geom.year_to_x(*myr as f64);
            area.draw(&Circle::new(
                (mx, row_y),
                3,
                ShapeStyle::from(&INK).filled(),
            ))
            .map_err(|e| anyhow!("milestone: {e}"))?;
        }

        // 5f. Meta-methodology columns.
        let sector_text = r
            .sector
            .iter()
            .map(|s| sector_label(lang, s))
            .collect::<Vec<_>>()
            .join(" \u{00B7} ");
        area.draw_text(
            &sector_text,
            &meta_style,
            (geom.year_to_x(X_HI as f64 + COL_SECTOR), row_y),
        )
        .map_err(|e| anyhow!("sector: {e}"))?;

        if r.et {
            area.draw_text(
                t(lang, "reach_et"),
                &et_style,
                (geom.year_to_x(X_HI as f64 + COL_REACH), row_y),
            )
            .map_err(|e| anyhow!("ET: {e}"))?;
        }

        let dots = "\u{25CF}".repeat(r.teeth as usize) + &"\u{25CB}".repeat(3 - r.teeth as usize);
        let teeth_colour = match r.teeth {
            1 => RGBColor(0x88, 0x88, 0x88),
            2 => RGBColor(0xC0, 0x80, 0x20),
            3 => RGBColor(0xC0, 0x00, 0x00),
            _ => RGBColor(0x88, 0x88, 0x88),
        };
        let teeth_style = ("sans-serif", 9)
            .into_font()
            .color(&teeth_colour)
            .pos(Pos::new(HPos::Left, VPos::Center));
        area.draw_text(
            &dots,
            &teeth_style,
            (geom.year_to_x(X_HI as f64 + COL_TEETH), row_y),
        )
        .map_err(|e| anyhow!("teeth: {e}"))?;

        let cad_label_key = format!("cadence_{}", r.cadence);
        let cad_text = t(lang, &cad_label_key);
        let cad_colour = match r.cadence {
            "ann" => RGBColor(0xA8, 0x32, 0x00),
            "rev" => RGBColor(0x8A, 0x67, 0x00),
            "stat" => RGBColor(0x40, 0x60, 0x40),
            _ => INK,
        };
        let cad_style = ("sans-serif", 8)
            .into_font()
            .color(&cad_colour)
            .pos(Pos::new(HPos::Left, VPos::Center));
        area.draw_text(
            cad_text,
            &cad_style,
            (geom.year_to_x(X_HI as f64 + COL_CAD), row_y),
        )
        .map_err(|e| anyhow!("cadence: {e}"))?;

        let chap_text = r
            .chapters
            .iter()
            .map(|n| format!("\u{00A7}{n}"))
            .collect::<Vec<_>>()
            .join(" ");
        area.draw_text(
            &chap_text,
            &chap_style,
            (geom.year_to_x(X_HI as f64 + COL_CHAP), row_y),
        )
        .map_err(|e| anyhow!("chapters: {e}"))?;

        // Truncate the note to fit roughly within the note-column
        // width (the python wraps to 2 lines × 48 chars; we cap at
        // ~80 chars on a single line for v1).
        let note = if r.note.chars().count() > 80 {
            let prefix: String = r.note.chars().take(78).collect();
            format!("{prefix}…")
        } else {
            r.note.to_string()
        };
        area.draw_text(
            &note,
            &note_style,
            (geom.year_to_x(X_HI as f64 + COL_NOTE), row_y),
        )
        .map_err(|e| anyhow!("note: {e}"))?;
    }

    // 6. Column headers above the first row.
    let hdr_style = ("sans-serif", 8)
        .into_font()
        .style(FontStyle::Bold)
        .color(&INK)
        .pos(Pos::new(HPos::Left, VPos::Center));
    let hdr_y = geom.plot_y0 + 6;
    for (col_x, key) in &[
        (COL_SECTOR, "col_sector"),
        (COL_REACH, "col_reach"),
        (COL_TEETH, "col_teeth"),
        (COL_CAD, "col_cadence"),
        (COL_CHAP, "col_chapters"),
        (COL_NOTE, "col_summary"),
    ] {
        area.draw_text(
            t(lang, key),
            &hdr_style,
            (geom.year_to_x(X_HI as f64 + *col_x), hdr_y),
        )
        .map_err(|e| anyhow!("header: {e}"))?;
    }

    // 7. Conflict markers + dashed connectors.
    let conflict_text_style = ("sans-serif", 7)
        .into_font()
        .color(&CONFLICT_TEXT)
        .pos(Pos::new(HPos::Center, VPos::Bottom));
    let mut by_year: BTreeMap<i32, Vec<usize>> = BTreeMap::new();
    for c in CONFLICTS {
        if let Some(&row) = label_to_row.get(c.label) {
            by_year.entry(c.year).or_default().push(row);
            let cx = geom.year_to_x(c.year as f64);
            let cy = geom.row_y(row);
            // Big red X (two diagonals).
            let sz = 6;
            area.draw(&PathElement::new(
                vec![(cx - sz, cy - sz), (cx + sz, cy + sz)],
                ShapeStyle::from(&CONFLICT_MARK).stroke_width(2),
            ))
            .map_err(|e| anyhow!("X 1: {e}"))?;
            area.draw(&PathElement::new(
                vec![(cx - sz, cy + sz), (cx + sz, cy - sz)],
                ShapeStyle::from(&CONFLICT_MARK).stroke_width(2),
            ))
            .map_err(|e| anyhow!("X 2: {e}"))?;
            // Truncated conflict text below.
            let truncated: String = if c.text.chars().count() > 55 {
                c.text.chars().take(55).collect()
            } else {
                c.text.to_string()
            };
            area.draw_text(
                &format!("\u{26A1}{truncated}"),
                &conflict_text_style,
                (cx, cy + 12),
            )
            .map_err(|e| anyhow!("conflict text: {e}"))?;
        }
    }
    // Dashed connectors between same-year conflict pairs.
    for (yr, rows) in by_year {
        if rows.len() < 2 {
            continue;
        }
        for i in 0..rows.len() {
            for j in (i + 1)..rows.len() {
                let y1 = geom.row_y(rows[i]);
                let y2 = geom.row_y(rows[j]);
                if y1 == y2 {
                    continue;
                }
                draw_vertical_dashed(area, geom.year_to_x(yr as f64), y1, y2, &CONFLICT_MARK)?;
            }
        }
    }

    Ok(())
}

// ============================================================================
// Geometry — Y axis: one row per regulation in the subset (inverted).
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
    fn new(x0: i32, y0: i32, x1: i32, y1: i32, n_rows: usize) -> Self {
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
            n_rows: n_rows.max(1) as i32,
        }
    }

    fn year_to_x(&self, yr: f64) -> i32 {
        self.plot_x0 + ((yr - X_LO as f64) / self.x_range * self.plot_w) as i32
    }

    fn row_y(&self, i: usize) -> i32 {
        let band = self.plot_h / self.n_rows as f64;
        self.plot_y0 + (i as f64 * band + band / 2.0) as i32
    }

    fn row_top(&self, i: usize) -> i32 {
        let band = self.plot_h / self.n_rows as f64;
        self.plot_y0 + (i as f64 * band) as i32
    }

    /// Like `row_y`, but with a fractional offset (in row-band units).
    /// `offset=-0.30` returns the top of the bar; `offset=0.30` the
    /// bottom. Matches the python bar half-height of 0.30.
    fn row_y_offset(&self, i: usize, offset: f64) -> i32 {
        let band = self.plot_h / self.n_rows as f64;
        self.plot_y0 + (i as f64 * band + band / 2.0 + offset * band) as i32
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

fn draw_right_arrow(
    area: &DrawingArea<BitMapBackend<'_>, plotters::coord::Shift>,
    x_start: i32,
    x_end: i32,
    y: i32,
    colour: &RGBColor,
) -> Result<()> {
    // Shaft.
    area.draw(&PathElement::new(
        vec![(x_start, y), (x_end, y)],
        ShapeStyle::from(colour).stroke_width(2),
    ))
    .map_err(|e| anyhow!("arrow shaft: {e}"))?;
    // Head: two short diagonals meeting at the tip.
    let head = 5;
    area.draw(&PathElement::new(
        vec![(x_end - head, y - head), (x_end, y)],
        ShapeStyle::from(colour).stroke_width(2),
    ))
    .map_err(|e| anyhow!("arrow head 1: {e}"))?;
    area.draw(&PathElement::new(
        vec![(x_end - head, y + head), (x_end, y)],
        ShapeStyle::from(colour).stroke_width(2),
    ))
    .map_err(|e| anyhow!("arrow head 2: {e}"))?;
    Ok(())
}

fn draw_vertical_dashed(
    area: &DrawingArea<BitMapBackend<'_>, plotters::coord::Shift>,
    x: i32,
    y1: i32,
    y2: i32,
    colour: &RGBColor,
) -> Result<()> {
    let (lo, hi) = if y1 < y2 { (y1, y2) } else { (y2, y1) };
    let mut y = lo;
    while y < hi {
        let y_end = (y + 3).min(hi);
        area.draw(&PathElement::new(
            vec![(x, y), (x, y_end)],
            ShapeStyle::from(&colour.mix(0.55)).stroke_width(1),
        ))
        .map_err(|e| anyhow!("dashed: {e}"))?;
        y += 6;
    }
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
    fn conflict_subset_has_expected_size() {
        let labels: BTreeSet<&str> = CONFLICTS.iter().map(|c| c.label).collect();
        // 5 conflict pairs × 2 sides = 10 markers, dedup'd to distinct
        // labels → expect ~9 (GDPR appears in 2 pairs).
        let subset: Vec<&Regulation> = REGS.iter().filter(|r| labels.contains(r.label)).collect();
        assert!(
            (8..=10).contains(&subset.len()),
            "Panel C subset count out of expected band: {} (expected 8..=10)",
            subset.len()
        );
    }

    #[test]
    fn render_panel_c_only_writes_non_trivial_png_en() {
        let tmp = TempDir::new().unwrap();
        let out = tmp.path().join("panel_c_en.png");
        render_panel_c_only(&out, "en").expect("EN render");
        let bytes = std::fs::read(&out).unwrap();
        assert_eq!(&bytes[0..4], &[0x89, 0x50, 0x4E, 0x47]);
        assert!(
            bytes.len() > 10_000,
            "panel C PNG suspiciously small: {} bytes",
            bytes.len()
        );
    }

    #[test]
    fn render_panel_c_only_writes_non_trivial_png_de() {
        let tmp = TempDir::new().unwrap();
        let out = tmp.path().join("panel_c_de.png");
        render_panel_c_only(&out, "de").expect("DE render");
        let size = std::fs::metadata(&out).unwrap().len();
        assert!(size > 10_000);
    }

    #[test]
    fn render_panel_c_only_writes_non_trivial_png_hi() {
        let tmp = TempDir::new().unwrap();
        let out = tmp.path().join("panel_c_hi.png");
        render_panel_c_only(&out, "hi").expect("HI render");
        let size = std::fs::metadata(&out).unwrap().len();
        assert!(size > 10_000);
    }

    #[test]
    fn render_panel_c_handles_unknown_lang() {
        let tmp = TempDir::new().unwrap();
        let out = tmp.path().join("panel_c_ja.png");
        render_panel_c_only(&out, "ja").expect("unknown-lang render");
        let size = std::fs::metadata(&out).unwrap().len();
        assert!(size > 10_000);
    }

    #[test]
    fn every_cadence_translation_exists_in_every_language() {
        for lang in ["en", "de", "fr", "it", "rm", "hi"] {
            for cad in ["ann", "rev", "stat"] {
                let key = format!("cadence_{cad}");
                let v = t(lang, &key);
                assert!(
                    !v.is_empty() && v != key,
                    "missing translation '{key}' in '{lang}' (got {v:?})"
                );
            }
        }
    }
}
