//! `agentic-figures` — the Rust port of `render_figspec.py`.
//!
//! Renders fenced ```figspec JSON blocks to PNG figures with the pure-Rust
//! `plotters` backend (no system/C deps). Real graphical types only —
//! bar / hbar / line / matrix / quadrant / flow — drawn in pixel space for full
//! layout control. [`resolve_markdown`] is the `render_figspec` equivalent: it
//! replaces each figspec block with `![caption](figures/<subdir>/<id>.png)` and
//! writes the PNGs, so the DOCX/book layer never sees raw figspec.

use std::path::Path;

use anyhow::{Context, Result, anyhow};
use plotters::prelude::*;
use plotters::style::text_anchor::{HPos, Pos, VPos};
use serde_json::Value;

// Wave 1 figspec types (image-embed, sankey, wheel, tier-matrix,
// callout-diagram, comparison-overlay, table) — Rust-foundation parity
// renderers for the AI Norms reference book.
mod render_callout;
mod render_image_embed;
mod render_overlay;
mod render_sankey;
mod render_table;
mod render_tier_matrix;
mod render_wheel;

/// Regulation-timeline figure (port of `regulation_timeline_v3_kit`).
/// First commit ships the data layer + a stub `render` API; panel
/// renderers (A / B / C) and the `LANGUAGES` per-language string
/// table land in follow-up commits.
pub mod regulation_timeline;
/// Per-language UI string table for `regulation_timeline` (port of
/// the kit's `LANGUAGES` dict). Re-exported by [`regulation_timeline`]
/// as `t(lang, key)`.
pub mod regulation_timeline_languages;
/// Panel A renderer (enforcement starts per year, stacked by
/// jurisdiction). Port of the python kit's `render_panel_A`.
pub mod regulation_timeline_panel_a;
/// Panel B renderer (hot-spots: same regulatory goal, mismatched
/// country timelines). Port of the python kit's `render_panel_B`.
pub mod regulation_timeline_panel_b;
/// Panel C renderer (per-regulation conflict-involved Gantt with
/// meta-methodology columns + conflict markers). Port of the
/// python kit's `render_panel_C` + `_render_detail`.
pub mod regulation_timeline_panel_c;

// Wong colour-blind-safe palette (matches render_figspec).
pub(crate) const NAVY: RGBColor = RGBColor(0x1F, 0x49, 0x7D);
pub(crate) const WONG: [RGBColor; 8] = [
    RGBColor(0x00, 0x72, 0xB2),
    RGBColor(0xE6, 0x9F, 0x00),
    RGBColor(0x00, 0x9E, 0x73),
    RGBColor(0xCC, 0x79, 0xA7),
    RGBColor(0xD5, 0x5E, 0x00),
    RGBColor(0x56, 0xB4, 0xE9),
    RGBColor(0xF0, 0xE4, 0x42),
    RGBColor(0x99, 0x99, 0x99),
];
pub(crate) const INK: RGBColor = RGBColor(0x1a, 0x1a, 0x1a);
pub(crate) const GRID: RGBColor = RGBColor(0xD7, 0xDD, 0xE5);
pub(crate) const GREY: RGBColor = RGBColor(0x66, 0x66, 0x66);
pub(crate) const BORDER: RGBColor = RGBColor(0x9B, 0xA7, 0xB8);
pub(crate) const HEADBG: RGBColor = RGBColor(0x1F, 0x38, 0x64);
pub(crate) const WHITEC: RGBColor = RGBColor(0xFF, 0xFF, 0xFF);
// SWOT quadrant tints (tl, tr, bl, br) matching the matplotlib renderer.
const SWOT: [RGBColor; 4] = [
    RGBColor(0xE3, 0xEE, 0xF6),
    RGBColor(0xFD, 0xF0, 0xDD),
    RGBColor(0xFA, 0xE6, 0xE6),
    RGBColor(0xE6, 0xF2, 0xEC),
];

pub(crate) fn font(size: i32) -> TextStyle<'static> {
    ("sans-serif", size).into_font().color(&INK)
}
pub(crate) fn font_c(size: i32, color: &RGBColor) -> TextStyle<'static> {
    ("sans-serif", size).into_font().color(color)
}
pub(crate) fn font_b(size: i32, color: &RGBColor) -> TextStyle<'static> {
    ("sans-serif", size)
        .into_font()
        .style(FontStyle::Bold)
        .color(color)
}
pub(crate) fn centered(style: TextStyle<'static>) -> TextStyle<'static> {
    style.pos(Pos::new(HPos::Center, VPos::Center))
}
/// Draw the figure title (centred, bold, navy) at the top of the canvas.
pub(crate) fn draw_title(a: &Area<'_>, s: &str, w: i32) -> Result<()> {
    text(a, s, &centered(font_b(22, &NAVY)), w / 2, 30)
}
/// "Nice" axis ticks from 0 to max: returns (tick_max, step).
fn nice_ticks(max: f64) -> (f64, f64) {
    if max <= 0.0 {
        return (1.0, 0.25);
    }
    let raw = max / 5.0;
    let mag = 10f64.powf(raw.log10().floor());
    let norm = raw / mag;
    let step = if norm <= 1.0 {
        1.0
    } else if norm <= 2.0 {
        2.0
    } else if norm <= 5.0 {
        5.0
    } else {
        10.0
    } * mag;
    let tick_max = (max / step).ceil() * step;
    (tick_max, step)
}

pub(crate) type Area<'a> = DrawingArea<BitMapBackend<'a>, plotters::coord::Shift>;

pub(crate) fn text(a: &Area<'_>, s: &str, st: &TextStyle<'_>, x: i32, y: i32) -> Result<()> {
    a.draw_text(s, st, (x, y))
        .map_err(|e| anyhow!("draw_text: {e}"))
}
pub(crate) fn fill_rect(
    a: &Area<'_>,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    c: &RGBColor,
) -> Result<()> {
    a.draw(&Rectangle::new([(x0, y0), (x1, y1)], c.filled()))
        .map_err(|e| anyhow!("rect: {e}"))
}
pub(crate) fn stroke_rect(
    a: &Area<'_>,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    c: &RGBColor,
    w: u32,
) -> Result<()> {
    a.draw(&Rectangle::new(
        [(x0, y0), (x1, y1)],
        ShapeStyle::from(c).stroke_width(w),
    ))
    .map_err(|e| anyhow!("rect: {e}"))
}
pub(crate) fn line(a: &Area<'_>, pts: Vec<(i32, i32)>, c: &RGBColor, w: u32) -> Result<()> {
    a.draw(&PathElement::new(pts, ShapeStyle::from(c).stroke_width(w)))
        .map_err(|e| anyhow!("line: {e}"))
}

// ---- data helpers ----
pub(crate) fn strs(v: &Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(Value::as_array)
        .map(|a| a.iter().map(json_str).collect())
        .unwrap_or_default()
}
pub(crate) fn nums(v: &Value, key: &str) -> Vec<f64> {
    v.get(key)
        .and_then(Value::as_array)
        .map(|a| a.iter().map(|x| x.as_f64().unwrap_or(0.0)).collect())
        .unwrap_or_default()
}
pub(crate) fn json_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}
pub(crate) fn wrap(s: &str, width: usize) -> Vec<String> {
    textwrap::wrap(s, width)
        .into_iter()
        .map(|c| c.to_string())
        .collect()
}

/// Deterministic 64-bit hash of a string (FNV-1a). Used by Wave-1 renderers
/// that need a seed derived from the figspec content so identical specs always
/// produce identical PNGs (no `rand` dep, no time/process variance).
pub(crate) fn fig_seed(s: &str) -> u64 {
    let mut h = 0xcbf29ce484222325_u64;
    for b in s.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

// ---- public API ----
#[derive(Debug, Clone)]
pub struct FigSpec {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub caption: String,
    pub data: Value,
}

pub fn parse(spec_json: &str) -> Result<FigSpec> {
    let v: Value = serde_json::from_str(spec_json).context("figspec JSON")?;
    Ok(FigSpec {
        id: v
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("fig")
            .to_string(),
        kind: v
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("bar")
            .to_string(),
        title: v
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        caption: v
            .get("caption")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        data: v.get("data").cloned().unwrap_or(Value::Null),
    })
}

/// Render one figspec to a PNG at `out_path`.
pub fn render_figspec(spec_json: &str, out_path: &Path) -> Result<()> {
    let spec = parse(spec_json)?;
    if let Some(p) = out_path.parent() {
        std::fs::create_dir_all(p)?;
    }
    match spec.kind.as_str() {
        "bar" => render_bar(&spec, out_path, false),
        "hbar" => render_bar(&spec, out_path, true),
        "line" => render_line(&spec, out_path),
        "matrix" => render_matrix(&spec, out_path),
        "quadrant" => render_quadrant(&spec, out_path),
        "flow" => render_flow(&spec, out_path),
        "icon" => render_icon(&spec, out_path),
        "heatmap" => render_heatmap(&spec, out_path),
        "regstack" => render_regstack(&spec, out_path),
        "govmap" => render_govmap(&spec, out_path),
        "treemap" => render_treemap(&spec, out_path),
        "procmap" => render_procmap(&spec, out_path),
        // ---- Wave 1 parity types (AI Norms reference book) ----
        "image-embed" => render_image_embed::render(&spec, out_path),
        "sankey" => render_sankey::render(&spec, out_path),
        "wheel" => render_wheel::render(&spec, out_path),
        "tier-matrix" => render_tier_matrix::render(&spec, out_path),
        "callout-diagram" => render_callout::render(&spec, out_path),
        "comparison-overlay" => render_overlay::render(&spec, out_path),
        "table" => render_table::render(&spec, out_path),
        other => Err(anyhow!("unknown figspec type '{other}'")),
    }
}

/// Replace each ```figspec block in `md` with an image reference, writing PNGs
/// to `<fig_base>/figures/<subdir>/<id>.png`. Returns the resolved markdown.
///
/// **Wave 3 (AI-Norms parity, 2026-06-03)**: figspecs of `type: "table"`
/// are intercepted and emitted as a native pipe-style markdown table
/// (with a preceding `Table: <caption>` line so the downstream
/// `fold_table_captions` pass attaches the caption to a SEQ-numbered
/// `<w:tbl>` carrying the `TableGrid` style). This is the bridge that
/// turns the 22 AI-Norms reference tables — currently shipped as
/// figspec JSON sidecars — into real Word tables that satisfy the
/// `captioned_table_parity` gate.
pub fn resolve_markdown(md: &str, fig_base: &Path, subdir: &str) -> Result<(String, usize)> {
    let figdir = fig_base.join("figures").join(subdir);
    std::fs::create_dir_all(&figdir)?;
    let mut out = String::with_capacity(md.len());
    let mut rest = md;
    let mut n = 0usize;
    while let Some(start) = rest.find("```figspec") {
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        // Skip the opening ```figspec marker. In multi-line markdown a
        // newline normally follows; in single-line markdown dumps (where
        // the entire file lives on one line, which the campaign sources
        // and several frontmatter files do) the JSON body begins
        // immediately. The previous logic — `after.find('\n').map(|i| i +
        // 1).unwrap_or(after.len())` — jumped body_start past the entire
        // remaining string when no newline existed, so the closing-fence
        // search failed and every campaign book FAILED with "unterminated
        // figspec block". Skip the opener literally; only consume one
        // following newline if present.
        const OPENER: &str = "```figspec";
        let mut body_start = OPENER.len();
        if after[body_start..].starts_with('\n') {
            body_start += 1;
        }
        let end_rel = after[body_start..]
            .find("```")
            .ok_or_else(|| anyhow!("unterminated figspec block"))?;
        let json = &after[body_start..body_start + end_rel];
        let spec = parse(json)?;
        if spec.kind == "table" {
            // Native-Word-table path: emit a markdown pipe table so the
            // downstream DocxBlock::Table renderer applies TableGrid and
            // the SEQ-numbered "Table N." caption. Image-mode rendering is
            // skipped entirely for table figspecs.
            out.push_str(&render_table_as_markdown(&spec));
            n += 1;
        } else {
            let png = figdir.join(format!("{}.png", spec.id));
            // Surface a render failure instead of silently dropping the figure: a
            // deliverable must never lose a figspec without a trace (non-repudiation).
            render_figspec(json, &png)
                .map_err(|e| anyhow!("rendering figspec '{}': {e}", spec.id))?;
            out.push_str(&format!(
                "![{}](figures/{}/{}.png)",
                spec.caption.replace(['[', ']'], ""),
                subdir,
                spec.id
            ));
            n += 1;
        }
        // skip past the closing ```
        let consumed = body_start + end_rel + 3;
        rest = &after[consumed..];
    }
    out.push_str(rest);
    Ok((out, n))
}

/// Emit a `table` figspec as a pipe-style markdown table preceded by a
/// `Table: <caption>` line. The downstream
/// `agentic_export::book::fold_table_captions` pass folds the caption into
/// the table block, and `DocxBlock::Table` then renders a TableGrid-styled
/// `<w:tbl>` with a SEQ-numbered caption.
///
/// Pipe escaping: any literal `|` inside a cell is escaped as `\|` so the
/// markdown parser does not split the cell. Newlines inside cells are
/// flattened to a single space (Word table cells receive a single inline
/// paragraph; multi-paragraph cells were never part of the reference
/// table inventory).
fn render_table_as_markdown(spec: &FigSpec) -> String {
    let header: Vec<String> = spec
        .data
        .get("header")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(json_str).collect())
        .unwrap_or_default();
    let rows: Vec<Vec<String>> = spec
        .data
        .get("rows")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|v| {
                    v.as_array()
                        .map(|c| c.iter().map(json_str).collect())
                        .unwrap_or_default()
                })
                .collect()
        })
        .unwrap_or_default();
    if header.is_empty() && rows.is_empty() {
        // Defensive: empty figspec emits a marker comment so the failure
        // is visible during review without breaking the chapter render.
        return format!("<!-- figspec/table {}: missing header/rows -->\n", spec.id);
    }
    let ncols = header
        .len()
        .max(rows.iter().map(Vec::len).max().unwrap_or(0))
        .max(1);
    let esc = |s: &str| -> String {
        s.replace('\\', "\\\\")
            .replace('|', "\\|")
            .replace('\n', " ")
            .replace('\r', " ")
    };
    let pad = |row: &[String]| -> String {
        let mut cells: Vec<String> = (0..ncols)
            .map(|c| esc(row.get(c).map(String::as_str).unwrap_or("")))
            .collect();
        if cells.iter().all(String::is_empty) {
            cells = vec!["—".into(); ncols];
        }
        format!("| {} |", cells.join(" | "))
    };

    let caption = if spec.caption.is_empty() {
        spec.title.clone()
    } else {
        spec.caption.clone()
    };
    let mut out = String::with_capacity(256 + 64 * rows.len() * ncols);
    out.push('\n');
    if !caption.is_empty() {
        // The `fold_table_captions` pass picks this up and folds it into
        // the immediately-following Table block.
        out.push_str(&format!("Table: {}\n\n", caption.trim()));
    }
    if !header.is_empty() {
        out.push_str(&pad(&header));
        out.push('\n');
        let sep: Vec<&str> = (0..ncols).map(|_| "---").collect();
        out.push_str(&format!("| {} |\n", sep.join(" | ")));
    } else {
        // Synthesize an empty header so the markdown remains a valid table.
        let blank: Vec<String> = (0..ncols).map(|_| String::new()).collect();
        out.push_str(&pad(&blank));
        out.push('\n');
        let sep: Vec<&str> = (0..ncols).map(|_| "---").collect();
        out.push_str(&format!("| {} |\n", sep.join(" | ")));
    }
    for row in &rows {
        out.push_str(&pad(row));
        out.push('\n');
    }
    out.push('\n');
    out
}

// ---- renderers (pixel space) ----
fn render_bar(spec: &FigSpec, path: &Path, horizontal: bool) -> Result<()> {
    let labels = strs(&spec.data, "labels");
    let values = nums(&spec.data, "values");
    let xlabel = spec
        .data
        .get("xlabel")
        .and_then(Value::as_str)
        .unwrap_or("");
    let n = labels.len().max(values.len()).max(1) as i32;
    let rawmax = values.iter().copied().fold(0.0_f64, f64::max);
    let (tmax, step) = nice_ticks(rawmax);
    let nt = (tmax / step).round() as i32;

    if horizontal {
        let (w, h) = (1040i32, (150 + n * 54).max(420));
        let root = BitMapBackend::new(path, (w as u32, h as u32)).into_drawing_area();
        root.fill(&WHITE).map_err(|e| anyhow!("{e}"))?;
        draw_title(&root, &spec.title, w)?;
        let (ml, mr, mt, mb) = (240, 70, 70, 64);
        let pw = w - ml - mr;
        let ph = h - mt - mb;
        for k in 0..=nt {
            let xv = ml + ((f64::from(k) * step / tmax) * f64::from(pw)) as i32;
            line(&root, vec![(xv, mt), (xv, mt + ph)], &GRID, 1)?;
            // x-axis tick numbers: +4pt (11 → 15) — clearly visible bump
            text(
                &root,
                &fmt_num(f64::from(k) * step),
                &centered(font_c(15, &GREY)),
                xv,
                mt + ph + 16,
            )?;
        }
        line(&root, vec![(ml, mt), (ml, mt + ph)], &BORDER, 1)?;
        let slot = ph / n;
        let bh = (f64::from(slot) * 0.6) as i32;
        for i in 0..n {
            let v = values.get(i as usize).copied().unwrap_or(0.0);
            let y0 = mt + i * slot + (slot - bh) / 2;
            let bw = ((v / tmax) * f64::from(pw)) as i32;
            fill_rect(&root, ml, y0, ml + bw, y0 + bh, &WONG[i as usize % 8])?;
            let lbl = labels.get(i as usize).map(String::as_str).unwrap_or("");
            for (li, ln) in wrap(lbl, 26).iter().take(2).enumerate() {
                // bar category labels: +4pt (12 → 16) — clearly visible bump
                text(
                    &root,
                    ln,
                    &font(16).pos(Pos::new(HPos::Right, VPos::Center)),
                    ml - 8,
                    y0 + bh / 2 - 9 + li as i32 * 17,
                )?;
            }
            // value labels on bars: +4pt (12 → 16) — clearly visible bump
            text(
                &root,
                &fmt_num(v),
                &font_c(16, &NAVY).pos(Pos::new(HPos::Left, VPos::Center)),
                ml + bw + 6,
                y0 + bh / 2,
            )?;
        }
        if !xlabel.is_empty() {
            // x-axis name: +4pt (13 → 17) — clearly visible bump
            text(
                &root,
                xlabel,
                &centered(font_c(17, &GREY)),
                ml + pw / 2,
                h - 18,
            )?;
        }
        root.present().map_err(|e| anyhow!("present: {e}"))?;
    } else {
        let (w, h) = (1040i32, 640i32);
        let root = BitMapBackend::new(path, (w as u32, h as u32)).into_drawing_area();
        root.fill(&WHITE).map_err(|e| anyhow!("{e}"))?;
        draw_title(&root, &spec.title, w)?;
        let (ml, mr, mt, mb) = (90, 50, 70, 96);
        let pw = w - ml - mr;
        let ph = h - mt - mb;
        for k in 0..=nt {
            let yv = mt + ph - ((f64::from(k) * step / tmax) * f64::from(ph)) as i32;
            line(&root, vec![(ml, yv), (ml + pw, yv)], &GRID, 1)?;
            // y-axis tick numbers: +4pt (11 → 15) — clearly visible bump
            text(
                &root,
                &fmt_num(f64::from(k) * step),
                &font_c(15, &GREY).pos(Pos::new(HPos::Right, VPos::Center)),
                ml - 8,
                yv,
            )?;
        }
        line(&root, vec![(ml, mt), (ml, mt + ph)], &BORDER, 1)?;
        let slot = pw / n;
        let bw = (f64::from(slot) * 0.6) as i32;
        let y1 = mt + ph;
        for i in 0..n {
            let v = values.get(i as usize).copied().unwrap_or(0.0);
            let bx = ml + i * slot + (slot - bw) / 2;
            let bh = ((v / tmax) * f64::from(ph)) as i32;
            fill_rect(&root, bx, y1 - bh, bx + bw, y1, &WONG[i as usize % 8])?;
            let lbl = labels.get(i as usize).map(String::as_str).unwrap_or("");
            for (li, ln) in wrap(lbl, 12).iter().take(2).enumerate() {
                // bar category labels (x-axis): +4pt (12 → 16) — clearly visible bump
                text(
                    &root,
                    ln,
                    &centered(font(16)),
                    bx + bw / 2,
                    y1 + 19 + li as i32 * 18,
                )?;
            }
            // value labels on bars: +4pt (12 → 16) — clearly visible bump
            text(
                &root,
                &fmt_num(v),
                &centered(font_c(16, &NAVY)),
                bx + bw / 2,
                y1 - bh - 13,
            )?;
        }
        if !xlabel.is_empty() {
            // y-axis name (rotated): +4pt (13 → 17) — clearly visible bump
            text(
                &root,
                xlabel,
                &centered(font_c(17, &GREY).transform(FontTransform::Rotate270)),
                28,
                mt + ph / 2,
            )?;
        }
        root.present().map_err(|e| anyhow!("present: {e}"))?;
    }
    Ok(())
}

fn fmt_num(v: f64) -> String {
    if (v.fract()).abs() < 1e-9 {
        format!("{}", v as i64)
    } else {
        format!("{v:.2}")
    }
}

fn render_line(spec: &FigSpec, path: &Path) -> Result<()> {
    let labels = strs(&spec.data, "labels");
    let values = nums(&spec.data, "values");
    let (w, h) = (1040i32, 600i32);
    let root = BitMapBackend::new(path, (w as u32, h as u32)).into_drawing_area();
    root.fill(&WHITE).map_err(|e| anyhow!("{e}"))?;
    draw_title(&root, &spec.title, w)?;
    let (ml, mr, mt, mb) = (90, 50, 70, 80);
    let pw = w - ml - mr;
    let ph = h - mt - mb;
    let rawmax = values.iter().copied().fold(0.0_f64, f64::max);
    let (tmax, step) = nice_ticks(rawmax);
    let nt = (tmax / step).round() as i32;
    for k in 0..=nt {
        let yv = mt + ph - ((f64::from(k) * step / tmax) * f64::from(ph)) as i32;
        line(&root, vec![(ml, yv), (ml + pw, yv)], &GRID, 1)?;
        text(
            &root,
            &fmt_num(f64::from(k) * step),
            &font_c(11, &GREY).pos(Pos::new(HPos::Right, VPos::Center)),
            ml - 8,
            yv,
        )?;
    }
    line(&root, vec![(ml, mt), (ml, mt + ph)], &BORDER, 1)?;
    let n = values.len().max(2);
    let pts: Vec<(i32, i32)> = values
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let x = ml + (i as f64 / (n - 1) as f64 * f64::from(pw)) as i32;
            let y = mt + ph - ((v / tmax) * f64::from(ph)) as i32;
            (x, y)
        })
        .collect();
    line(&root, pts.clone(), &WONG[0], 3)?;
    for (i, (x, y)) in pts.iter().enumerate() {
        fill_rect(&root, x - 4, y - 4, x + 4, y + 4, &NAVY)?;
        if let Some(l) = labels.get(i) {
            text(&root, l, &centered(font(12)), *x, mt + ph + 16)?;
        }
    }
    root.present().map_err(|e| anyhow!("present: {e}"))?;
    Ok(())
}

fn render_matrix(spec: &FigSpec, path: &Path) -> Result<()> {
    let rows = strs(&spec.data, "rows");
    let cols = strs(&spec.data, "cols");
    let cells: Vec<Vec<String>> = spec
        .data
        .get("cells")
        .and_then(Value::as_array)
        .map(|rs| {
            rs.iter()
                .map(|r| {
                    r.as_array()
                        .map(|cs| cs.iter().map(json_str).collect())
                        .unwrap_or_default()
                })
                .collect()
        })
        .unwrap_or_default();
    let nc = cols.len().max(1);
    let nr = rows.len().max(1);
    let rowlab_w = 240i32;
    let cell_w = 150i32;
    let cell_h = 64i32;
    let head_h = 60i32;
    let legend = spec
        .data
        .get("legend")
        .and_then(Value::as_str)
        .unwrap_or("");
    let legend_h = if legend.is_empty() { 0 } else { 30 };
    let (w, h) = (
        rowlab_w + nc as i32 * cell_w + 40,
        70 + head_h + nr as i32 * cell_h + 30 + legend_h,
    );
    let root = BitMapBackend::new(path, (w as u32, h as u32)).into_drawing_area();
    root.fill(&WHITE).map_err(|e| anyhow!("{e}"))?;
    draw_title(&root, &spec.title, w)?;
    let ox = 20;
    let oy = 60;
    // top-left corner cell (navy, empty) + column headers (navy, bold white)
    fill_rect(&root, ox, oy, ox + rowlab_w, oy + head_h, &HEADBG)?;
    for (c, ch) in cols.iter().enumerate() {
        let x0 = ox + rowlab_w + c as i32 * cell_w;
        fill_rect(&root, x0, oy, x0 + cell_w, oy + head_h, &HEADBG)?;
        // column headers: +4pt (13 → 17) — clearly visible bump
        for (li, ln) in wrap(ch, 14).iter().take(2).enumerate() {
            text(
                &root,
                ln,
                &centered(font_b(17, &WHITEC)),
                x0 + cell_w / 2,
                oy + 24 + li as i32 * 19,
            )?;
        }
    }
    for r in 0..nr {
        let y0 = oy + head_h + r as i32 * cell_h;
        // row label — navy fill, bold white (like the header)
        fill_rect(&root, ox, y0, ox + rowlab_w, y0 + cell_h, &HEADBG)?;
        stroke_rect(&root, ox, y0, ox + rowlab_w, y0 + cell_h, &BORDER, 1)?;
        // row labels: +4pt (12 → 16) per user readability fix (the columns
        // run through the rows, so "table columns" includes the row-anchored
        // labels and the per-row cell content)
        for (li, ln) in wrap(rows.get(r).map(String::as_str).unwrap_or(""), 26)
            .iter()
            .take(3)
            .enumerate()
        {
            text(
                &root,
                ln,
                &font_b(16, &WHITEC).pos(Pos::new(HPos::Left, VPos::Center)),
                ox + 10,
                y0 + cell_h / 2 - 14 + li as i32 * 18,
            )?;
        }
        let shade = if r % 2 == 0 {
            RGBColor(0xF4, 0xF6, 0xFA)
        } else {
            WHITEC
        };
        for c in 0..nc {
            let x0 = ox + rowlab_w + c as i32 * cell_w;
            fill_rect(&root, x0, y0, x0 + cell_w, y0 + cell_h, &shade)?;
            stroke_rect(&root, x0, y0, x0 + cell_w, y0 + cell_h, &BORDER, 1)?;
            let val = cells
                .get(r)
                .and_then(|cr| cr.get(c))
                .map(String::as_str)
                .unwrap_or("");
            // cell values: +4pt (12 → 16) per user readability fix
            for (li, ln) in wrap(val, 16).iter().take(3).enumerate() {
                text(
                    &root,
                    ln,
                    &centered(font(16)),
                    x0 + cell_w / 2,
                    y0 + cell_h / 2 - 12 + li as i32 * 18,
                )?;
            }
        }
    }
    if !legend.is_empty() {
        let ly = oy + head_h + nr as i32 * cell_h + 18;
        // legend: +4pt (11 → 15) — clearly visible bump
        text(
            &root,
            &format!("Legend: {legend}"),
            &font_c(15, &GREY).pos(Pos::new(HPos::Left, VPos::Center)),
            ox,
            ly,
        )?;
    }
    root.present().map_err(|e| anyhow!("present: {e}"))?;
    Ok(())
}

fn render_quadrant(spec: &FigSpec, path: &Path) -> Result<()> {
    let q = spec.data.get("quadrants").cloned().unwrap_or(Value::Null);
    let (w, h) = (1000i32, 760i32);
    let root = BitMapBackend::new(path, (w as u32, h as u32)).into_drawing_area();
    root.fill(&WHITE).map_err(|e| anyhow!("{e}"))?;
    draw_title(&root, &spec.title, w)?;
    let (ox, oy) = (30, 60);
    let qw = (w - 60) / 2;
    let qh = (h - 90) / 2;
    // (key, col, row, swot-tint-index)
    let cells = [
        ("tl", 0, 0, 0),
        ("tr", 1, 0, 1),
        ("bl", 0, 1, 2),
        ("br", 1, 1, 3),
    ];
    for (key, cx, cy, tint) in cells {
        let x0 = ox + cx * qw;
        let y0 = oy + cy * qh;
        fill_rect(&root, x0, y0, x0 + qw, y0 + qh, &SWOT[tint])?;
        stroke_rect(&root, x0, y0, x0 + qw, y0 + qh, &NAVY, 2)?;
        let qd = q.get(key);
        let title = qd
            .and_then(|d| d.get("title"))
            .and_then(Value::as_str)
            .unwrap_or("");
        text(
            &root,
            title,
            &centered(font_b(15, &NAVY)),
            x0 + qw / 2,
            y0 + 22,
        )?;
        let items: Vec<String> = qd
            .and_then(|d| d.get("items"))
            .and_then(Value::as_array)
            .map(|a| a.iter().map(json_str).collect())
            .unwrap_or_default();
        let mut yy = y0 + 52;
        for it in items.iter().take(6) {
            for (li, ln) in wrap(it, 40).iter().take(2).enumerate() {
                let pre = if li == 0 { "•  " } else { "   " };
                text(
                    &root,
                    &format!("{pre}{ln}"),
                    &font(12).pos(Pos::new(HPos::Left, VPos::Top)),
                    x0 + 18,
                    yy,
                )?;
                yy += 17;
            }
            yy += 3;
        }
    }
    root.present().map_err(|e| anyhow!("present: {e}"))?;
    Ok(())
}

/// Draw a connector line ending in a filled arrowhead at `end`.
pub(crate) fn arrow(a: &Area<'_>, start: (i32, i32), end: (i32, i32), c: &RGBColor) -> Result<()> {
    line(a, vec![start, end], c, 3)?;
    let (dx, dy) = ((end.0 - start.0) as f64, (end.1 - start.1) as f64);
    let len = (dx * dx + dy * dy).sqrt().max(1.0);
    let (ux, uy) = (dx / len, dy / len);
    let (px, py) = (-uy, ux);
    let s = 11.0;
    let back = (end.0 - (ux * s) as i32, end.1 - (uy * s) as i32);
    let p1 = (
        back.0 + (px * s * 0.55) as i32,
        back.1 + (py * s * 0.55) as i32,
    );
    let p2 = (
        back.0 - (px * s * 0.55) as i32,
        back.1 - (py * s * 0.55) as i32,
    );
    a.draw(&Polygon::new(vec![end, p1, p2], c.filled()))
        .map_err(|e| anyhow!("arrowhead: {e}"))?;
    Ok(())
}

fn render_flow(spec: &FigSpec, path: &Path) -> Result<()> {
    use std::collections::HashMap;
    let nodes: Vec<String> = strs(&spec.data, "nodes");
    let nodes = if nodes.is_empty() {
        strs(&spec.data, "steps")
    } else {
        nodes
    };
    let n = nodes.len();
    if n == 0 {
        // nothing to draw; emit a tiny blank canvas
        let root = BitMapBackend::new(path, (400, 120)).into_drawing_area();
        root.fill(&WHITE).map_err(|e| anyhow!("{e}"))?;
        root.present().map_err(|e| anyhow!("{e}"))?;
        return Ok(());
    }
    let nidx: HashMap<&str, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, s)| (s.as_str(), i))
        .collect();
    // edges as [from, to, label]
    let edges: Vec<(usize, usize, String)> = spec
        .data
        .get("edges")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|e| {
                    let arr = e.as_array()?;
                    let f = nidx.get(arr.first()?.as_str()?)?;
                    let t = nidx.get(arr.get(1)?.as_str()?)?;
                    let lbl = arr.get(2).and_then(Value::as_str).unwrap_or("").to_string();
                    Some((*f, *t, lbl))
                })
                .collect()
        })
        .unwrap_or_default();

    // Layer assignment via a Kahn-style topological pass that handles cycles
    // gracefully (Sugiyama's longest-path variant diverged on MAPE-K's
    // Execute → Monitor back-edge: every outer pass kept bumping layers by
    // +1, producing ~25 layers and a ~7000 px canvas; a hard clamp at
    // `n - 1` instead collapsed all cycle members to the last column). The
    // fix is a remaining-in-degree placement: at each layer, place every
    // unplaced node whose remaining in-degree equals the current minimum,
    // then decrement its successors' remaining in-degrees. For DAGs this
    // matches the topological ordering; for strongly-connected components
    // it picks the lowest-in-degree members first (preserving cycle members
    // in distinct layers rather than collapsing them).
    let mut layer = vec![0usize; n];
    if !edges.is_empty() {
        let mut indeg = vec![0i32; n];
        for (_, t, _) in &edges {
            indeg[*t] += 1;
        }
        let mut remaining_indeg = indeg.clone();
        let mut placed = vec![false; n];
        let mut current_layer = 0usize;
        let mut iterations = 0usize;
        let max_iterations = n.saturating_add(1);
        while iterations < max_iterations {
            iterations += 1;
            // Find unplaced nodes with minimum remaining in-degree.
            let unplaced: Vec<usize> = (0..n).filter(|&i| !placed[i]).collect();
            if unplaced.is_empty() {
                break;
            }
            let min_deg = unplaced
                .iter()
                .map(|&i| remaining_indeg[i])
                .min()
                .unwrap_or(0);
            let layer_nodes: Vec<usize> = unplaced
                .iter()
                .copied()
                .filter(|&i| remaining_indeg[i] == min_deg)
                .collect();
            for &i in &layer_nodes {
                layer[i] = current_layer;
                placed[i] = true;
            }
            // Decrement in-degrees of placed nodes' successors (unplaced only).
            for &f in &layer_nodes {
                for (ef, et, _) in &edges {
                    if *ef == f && !placed[*et] {
                        remaining_indeg[*et] -= 1;
                    }
                }
            }
            current_layer += 1;
        }
    } else {
        // No edges → place each node in its own column (preserves ordering).
        for (i, l) in layer.iter_mut().enumerate() {
            *l = i;
        }
    }
    let nlayers = layer.iter().copied().max().unwrap_or(0) + 1;
    let mut cols: Vec<Vec<usize>> = vec![Vec::new(); nlayers];
    for (i, &l) in layer.iter().enumerate() {
        cols[l].push(i);
    }
    let max_rows = cols.iter().map(Vec::len).max().unwrap_or(1).max(1) as i32;

    // Box width / wrap budget scaled to the longest node label so multi-line
    // wrapping stays inside the box and doesn't truncate mid-word at the
    // hard `take(3)` cap. ~7 px per character at the 13-pt box-label size.
    let max_label_chars = nodes.iter().map(|s| s.len()).max().unwrap_or(0);
    let wrap_chars = max_label_chars.div_ceil(4).clamp(22, 36) as usize;
    let approx_text_w = wrap_chars as i32 * 7 + 16;
    let box_w = approx_text_w.clamp(190, 260);
    // Tall enough for 4 wrapped lines instead of the previous 3-line cap.
    let box_h = 92i32;
    let col_gap = 110i32;
    let row_gap = 38i32;
    let (mx, my) = (32i32, 70i32);
    let w = mx * 2 + nlayers as i32 * box_w + (nlayers as i32 - 1) * col_gap;
    let inner_h = max_rows * box_h + (max_rows - 1) * row_gap;
    let h = my + inner_h + 36;
    let root = BitMapBackend::new(path, (w as u32, h as u32)).into_drawing_area();
    root.fill(&WHITE).map_err(|e| anyhow!("{e}"))?;
    draw_title(&root, &spec.title, w)?;

    // node position (top-left), centring each layer vertically.
    let mut topleft = vec![(0i32, 0i32); n];
    for (l, col) in cols.iter().enumerate() {
        let x = mx + l as i32 * (box_w + col_gap);
        let layer_h = col.len() as i32 * box_h + (col.len() as i32 - 1).max(0) * row_gap;
        let y0 = my + (inner_h - layer_h) / 2;
        for (k, &node) in col.iter().enumerate() {
            topleft[node] = (x, y0 + k as i32 * (box_h + row_gap));
        }
    }

    // edges first (under boxes)
    for (f, t, lbl) in &edges {
        let (fx, fy) = topleft[*f];
        let (tx, ty) = topleft[*t];
        let (sx, sy) = (fx + box_w, fy + box_h / 2); // src right-centre
        let (dx, dy) = (tx, ty + box_h / 2); // dst left-centre
        let midx = (sx + dx) / 2;
        // orthogonal elbow: right, vertical, right (+arrowhead at dst)
        line(&root, vec![(sx, sy), (midx, sy)], &GRID, 2)?;
        line(&root, vec![(midx, sy), (midx, dy)], &GRID, 2)?;
        arrow(&root, (midx, dy), (dx, dy), &WONG[0])?;
        if !lbl.is_empty() {
            // Edge labels are rendered at 13 pt (was 12; +1.0 pt per
            // fig07_01 readability feedback 2026-06-07) and wrap to TWO
            // LINES inside the column gap instead of truncating with an
            // ellipsis. `gap_chars` budgets ~6 px per char at 13 pt minus
            // 16 px padding; we then halve it so a two-line wrap fits the
            // same gap.
            const EDGE_FONT_PT: i32 = 13;
            const EDGE_LINE_H: i32 = 14; // line height at 13 pt
            const EDGE_MAX_LINES: usize = 2;
            let gap_chars = ((col_gap - 16) / 6).max(8) as usize;
            let per_line = (gap_chars / 2).max(10);
            // Wrap, then take at most EDGE_MAX_LINES. If the source label
            // truly exceeds the two-line budget, the LAST line is suffixed
            // with `…` so the truncation is still visible to the reader.
            let mut lines = wrap(lbl, per_line);
            let truncated = lines.len() > EDGE_MAX_LINES;
            lines.truncate(EDGE_MAX_LINES);
            if truncated {
                if let Some(last) = lines.last_mut() {
                    let cap = per_line.saturating_sub(1);
                    let prefix: String = last.chars().take(cap).collect();
                    *last = format!("{prefix}…");
                }
            }
            let line_count = lines.len() as i32;
            // Vertical centre of the label stack on the elbow's vertical
            // leg, then bias 2 px up so the stack sits clear of the
            // horizontal arrow segment at `dy`. Each line is EDGE_LINE_H
            // tall; the label rect grows with line_count so it never
            // overlaps the arrow line below.
            let centre_y = (sy + dy) / 2 - 2;
            let stack_top = centre_y - (line_count - 1) * EDGE_LINE_H / 2;
            let max_chars = lines
                .iter()
                .map(|s| s.chars().count() as i32)
                .max()
                .unwrap_or(0);
            let tw = max_chars * 6 + 8;
            let half = tw / 2;
            // Hard-clamp the white-rect span to the column-gap interior
            // so the background never paints over either box.
            let left = (midx - half).max(sx + 4);
            let right = (midx + half).min(dx - 4);
            let rect_top = stack_top - EDGE_LINE_H / 2 - 1;
            let rect_bottom = stack_top + (line_count - 1) * EDGE_LINE_H + EDGE_LINE_H / 2 + 1;
            fill_rect(&root, left, rect_top, right, rect_bottom, &WHITE)?;
            for (li, line_text) in lines.iter().enumerate() {
                let ly = stack_top + li as i32 * EDGE_LINE_H;
                text(
                    &root,
                    line_text,
                    &centered(font_c(EDGE_FONT_PT, &GREY)),
                    midx,
                    ly,
                )?;
            }
        }
    }
    // boxes
    for (i, label) in nodes.iter().enumerate() {
        let (x0, y0) = topleft[i];
        fill_rect(
            &root,
            x0,
            y0,
            x0 + box_w,
            y0 + box_h,
            &RGBColor(0xEE, 0xF2, 0xF8),
        )?;
        stroke_rect(&root, x0, y0, x0 + box_w, y0 + box_h, &NAVY, 2)?;
        let lines = wrap(label, wrap_chars);
        let max_lines = 4usize;
        let total = lines.len().min(max_lines) as i32;
        for (li, ln) in lines.iter().take(max_lines).enumerate() {
            let cy = y0 + box_h / 2 - (total - 1) * 9 + li as i32 * 17;
            text(&root, ln, &centered(font(13)), x0 + box_w / 2, cy)?;
        }
    }
    root.present().map_err(|e| anyhow!("present: {e}"))?;
    Ok(())
}

// ===========================================================================
// Book figures — Rust ports of the AI-Norms book generators (gen_icons.py,
// gen_normsmap.py, gen_regstack.py). Driven by figspec JSON so any document
// (dimension / campaign / solution) can embed them.
// ===========================================================================

pub(crate) fn fill_circle(a: &Area<'_>, x: i32, y: i32, r: i32, c: &RGBColor) -> Result<()> {
    a.draw(&Circle::new((x, y), r, c.filled()))
        .map_err(|e| anyhow!("circle: {e}"))
}
pub(crate) fn stroke_circle(
    a: &Area<'_>,
    x: i32,
    y: i32,
    r: i32,
    c: &RGBColor,
    w: u32,
) -> Result<()> {
    a.draw(&Circle::new((x, y), r, ShapeStyle::from(c).stroke_width(w)))
        .map_err(|e| anyhow!("circle: {e}"))
}
pub(crate) fn fill_poly(a: &Area<'_>, pts: Vec<(i32, i32)>, c: &RGBColor) -> Result<()> {
    a.draw(&Polygon::new(pts, c.filled()))
        .map_err(|e| anyhow!("poly: {e}"))
}
/// Parse "#RRGGBB" → RGBColor (falls back to grey).
pub(crate) fn hex_color(s: &str) -> RGBColor {
    let h = s.trim_start_matches('#');
    if h.len() == 6 {
        if let (Ok(r), Ok(g), Ok(b)) = (
            u8::from_str_radix(&h[0..2], 16),
            u8::from_str_radix(&h[2..4], 16),
            u8::from_str_radix(&h[4..6], 16),
        ) {
            return RGBColor(r, g, b);
        }
    }
    GREY
}
/// YlOrRd heat ramp over 0..=5 (matches the matplotlib `YlOrRd` cells).
fn heat_color(h: f64) -> RGBColor {
    const STOPS: [(u8, u8, u8); 6] = [
        (255, 255, 204),
        (255, 237, 160),
        (254, 178, 76),
        (253, 141, 60),
        (240, 59, 32),
        (189, 0, 38),
    ];
    let h = h.clamp(0.0, 5.0);
    let i = h.floor() as usize;
    let f = h - h.floor();
    let (a, b) = (STOPS[i.min(5)], STOPS[(i + 1).min(5)]);
    let lerp = |x: u8, y: u8| (f64::from(x) + (f64::from(y) - f64::from(x)) * f).round() as u8;
    RGBColor(lerp(a.0, b.0), lerp(a.1, b.1), lerp(a.2, b.2))
}
/// A dashed rectangle border (for "proposed/draft" chips).
fn dashed_rect(
    a: &Area<'_>,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    c: &RGBColor,
    w: u32,
) -> Result<()> {
    let dash = 7;
    let gap = 4;
    let mut seg = |ax: i32, ay: i32, bx: i32, by: i32| -> Result<()> {
        let len = (((bx - ax).pow(2) + (by - ay).pow(2)) as f64).sqrt();
        let n = (len / f64::from(dash + gap)).max(1.0) as i32;
        for k in 0..n {
            let t0 = f64::from(k) * f64::from(dash + gap) / len.max(1.0);
            let t1 = (f64::from(k) * f64::from(dash + gap) + f64::from(dash)) / len.max(1.0);
            let p0 = (
                ax + ((bx - ax) as f64 * t0) as i32,
                ay + ((by - ay) as f64 * t0) as i32,
            );
            let p1 = (
                ax + ((bx - ax) as f64 * t1.min(1.0)) as i32,
                ay + ((by - ay) as f64 * t1.min(1.0)) as i32,
            );
            line(a, vec![p0, p1], c, w)?;
        }
        Ok(())
    };
    seg(x0, y0, x1, y0)?;
    seg(x1, y0, x1, y1)?;
    seg(x1, y1, x0, y1)?;
    seg(x0, y1, x0, y0)
}

/// `gen_icons.py` → flat admonition icon (note / tip / warning) on white.
///
/// Round V (visual parity, 2026-06-03): bumped from 200×200 to 330×330 to
/// match the dominant admonition-icon resolution observed in the reference
/// `AI_Norms_and_Regulations_BOOK.docx` (`word/media/` icon images cluster
/// at 290–330 px). All shape coordinates are scaled by `S = ICON_PX / 200`
/// from the original 200-px design so the silhouette stays identical;
/// only the bitmap raster is denser. See subagent inventory icons-02.
fn render_icon(spec: &FigSpec, out: &Path) -> Result<()> {
    const ICON_PX: u32 = 330;
    let variant = spec
        .data
        .get("variant")
        .and_then(Value::as_str)
        .unwrap_or("note");
    let root = BitMapBackend::new(out, (ICON_PX, ICON_PX)).into_drawing_area();
    root.fill(&WHITEC).map_err(|e| anyhow!("fill: {e}"))?;
    let navy = RGBColor(0x1F, 0x38, 0x64);
    let green = RGBColor(0x2E, 0x7D, 0x32);
    let amber = RGBColor(0xC7, 0x7F, 0x18);
    // Scale factor from the legacy 200-px design grid → ICON_PX raster.
    let s = |v: i32| (f64::from(v) * f64::from(ICON_PX) / 200.0).round() as i32;
    match variant {
        "tip" => {
            fill_circle(&root, s(100), s(100), s(92), &green)?;
            fill_circle(&root, s(100), s(86), s(44), &WHITEC)?; // bulb
            fill_rect(&root, s(80), s(126), s(120), s(162), &WHITEC)?; // base
            fill_rect(&root, s(86), s(162), s(114), s(178), &green)?; // screw
        }
        "warning" => {
            fill_poly(
                &root,
                vec![(s(100), s(20)), (s(186), s(172)), (s(14), s(172))],
                &amber,
            )?;
            fill_rect(&root, s(92), s(64), s(108), s(130), &WHITEC)?; // !
            fill_circle(&root, s(100), s(150), s(9), &WHITEC)?;
        }
        _ => {
            fill_circle(&root, s(100), s(100), s(92), &navy)?;
            fill_circle(&root, s(100), s(58), s(13), &WHITEC)?; // dot of i
            fill_rect(&root, s(88), s(80), s(112), s(150), &WHITEC)?; // stem of i
        }
    }
    root.present().map_err(|e| anyhow!("present: {e}"))?;
    Ok(())
}

/// `gen_normsmap.py` → standards-landscape heatmap. Two balanced columns (so it
/// fits an A4 text frame): domain bands, coverage dots (filled = covered, hollow
/// = listed), per-column Reg./Ind. heat cells, gradient legend.
fn render_heatmap(spec: &FigSpec, out: &Path) -> Result<()> {
    let Some(domains) = spec.data.get("domains").and_then(Value::as_array) else {
        return Err(anyhow!("heatmap: missing data.domains"));
    };
    let nrows = |d: &Value| d.get("rows").and_then(Value::as_array).map_or(0, Vec::len);

    // Split domains into two columns, balancing total rows (left fills to ~half).
    let total_rows: usize = domains.iter().map(nrows).sum();
    let target = total_rows.div_ceil(2);
    let (mut left, mut right): (Vec<&Value>, Vec<&Value>) = (Vec::new(), Vec::new());
    let mut acc = 0usize;
    for d in domains {
        if acc < target {
            left.push(d);
            acc += nrows(d);
        } else {
            right.push(d);
        }
    }

    let w = 1120i32;
    let margin = 20i32;
    let gap = 24i32;
    let col_w = (w - margin * 2 - gap) / 2;
    let band_h = 30i32;
    let row_h = 28i32;
    let top = 102i32;
    let col_h = |ds: &[&Value]| -> i32 {
        ds.iter()
            .map(|d| band_h + 10 + nrows(d) as i32 * row_h + 12)
            .sum()
    };
    let body_h = col_h(&left).max(col_h(&right));
    let h = top + body_h + 88;

    let root = BitMapBackend::new(out, (w as u32, h as u32)).into_drawing_area();
    root.fill(&WHITEC).map_err(|e| anyhow!("fill: {e}"))?;
    if !spec.title.is_empty() {
        text(&root, &spec.title, &font_b(19, &NAVY), 22, 26)?;
    }
    text(
        &root,
        "Filled = covered here · hollow = in the landscape table · no mark = wider universe · Reg.= regulators, Ind.= industry",
        &font_c(12, &GREY),
        22,
        54,
    )?;

    let cell_w = 46i32;
    let cell_h = 22i32;
    let cell_gap = 8i32;
    let draw_col = |x0: i32, ds: &[&Value]| -> Result<()> {
        let x_ind = x0 + col_w - cell_w - 6;
        let x_reg = x_ind - cell_w - cell_gap;
        let x_dot = x0 + 14;
        let x_id = x0 + 30;
        let x_title = x0 + 150;
        text(
            &root,
            "Reg.",
            &centered(font_b(11, &NAVY)),
            x_reg + cell_w / 2,
            top - 12,
        )?;
        text(
            &root,
            "Ind.",
            &centered(font_b(11, &NAVY)),
            x_ind + cell_w / 2,
            top - 12,
        )?;
        let mut y = top;
        for d in ds {
            let name = d.get("name").and_then(Value::as_str).unwrap_or("");
            fill_rect(&root, x0, y, x0 + col_w, y + band_h, &HEADBG)?;
            text(&root, name, &font_b(13, &WHITEC), x0 + 10, y + band_h / 2)?;
            y += band_h + 10;
            for row in d
                .get("rows")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let c = row.as_array().cloned().unwrap_or_default();
                let id = c.first().map(json_str).unwrap_or_default();
                let title = c.get(1).map(json_str).unwrap_or_default();
                let rh = c.get(2).and_then(Value::as_f64).unwrap_or(0.0);
                let ih = c.get(3).and_then(Value::as_f64).unwrap_or(0.0);
                let level = c.get(4).and_then(Value::as_i64).unwrap_or(0);
                let cy = y + row_h / 2;
                if level == 2 {
                    fill_circle(&root, x_dot, cy, 6, &HEADBG)?;
                } else if level == 1 {
                    stroke_circle(&root, x_dot, cy, 6, &HEADBG, 2)?;
                }
                text(&root, &id, &font_b(12, &INK), x_id, cy)?;
                text(&root, &title, &font_c(11, &GREY), x_title, cy)?;
                for (cx, hv) in [(x_reg, rh), (x_ind, ih)] {
                    let col = heat_color(hv);
                    fill_rect(
                        &root,
                        cx,
                        cy - cell_h / 2,
                        cx + cell_w,
                        cy + cell_h / 2,
                        &col,
                    )?;
                    stroke_rect(
                        &root,
                        cx,
                        cy - cell_h / 2,
                        cx + cell_w,
                        cy + cell_h / 2,
                        &WHITEC,
                        2,
                    )?;
                    let tc = if hv >= 4.0 { WHITEC } else { INK };
                    text(
                        &root,
                        &format!("{}", hv as i64),
                        &centered(font_b(12, &tc)),
                        cx + cell_w / 2,
                        cy,
                    )?;
                }
                y += row_h;
            }
            y += 12;
        }
        Ok(())
    };
    draw_col(margin, &left)?;
    draw_col(margin + col_w + gap, &right)?;

    // gradient legend (bottom-left)
    let (lx0, lx1, ly) = (margin, margin + 360, h - 56);
    for i in 0..60 {
        let c = heat_color(f64::from(i) / 59.0 * 5.0);
        let sx = lx0 + (lx1 - lx0) * i / 60;
        fill_rect(&root, sx, ly, sx + (lx1 - lx0) / 60 + 1, ly + 18, &c)?;
    }
    stroke_rect(&root, lx0, ly, lx1, ly + 18, &BORDER, 1)?;
    text(&root, "cool 0", &font_c(12, &GREY), lx0, ly + 32)?;
    text(&root, "hot 5", &font_c(12, &GREY), lx1 - 34, ly + 32)?;
    root.present().map_err(|e| anyhow!("present: {e}"))?;
    Ok(())
}

/// `gen_regstack.py` → per-jurisdiction regulation stack: themed layers of
/// chips; solid border = in force, dashed border = proposed/draft/voluntary.
fn render_regstack(spec: &FigSpec, out: &Path) -> Result<()> {
    let layers = spec.data.get("layers").and_then(Value::as_array);
    let Some(layers) = layers else {
        return Err(anyhow!("regstack: missing data.layers"));
    };
    let w = 1120i32;
    let x0 = 24;
    let x1 = w - 24;
    let chip_h = 40;
    let chip_gap = 14;
    let row_gap = 12;
    let header_h = 34;
    let chip_w = |label: &str| -> i32 { (12 * label.chars().count() as i32 + 40).max(150) };

    // First pass: compute layout + height.
    struct Chip {
        x: i32,
        y: i32,
        w: i32,
        label: String,
        status: i64,
        edge: RGBColor,
    }
    struct Hdr {
        y: i32,
        name: String,
        band: RGBColor,
        edge: RGBColor,
    }
    let mut chips = Vec::new();
    let mut hdrs = Vec::new();
    let mut y = 70;
    for layer in layers {
        let name = layer.get("name").and_then(Value::as_str).unwrap_or("");
        let band = hex_color(
            layer
                .get("band")
                .and_then(Value::as_str)
                .unwrap_or("#ECECEC"),
        );
        let edge = hex_color(
            layer
                .get("edge")
                .and_then(Value::as_str)
                .unwrap_or("#555555"),
        );
        hdrs.push(Hdr {
            y,
            name: name.to_string(),
            band,
            edge,
        });
        y += header_h + 10;
        let mut x = x0;
        for chip in layer
            .get("chips")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let c = chip.as_array().cloned().unwrap_or_default();
            let label = c.first().map(json_str).unwrap_or_default();
            let status = c.get(1).and_then(Value::as_i64).unwrap_or(1);
            let cw = chip_w(&label);
            if x + cw > x1 && x > x0 {
                x = x0;
                y += chip_h + row_gap;
            }
            chips.push(Chip {
                x,
                y,
                w: cw,
                label,
                status,
                edge,
            });
            x += cw + chip_gap;
        }
        y += chip_h + 26;
    }
    let h = y + 30;
    let root = BitMapBackend::new(out, (w as u32, h as u32)).into_drawing_area();
    root.fill(&WHITEC).map_err(|e| anyhow!("fill: {e}"))?;
    if !spec.title.is_empty() {
        text(&root, &spec.title, &font_b(20, &NAVY), 24, 30)?;
    }
    for hd in &hdrs {
        fill_rect(&root, x0, hd.y, x1, hd.y + header_h, &hd.band)?;
        fill_rect(&root, x0, hd.y, x0 + 10, hd.y + header_h, &NAVY)?;
        text(
            &root,
            &hd.name,
            &font_b(14, &NAVY),
            x0 + 22,
            hd.y + header_h / 2,
        )?;
    }
    for c in &chips {
        fill_rect(&root, c.x, c.y, c.x + c.w, c.y + chip_h, &WHITEC)?;
        if c.status == 1 {
            stroke_rect(&root, c.x, c.y, c.x + c.w, c.y + chip_h, &c.edge, 2)?;
        } else {
            dashed_rect(&root, c.x, c.y, c.x + c.w, c.y + chip_h, &c.edge, 2)?;
        }
        text(
            &root,
            &c.label,
            &centered(font_c(13, &INK)),
            c.x + c.w / 2,
            c.y + chip_h / 2,
        )?;
    }
    root.present().map_err(|e| anyhow!("present: {e}"))?;
    Ok(())
}

/// Split an explicit-`\n` label, else word-wrap to `width` chars.
pub(crate) fn box_lines(label: &str, width: usize) -> Vec<String> {
    if label.contains('\n') {
        label.split('\n').map(str::to_string).collect()
    } else {
        wrap(label, width)
    }
}
pub(crate) fn ml_centered(
    a: &Area<'_>,
    lines: &[String],
    st: &TextStyle<'static>,
    cx: i32,
    cy: i32,
    lh: i32,
) {
    let n = lines.len() as i32;
    let y0 = cy - (n - 1) * lh / 2;
    for (i, ln) in lines.iter().enumerate() {
        let _ = text(a, ln, &centered(st.clone()), cx, y0 + i as i32 * lh);
    }
}

/// `gen_gov.py` → government-structure diagram. `layout`="branches" (L/E/J
/// columns, optional party band) or "tiers" (supranational blocs); AI/cyber
/// bodies tinted amber; a checks-&-balances panel + legend at the foot.
fn render_govmap(spec: &FigSpec, out: &Path) -> Result<()> {
    let aifill = RGBColor(0xFC, 0xE9, 0xC8);
    let aiedge = RGBColor(0xC7, 0x7F, 0x18);
    let branchfill = RGBColor(0xE6, 0xEB, 0xF3);
    let panelfill = RGBColor(0xF4, 0xF6, 0xFA);
    let panel_edge = RGBColor(0x9A, 0xA8, 0xBF);
    let box_edge = RGBColor(0x9A, 0xA8, 0xBF);

    let w = 1120i32;
    let layout = spec
        .data
        .get("layout")
        .and_then(Value::as_str)
        .unwrap_or("branches");
    let checks = strs_pairs(spec.data.get("checks"));
    let lh = 18;

    // Helper to size a box from a label.
    let box_h = |lines: usize| -> i32 { 26 + lh * lines as i32 };

    // ---- compute body geometry (pass 1) ----
    // returns the y at which the body ends (before the checks panel).
    let mut body_items: Vec<(i32, i32, i32, i32, Vec<String>, bool)> = Vec::new(); // x0,y0,w,h,lines,ai
    let mut headers: Vec<(i32, i32, i32, String)> = Vec::new(); // x0,y,w,name
    let mut arrows: Vec<(i32, i32, i32)> = Vec::new(); // cx, y_from, y_to
    let mut party: Option<String> = spec
        .data
        .get("party")
        .and_then(Value::as_str)
        .map(str::to_string);

    let mut top = 70;
    if party.is_some() {
        top = 124; // leave room for the party band at y 70..108
    }

    let body_bottom;
    if layout == "tiers" {
        party = None; // blocs have no party band
        let tiers = spec.data.get("tiers").and_then(Value::as_array);
        let mut y = top;
        let tiers = tiers.cloned().unwrap_or_default();
        let nt = tiers.len();
        for (ti, tier) in tiers.iter().enumerate() {
            let t = tier.as_array().cloned().unwrap_or_default();
            let tname = t.first().map(json_str).unwrap_or_default();
            let boxes = t
                .get(1)
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            headers.push((24, y, 0, format!("\u{25B8} {tname}"))); // tier label (w=0 → plain text)
            let m = boxes.len().max(1) as i32;
            let avail = w - 320;
            let bw = (avail / m - 16).clamp(160, 440);
            let total = bw * m + 16 * (m - 1);
            let x_start = w - 40 - total;
            let row_y = y + 6;
            let mut maxh = 0;
            for (bi, b) in boxes.iter().enumerate() {
                let c = b.as_array().cloned().unwrap_or_default();
                let label = c.first().map(json_str).unwrap_or_default();
                let ai = c.get(1).and_then(Value::as_bool).unwrap_or(false);
                let lines = box_lines(&label, 26);
                let h = box_h(lines.len());
                maxh = maxh.max(h);
                let bx = x_start + bi as i32 * (bw + 16);
                body_items.push((bx, row_y, bw, h, lines, ai));
            }
            if ti + 1 < nt {
                arrows.push((w / 2, row_y + maxh, row_y + maxh + 26));
            }
            y = row_y + maxh + 40;
        }
        body_bottom = y;
    } else {
        // three branches L / E / J
        let centers = [205, 560, 915];
        let half = 165;
        let names = [("L", "LEGISLATIVE"), ("E", "EXECUTIVE"), ("J", "JUDICIAL")];
        let mut max_bottom = top;
        for (i, (key, disp)) in names.iter().enumerate() {
            let cx = centers[i];
            headers.push((cx - half, top, half * 2, (*disp).to_string()));
            let mut y = top + 34 + 12;
            let arr = spec
                .data
                .get("branches")
                .and_then(|b| b.get(key))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            for item in &arr {
                let c = item.as_array().cloned().unwrap_or_default();
                let label = c.first().map(json_str).unwrap_or_default();
                let ai = c.get(1).and_then(Value::as_bool).unwrap_or(false);
                let lines = box_lines(&label, 30);
                let h = box_h(lines.len());
                body_items.push((cx - half, y, half * 2, h, lines, ai));
                y += h + 12;
            }
            max_bottom = max_bottom.max(y);
        }
        body_bottom = max_bottom;
    }

    // checks panel + legend geometry
    let panel_top = body_bottom + 24;
    let panel_h = if checks.is_empty() {
        0
    } else {
        40 + 22 * checks.len() as i32 + 12
    };
    let legend_y = panel_top + panel_h + 18;
    let h = legend_y + 44;

    // ---- draw (pass 2) ----
    let root = BitMapBackend::new(out, (w as u32, h as u32)).into_drawing_area();
    root.fill(&WHITEC).map_err(|e| anyhow!("fill: {e}"))?;
    if !spec.title.is_empty() {
        text(&root, &spec.title, &centered(font_b(20, &NAVY)), w / 2, 28)?;
    }
    if let Some(p) = &party {
        fill_rect(&root, 40, 70, w - 40, 108, &RGBColor(0xD9, 0x53, 0x4F))?;
        text(&root, p, &centered(font_b(15, &WHITEC)), w / 2, 89)?;
    }
    for (x0, y, bw, name) in &headers {
        if *bw == 0 {
            // tier label (italic-ish): plain navy bold, left aligned
            text(&root, name, &font_b(13, &NAVY), *x0, *y + 10)?;
        } else {
            fill_rect(&root, *x0, *y, *x0 + *bw, *y + 34, &HEADBG)?;
            text(
                &root,
                name,
                &centered(font_b(15, &WHITEC)),
                *x0 + *bw / 2,
                *y + 17,
            )?;
        }
    }
    for (cx, yf, yt) in &arrows {
        line(&root, vec![(*cx, *yf), (*cx, *yt)], &panel_edge, 2)?;
        fill_poly(
            &root,
            vec![(*cx - 6, *yt - 8), (*cx + 6, *yt - 8), (*cx, *yt)],
            &panel_edge,
        )?;
    }
    for (x0, y, bw, bh, lines, ai) in &body_items {
        let (fc, ec) = if *ai {
            (aifill, aiedge)
        } else {
            (branchfill, box_edge)
        };
        fill_rect(&root, *x0, *y, *x0 + *bw, *y + *bh, &fc)?;
        stroke_rect(
            &root,
            *x0,
            *y,
            *x0 + *bw,
            *y + *bh,
            &ec,
            if *ai { 2 } else { 1 },
        )?;
        ml_centered(
            &root,
            lines,
            &font_c(13, &INK),
            *x0 + *bw / 2,
            *y + *bh / 2,
            lh,
        );
    }
    // checks panel
    if !checks.is_empty() {
        fill_rect(
            &root,
            40,
            panel_top,
            w - 40,
            panel_top + panel_h,
            &panelfill,
        )?;
        stroke_rect(
            &root,
            40,
            panel_top,
            w - 40,
            panel_top + panel_h,
            &panel_edge,
            1,
        )?;
        text(
            &root,
            "Key checks & balances and law-making powers",
            &font_b(14, &NAVY),
            58,
            panel_top + 22,
        )?;
        let mut yy = panel_top + 52;
        for ln in &checks {
            text(&root, "\u{25B8}", &font_b(13, &aiedge), 58, yy)?;
            text(&root, ln, &font_c(13, &INK), 80, yy)?;
            yy += 22;
        }
    }
    // legend
    fill_rect(&root, 40, legend_y, 78, legend_y + 26, &aifill)?;
    stroke_rect(&root, 40, legend_y, 78, legend_y + 26, &aiedge, 2)?;
    text(
        &root,
        "= body with an AI / cybersecurity regulation role",
        &font_c(13, &INK),
        90,
        legend_y + 13,
    )?;
    root.present().map_err(|e| anyhow!("present: {e}"))?;
    Ok(())
}

/// Read a JSON array of strings (the checks list).
fn strs_pairs(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| a.iter().map(json_str).collect())
        .unwrap_or_default()
}

// ---- squarified treemap (Bruls et al.); port of the `squarify` library ----
#[derive(Clone, Copy)]
struct TRect {
    x: f64,
    y: f64,
    dx: f64,
    dy: f64,
}
fn tm_layout(sizes: &[f64], x: f64, y: f64, dx: f64, dy: f64) -> Vec<TRect> {
    let covered: f64 = sizes.iter().sum();
    let mut out = Vec::with_capacity(sizes.len());
    if dx >= dy {
        let width = covered / dy.max(1e-9);
        let mut yy = y;
        for &s in sizes {
            let h = s / width.max(1e-9);
            out.push(TRect {
                x,
                y: yy,
                dx: width,
                dy: h,
            });
            yy += h;
        }
    } else {
        let height = covered / dx.max(1e-9);
        let mut xx = x;
        for &s in sizes {
            let wv = s / height.max(1e-9);
            out.push(TRect {
                x: xx,
                y,
                dx: wv,
                dy: height,
            });
            xx += wv;
        }
    }
    out
}
fn tm_leftover(sizes: &[f64], x: f64, y: f64, dx: f64, dy: f64) -> (f64, f64, f64, f64) {
    let covered: f64 = sizes.iter().sum();
    if dx >= dy {
        let width = covered / dy.max(1e-9);
        (x + width, y, dx - width, dy)
    } else {
        let height = covered / dx.max(1e-9);
        (x, y + height, dx, dy - height)
    }
}
fn tm_worst(sizes: &[f64], x: f64, y: f64, dx: f64, dy: f64) -> f64 {
    tm_layout(sizes, x, y, dx, dy)
        .iter()
        .map(|r| (r.dx / r.dy.max(1e-9)).max(r.dy / r.dx.max(1e-9)))
        .fold(0.0, f64::max)
}
fn squarify(sizes: &[f64], x: f64, y: f64, dx: f64, dy: f64) -> Vec<TRect> {
    if sizes.is_empty() {
        return vec![];
    }
    if sizes.len() == 1 {
        return tm_layout(sizes, x, y, dx, dy);
    }
    let mut i = 1;
    while i < sizes.len()
        && tm_worst(&sizes[..i], x, y, dx, dy) >= tm_worst(&sizes[..=i], x, y, dx, dy)
    {
        i += 1;
    }
    let (lx, ly, ldx, ldy) = tm_leftover(&sizes[..i], x, y, dx, dy);
    let mut r = tm_layout(&sizes[..i], x, y, dx, dy);
    r.extend(squarify(&sizes[i..], lx, ly, ldx, ldy));
    r
}

/// `gen_popmap.py` → squarified population treemap: region-coloured tiles sized
/// by value, white borders, size-conditional labels, region legend.
fn render_treemap(spec: &FigSpec, out: &Path) -> Result<()> {
    let regions: std::collections::HashMap<String, String> = spec
        .data
        .get("regions")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("#888888").to_string()))
                .collect()
        })
        .unwrap_or_default();
    let mut items: Vec<(String, f64, String)> = spec
        .data
        .get("items")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|it| {
                    let c = it.as_array().cloned().unwrap_or_default();
                    (
                        c.first().map(json_str).unwrap_or_default(),
                        c.get(1).and_then(Value::as_f64).unwrap_or(0.0),
                        c.get(2).map(json_str).unwrap_or_default(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    if items.is_empty() {
        return Err(anyhow!("treemap: missing data.items"));
    }
    // Optional short-name aliases for tight tiles (gen_popmap SHORT fallback).
    let short: std::collections::HashMap<String, String> = spec
        .data
        .get("short")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap_or(k).to_string()))
                .collect()
        })
        .unwrap_or_default();
    items.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let w = 1120i32;
    let h = 760i32;
    let (ax, ay, aw, ah) = (
        20.0_f64,
        84.0_f64,
        f64::from(w - 40),
        f64::from(h - 84 - 72),
    );
    let total: f64 = items.iter().map(|i| i.1).sum();
    let scale = aw * ah / total.max(1e-9);
    let sizes: Vec<f64> = items.iter().map(|i| i.1 * scale).collect();
    let rects = squarify(&sizes, ax, ay, aw, ah);

    let root = BitMapBackend::new(out, (w as u32, h as u32)).into_drawing_area();
    root.fill(&WHITEC).map_err(|e| anyhow!("fill: {e}"))?;
    if !spec.title.is_empty() {
        text(&root, &spec.title, &font_b(20, &NAVY), 20, 32)?;
    }
    #[allow(clippy::cast_possible_truncation)]
    for (r, (name, val, region)) in rects.iter().zip(items.iter()) {
        let (x0, y0) = (r.x as i32, r.y as i32);
        let (x1, y1) = ((r.x + r.dx) as i32, (r.y + r.dy) as i32);
        let col = hex_color(regions.get(region).map_or("#888888", String::as_str));
        fill_rect(&root, x0, y0, x1, y1, &col)?;
        stroke_rect(&root, x0, y0, x1, y1, &WHITEC, 2)?;
        let cx = (r.x + r.dx / 2.0) as i32;
        let cy = (r.y + r.dy / 2.0) as i32;
        // Tight tiles fall back to a short alias if one is provided.
        let tight = r.dx < 80.0;
        let nm = if tight {
            short.get(name).cloned().unwrap_or_else(|| name.clone())
        } else {
            name.clone()
        };
        let val_line = if *val >= 1000.0 {
            Some(format!("{:.2} bn", val / 1000.0))
        } else if *val >= 45.0 {
            Some(format!("{} mn", *val as i64))
        } else {
            None
        };
        // Portrait tiles: rotate the label 90° so it stays readable (gen_popmap).
        if r.dy > r.dx * 1.4 && r.dy > 60.0 && r.dx > 22.0 {
            text(
                &root,
                &nm,
                &font_b(12, &WHITEC).transform(FontTransform::Rotate270),
                cx,
                cy,
            )?;
        } else if r.dx > 44.0 && r.dy > 24.0 {
            let mut lines = vec![nm];
            if r.dy > 40.0 {
                if let Some(v) = val_line {
                    lines.push(v);
                }
            }
            ml_centered(&root, &lines, &font_b(13, &WHITEC), cx, cy, 16);
        }
    }
    // region legend along the bottom
    let mut lx = 20;
    let ly = h - 44;
    for (name, col) in &regions {
        let c = hex_color(col);
        fill_rect(&root, lx, ly, lx + 22, ly + 18, &c)?;
        text(&root, name, &font_c(13, &GREY), lx + 28, ly + 9)?;
        lx += 28 + 9 * name.chars().count() as i32 + 28;
        if lx > w - 160 {
            lx = 20;
        }
    }
    root.present().map_err(|e| anyhow!("present: {e}"))?;
    Ok(())
}

/// `gen_5338.py` → process map: side-by-side panels of grouped, type-tagged
/// chips (Generic / Modified / AI-specific), with a type legend. A clean,
/// self-drawn replacement for the watermarked ISO 5338 image.
fn render_procmap(spec: &FigSpec, out: &Path) -> Result<()> {
    // type -> (fill, edge, text)
    let types: std::collections::HashMap<String, (RGBColor, RGBColor, RGBColor)> = spec
        .data
        .get("types")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .map(|(k, v)| {
                    let a = v.as_array().cloned().unwrap_or_default();
                    let col = |i: usize, d: &str| {
                        hex_color(a.get(i).map_or(d, |x| x.as_str().unwrap_or(d)))
                    };
                    (
                        k.clone(),
                        (col(0, "#EFEFEF"), col(1, "#9A9A9A"), col(2, "#333333")),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let deftype = (
        RGBColor(0xEF, 0xEF, 0xEF),
        RGBColor(0x9A, 0x9A, 0x9A),
        RGBColor(0x33, 0x33, 0x33),
    );

    let panels = spec
        .data
        .get("panels")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if panels.is_empty() {
        return Err(anyhow!("procmap: missing data.panels"));
    }
    let w = 1120i32;
    let content_x0 = 20;
    let content_w = w - 40;
    let total_weight: f64 = panels
        .iter()
        .map(|p| p.get("weight").and_then(Value::as_f64).unwrap_or(1.0))
        .sum();
    let gap = 16;
    let hdr_h = 30;
    let chip_h = 34;
    let chip_gap = 8;
    let grp_gap = 16;
    let top = 76;

    // geometry per panel
    let mut panel_x = content_x0;
    // first pass for height
    let mut max_bottom = top;
    struct Draw {
        x0: i32,
        y0: i32,
        w: i32,
        kind: u8, // 0 = header, 1 = chip
        text: String,
        ty: String,
    }
    let mut draws: Vec<Draw> = Vec::new();
    for p in &panels {
        let weight = p.get("weight").and_then(Value::as_f64).unwrap_or(1.0);
        let pw = ((weight / total_weight) * f64::from(content_w - gap * (panels.len() as i32 - 1)))
            as i32;
        let mut y = top;
        for g in p
            .get("groups")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let title = g.get("title").and_then(Value::as_str).unwrap_or("");
            let cols = g.get("cols").and_then(Value::as_u64).unwrap_or(1).max(1) as i32;
            draws.push(Draw {
                x0: panel_x,
                y0: y,
                w: pw,
                kind: 0,
                text: title.to_string(),
                ty: String::new(),
            });
            y += hdr_h + 8;
            let chips: Vec<Value> = g
                .get("chips")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let colw = (pw - (cols - 1) * 10) / cols;
            let per_col = (chips.len() as i32 + cols - 1) / cols;
            let mut col_bottoms = vec![y; cols as usize];
            for (ci, chip) in chips.iter().enumerate() {
                let col = ci as i32 / per_col.max(1);
                let col = col.min(cols - 1);
                let c = chip.as_array().cloned().unwrap_or_default();
                let label = c.first().map(json_str).unwrap_or_default();
                let ty = c.get(1).map(json_str).unwrap_or_default();
                let cx0 = panel_x + col * (colw + 10);
                draws.push(Draw {
                    x0: cx0,
                    y0: col_bottoms[col as usize],
                    w: colw,
                    kind: 1,
                    text: label,
                    ty,
                });
                col_bottoms[col as usize] += chip_h + chip_gap;
            }
            y = *col_bottoms.iter().max().unwrap_or(&y) + grp_gap;
        }
        max_bottom = max_bottom.max(y);
        panel_x += pw + gap;
    }
    let h = max_bottom + 64;

    let root = BitMapBackend::new(out, (w as u32, h as u32)).into_drawing_area();
    root.fill(&WHITEC).map_err(|e| anyhow!("fill: {e}"))?;
    if !spec.title.is_empty() {
        text(&root, &spec.title, &font_b(16, &NAVY), 20, 30)?;
    }
    for d in &draws {
        if d.kind == 0 {
            fill_rect(&root, d.x0, d.y0, d.x0 + d.w, d.y0 + hdr_h, &HEADBG)?;
            text(
                &root,
                &d.text,
                &font_b(13, &WHITEC),
                d.x0 + 8,
                d.y0 + hdr_h / 2,
            )?;
        } else {
            let (fc, ec, tc) = types.get(&d.ty).copied().unwrap_or(deftype);
            fill_rect(&root, d.x0, d.y0, d.x0 + d.w, d.y0 + chip_h, &fc)?;
            stroke_rect(&root, d.x0, d.y0, d.x0 + d.w, d.y0 + chip_h, &ec, 1)?;
            text(
                &root,
                &d.text,
                &centered(font_c(12, &tc)),
                d.x0 + d.w / 2,
                d.y0 + chip_h / 2,
            )?;
        }
    }
    // type legend along the bottom
    let legend: Vec<Value> = spec
        .data
        .get("legend")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut lx = 20;
    let ly = h - 40;
    for item in &legend {
        let c = item.as_array().cloned().unwrap_or_default();
        let label = c.first().map(json_str).unwrap_or_default();
        let ty = c.get(1).map(json_str).unwrap_or_default();
        let (fc, ec, _tc) = types.get(&ty).copied().unwrap_or(deftype);
        fill_rect(&root, lx, ly, lx + 26, ly + 20, &fc)?;
        stroke_rect(&root, lx, ly, lx + 26, ly + 20, &ec, 1)?;
        text(&root, &label, &font_c(12, &GREY), lx + 32, ly + 10)?;
        lx += 32 + 8 * label.chars().count() as i32 + 30;
    }
    root.present().map_err(|e| anyhow!("present: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_markdown_handles_single_line_dump() {
        // Regression for the 2026-06-07 campaign-book FAIL cascade:
        // single-line markdown dumps (campaign sources, several
        // frontmatter files) have no newline after the ```figspec opener,
        // and the previous body-offset logic jumped past the entire
        // string in that case, producing "unterminated figspec block"
        // for every figspec in every campaign chapter.
        let dir = std::env::temp_dir().join("agentic_fig_resolvetest");
        std::fs::create_dir_all(&dir).unwrap();
        // Two figspec blocks with NO newlines between the opener and the
        // JSON, then trailing prose.
        let single_line = "prose ```figspec{\"id\":\"a\",\"type\":\"bar\",\"title\":\"A\",\"caption\":\"c\",\"data\":{\"labels\":[\"x\"],\"values\":[1]}}``` more prose ```figspec{\"id\":\"b\",\"type\":\"bar\",\"title\":\"B\",\"caption\":\"c\",\"data\":{\"labels\":[\"y\"],\"values\":[2]}}``` end";
        let (resolved, n) =
            resolve_markdown(single_line, &dir, "resolvetest").expect("must not fail");
        assert_eq!(n, 2, "both figspec blocks must resolve");
        assert!(
            resolved.contains("![c](figures/resolvetest/a.png)"),
            "first replaced; got: {resolved}"
        );
        assert!(
            resolved.contains("![c](figures/resolvetest/b.png)"),
            "second replaced"
        );
        // Multi-line markdown still works (the legacy form).
        let multi_line = "prose\n```figspec\n{\"id\":\"c\",\"type\":\"bar\",\"title\":\"C\",\"caption\":\"cap\",\"data\":{\"labels\":[\"z\"],\"values\":[3]}}\n```\nmore";
        let (resolved2, n2) =
            resolve_markdown(multi_line, &dir, "resolvetest").expect("multi-line still works");
        assert_eq!(n2, 1);
        assert!(resolved2.contains("![cap](figures/resolvetest/c.png)"));
    }

    #[test]
    fn renders_bar_png() {
        let dir = std::env::temp_dir().join("agentic_fig_test");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("bar.png");
        let spec = r#"{"id":"t1","type":"bar","title":"Test bar","caption":"c","data":{"labels":["A","B","C"],"values":[3,7,5],"xlabel":"units"}}"#;
        render_figspec(spec, &p).unwrap();
        let meta = std::fs::metadata(&p).unwrap();
        assert!(meta.len() > 1000, "png too small: {}", meta.len());
    }

    #[test]
    fn renders_book_figure_types() {
        let dir = std::env::temp_dir().join("agentic_bookfig_test");
        std::fs::create_dir_all(&dir).unwrap();
        let cases = [
            (
                "icon",
                r#"{"id":"i","type":"icon","title":"","caption":"c","data":{"variant":"warning"}}"#,
            ),
            (
                "heatmap",
                r#"{"id":"h","type":"heatmap","title":"H","caption":"c","data":{"domains":[{"name":"D","rows":[["ISO/IEC 42001","AIMS",5,5,2],["ISO/IEC 23894","risk",4,3,1]]}]}}"#,
            ),
            (
                "regstack",
                r##"{"id":"r","type":"regstack","title":"R","caption":"c","data":{"layers":[{"name":"AI","band":"#FCEFD9","edge":"#C77F18","chips":[["EU AI Act",1],["GPAI Code",0]]}]}}"##,
            ),
            (
                "govmap_branches",
                r#"{"id":"g","type":"govmap","title":"G","caption":"c","data":{"layout":"branches","party":"CPC","branches":{"L":[["NPC",false]],"E":[["State Council",false],["CAC",true]],"J":[["SPC",false]]},"checks":["Party leadership.","No judicial review."]}}"#,
            ),
            (
                "govmap_tiers",
                r#"{"id":"gt","type":"govmap","title":"GT","caption":"c","data":{"layout":"tiers","tiers":[["Heads of state",[["ASEAN Summit",false]]],["Ministerial",[["ADGMIN",true]]]],"checks":["Consensus-based."]}}"#,
            ),
            (
                "treemap",
                r##"{"id":"tm","type":"treemap","title":"Pop","caption":"c","data":{"regions":{"Asia":"#1F3864","Europe":"#0B5C9E"},"items":[["India",1438,"Asia"],["China",1416,"Asia"],["Germany",84,"Europe"],["Switzerland",9,"Europe"]]}}"##,
            ),
            (
                "procmap",
                r##"{"id":"pm","type":"procmap","title":"5338","caption":"c","data":{"types":{"G":["#EFEFEF","#9A9A9A","#333333"],"AI":["#FCE9C8","#C77F18","#8A5A12"]},"panels":[{"weight":1,"groups":[{"title":"Technical","cols":2,"chips":[["Design (6.4.5)","G"],["AI data engineering (6.4.8)","AI"]]}]}],"legend":[["Generic","G"],["AI-specific","AI"]]}}"##,
            ),
        ];
        for (name, spec) in cases {
            let p = dir.join(format!("{name}.png"));
            render_figspec(spec, &p).unwrap();
            assert!(
                std::fs::metadata(&p).unwrap().len() > 500,
                "{name} png too small"
            );
        }
    }

    #[test]
    fn resolves_markdown_block() {
        let dir = std::env::temp_dir().join("agentic_fig_md");
        let md = "Intro\n\n```figspec\n{\"id\":\"m1\",\"type\":\"matrix\",\"title\":\"M\",\"caption\":\"cap\",\"data\":{\"rows\":[\"r1\"],\"cols\":[\"c1\"],\"cells\":[[\"x\"]]}}\n```\n\nOutro\n";
        let (out, n) = resolve_markdown(md, &dir, "sub").unwrap();
        assert_eq!(n, 1);
        assert!(out.contains("![cap](figures/sub/m1.png)"), "got: {out}");
        assert!(out.contains("Intro") && out.contains("Outro"));
    }

    /// Wave-3 (AI-Norms parity, 2026-06-03): a `type: "table"` figspec must
    /// resolve to a pipe-style markdown table with a `Table:` caption line
    /// — NOT to an image reference. This is the bridge that turns table
    /// figspecs into native `<w:tbl>` elements in the rendered docx.
    #[test]
    fn resolves_table_figspec_to_markdown_table() {
        let dir = std::env::temp_dir().join("agentic_fig_md_table");
        let md = "Lead-in.\n\n```figspec\n{\"id\":\"t1\",\"type\":\"table\",\"title\":\"Acronyms\",\"caption\":\"Acronym register\",\"data\":{\"header\":[\"Acronym\",\"Expansion\"],\"rows\":[[\"AI\",\"Artificial Intelligence\"],[\"ML\",\"Machine Learning\"]]}}\n```\n\nTrailer.\n";
        let (out, n) = resolve_markdown(md, &dir, "sub").unwrap();
        assert_eq!(n, 1);
        assert!(
            !out.contains("![") || !out.contains("(figures/sub/t1.png)"),
            "table figspec must NOT resolve to an image reference: {out}"
        );
        assert!(
            out.contains("Table: Acronym register"),
            "caption line present: {out}"
        );
        assert!(
            out.contains("| Acronym | Expansion |"),
            "header row present: {out}"
        );
        assert!(
            out.contains("| --- | --- |"),
            "separator row present: {out}"
        );
        assert!(
            out.contains("| AI | Artificial Intelligence |"),
            "body row 1: {out}"
        );
        assert!(
            out.contains("| ML | Machine Learning |"),
            "body row 2: {out}"
        );
        assert!(out.contains("Lead-in.") && out.contains("Trailer."));
    }

    /// Cells containing literal `|` characters must be escaped so the
    /// pipe-table parser does not split them into extra columns.
    #[test]
    fn table_figspec_escapes_pipes_in_cells() {
        let dir = std::env::temp_dir().join("agentic_fig_md_table_esc");
        let md = "x\n\n```figspec\n{\"id\":\"t2\",\"type\":\"table\",\"title\":\"\",\"caption\":\"\",\"data\":{\"header\":[\"A\",\"B\"],\"rows\":[[\"x|y\",\"q\"]]}}\n```\n";
        let (out, _n) = resolve_markdown(md, &dir, "sub").unwrap();
        assert!(out.contains("x\\|y"), "literal pipe must be escaped: {out}");
    }
}
