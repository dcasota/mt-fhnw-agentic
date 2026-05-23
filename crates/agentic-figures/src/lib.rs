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

// Wong colour-blind-safe palette (matches render_figspec).
const NAVY: RGBColor = RGBColor(0x1F, 0x49, 0x7D);
const WONG: [RGBColor; 8] = [
    RGBColor(0x00, 0x72, 0xB2),
    RGBColor(0xE6, 0x9F, 0x00),
    RGBColor(0x00, 0x9E, 0x73),
    RGBColor(0xCC, 0x79, 0xA7),
    RGBColor(0xD5, 0x5E, 0x00),
    RGBColor(0x56, 0xB4, 0xE9),
    RGBColor(0xF0, 0xE4, 0x42),
    RGBColor(0x99, 0x99, 0x99),
];
const INK: RGBColor = RGBColor(0x1a, 0x1a, 0x1a);
const GRID: RGBColor = RGBColor(0xD7, 0xDD, 0xE5);
const GREY: RGBColor = RGBColor(0x66, 0x66, 0x66);
const BORDER: RGBColor = RGBColor(0x9B, 0xA7, 0xB8);
const HEADBG: RGBColor = RGBColor(0x1F, 0x38, 0x64);
const WHITEC: RGBColor = RGBColor(0xFF, 0xFF, 0xFF);
// SWOT quadrant tints (tl, tr, bl, br) matching the matplotlib renderer.
const SWOT: [RGBColor; 4] = [
    RGBColor(0xE3, 0xEE, 0xF6),
    RGBColor(0xFD, 0xF0, 0xDD),
    RGBColor(0xFA, 0xE6, 0xE6),
    RGBColor(0xE6, 0xF2, 0xEC),
];

fn font(size: i32) -> TextStyle<'static> {
    ("sans-serif", size).into_font().color(&INK)
}
fn font_c(size: i32, color: &RGBColor) -> TextStyle<'static> {
    ("sans-serif", size).into_font().color(color)
}
fn font_b(size: i32, color: &RGBColor) -> TextStyle<'static> {
    ("sans-serif", size)
        .into_font()
        .style(FontStyle::Bold)
        .color(color)
}
fn centered(style: TextStyle<'static>) -> TextStyle<'static> {
    style.pos(Pos::new(HPos::Center, VPos::Center))
}
/// Draw the figure title (centred, bold, navy) at the top of the canvas.
fn draw_title(a: &Area<'_>, s: &str, w: i32) -> Result<()> {
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

type Area<'a> = DrawingArea<BitMapBackend<'a>, plotters::coord::Shift>;

fn text(a: &Area<'_>, s: &str, st: &TextStyle<'_>, x: i32, y: i32) -> Result<()> {
    a.draw_text(s, st, (x, y))
        .map_err(|e| anyhow!("draw_text: {e}"))
}
fn fill_rect(a: &Area<'_>, x0: i32, y0: i32, x1: i32, y1: i32, c: &RGBColor) -> Result<()> {
    a.draw(&Rectangle::new([(x0, y0), (x1, y1)], c.filled()))
        .map_err(|e| anyhow!("rect: {e}"))
}
fn stroke_rect(
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
fn line(a: &Area<'_>, pts: Vec<(i32, i32)>, c: &RGBColor, w: u32) -> Result<()> {
    a.draw(&PathElement::new(pts, ShapeStyle::from(c).stroke_width(w)))
        .map_err(|e| anyhow!("line: {e}"))
}

// ---- data helpers ----
fn strs(v: &Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(Value::as_array)
        .map(|a| a.iter().map(json_str).collect())
        .unwrap_or_default()
}
fn nums(v: &Value, key: &str) -> Vec<f64> {
    v.get(key)
        .and_then(Value::as_array)
        .map(|a| a.iter().map(|x| x.as_f64().unwrap_or(0.0)).collect())
        .unwrap_or_default()
}
fn json_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}
fn wrap(s: &str, width: usize) -> Vec<String> {
    textwrap::wrap(s, width)
        .into_iter()
        .map(|c| c.to_string())
        .collect()
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
        other => Err(anyhow!("unknown figspec type '{other}'")),
    }
}

/// Replace each ```figspec block in `md` with an image reference, writing PNGs
/// to `<fig_base>/figures/<subdir>/<id>.png`. Returns the resolved markdown.
pub fn resolve_markdown(md: &str, fig_base: &Path, subdir: &str) -> Result<(String, usize)> {
    let figdir = fig_base.join("figures").join(subdir);
    std::fs::create_dir_all(&figdir)?;
    let mut out = String::with_capacity(md.len());
    let mut rest = md;
    let mut n = 0usize;
    while let Some(start) = rest.find("```figspec") {
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        let body_start = after.find('\n').map(|i| i + 1).unwrap_or(after.len());
        let end_rel = after[body_start..]
            .find("```")
            .ok_or_else(|| anyhow!("unterminated figspec block"))?;
        let json = &after[body_start..body_start + end_rel];
        let spec = parse(json)?;
        let png = figdir.join(format!("{}.png", spec.id));
        if render_figspec(json, &png).is_ok() {
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
            text(
                &root,
                &fmt_num(f64::from(k) * step),
                &centered(font_c(11, &GREY)),
                xv,
                mt + ph + 14,
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
            for (li, ln) in wrap(lbl, 28).iter().take(2).enumerate() {
                text(
                    &root,
                    ln,
                    &font(12).pos(Pos::new(HPos::Right, VPos::Center)),
                    ml - 8,
                    y0 + bh / 2 - 7 + li as i32 * 14,
                )?;
            }
            text(
                &root,
                &fmt_num(v),
                &font_c(12, &NAVY).pos(Pos::new(HPos::Left, VPos::Center)),
                ml + bw + 6,
                y0 + bh / 2,
            )?;
        }
        if !xlabel.is_empty() {
            text(
                &root,
                xlabel,
                &centered(font_c(13, &GREY)),
                ml + pw / 2,
                h - 20,
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
            text(
                &root,
                &fmt_num(f64::from(k) * step),
                &font_c(11, &GREY).pos(Pos::new(HPos::Right, VPos::Center)),
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
            for (li, ln) in wrap(lbl, 14).iter().take(2).enumerate() {
                text(
                    &root,
                    ln,
                    &centered(font(12)),
                    bx + bw / 2,
                    y1 + 16 + li as i32 * 14,
                )?;
            }
            text(
                &root,
                &fmt_num(v),
                &centered(font_c(12, &NAVY)),
                bx + bw / 2,
                y1 - bh - 11,
            )?;
        }
        if !xlabel.is_empty() {
            text(
                &root,
                xlabel,
                &centered(font_c(13, &GREY).transform(FontTransform::Rotate270)),
                26,
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
        for (li, ln) in wrap(ch, 16).iter().take(2).enumerate() {
            text(
                &root,
                ln,
                &centered(font_b(13, &WHITEC)),
                x0 + cell_w / 2,
                oy + 22 + li as i32 * 16,
            )?;
        }
    }
    for r in 0..nr {
        let y0 = oy + head_h + r as i32 * cell_h;
        // row label — navy fill, bold white (like the header)
        fill_rect(&root, ox, y0, ox + rowlab_w, y0 + cell_h, &HEADBG)?;
        stroke_rect(&root, ox, y0, ox + rowlab_w, y0 + cell_h, &BORDER, 1)?;
        for (li, ln) in wrap(rows.get(r).map(String::as_str).unwrap_or(""), 30)
            .iter()
            .take(3)
            .enumerate()
        {
            text(
                &root,
                ln,
                &font_b(12, &WHITEC).pos(Pos::new(HPos::Left, VPos::Center)),
                ox + 10,
                y0 + cell_h / 2 - 12 + li as i32 * 15,
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
            for (li, ln) in wrap(val, 18).iter().take(3).enumerate() {
                text(
                    &root,
                    ln,
                    &centered(font(12)),
                    x0 + cell_w / 2,
                    y0 + cell_h / 2 - 10 + li as i32 * 14,
                )?;
            }
        }
    }
    if !legend.is_empty() {
        let ly = oy + head_h + nr as i32 * cell_h + 18;
        text(
            &root,
            &format!("Legend: {legend}"),
            &font_c(11, &GREY).pos(Pos::new(HPos::Left, VPos::Center)),
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
fn arrow(a: &Area<'_>, start: (i32, i32), end: (i32, i32), c: &RGBColor) -> Result<()> {
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

    // Longest-path layering (Sugiyama-style); cap iterations for safety on cycles.
    let mut layer = vec![0usize; n];
    for _ in 0..n {
        let mut changed = false;
        for (f, t, _) in &edges {
            if layer[*t] < layer[*f] + 1 {
                layer[*t] = layer[*f] + 1;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    // If there are no edges, fall back to a single column per index.
    if edges.is_empty() {
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

    let (box_w, box_h, col_gap, row_gap) = (190i32, 72i32, 96i32, 34i32);
    let (mx, my) = (30i32, 64i32);
    let w = mx * 2 + nlayers as i32 * box_w + (nlayers as i32 - 1) * col_gap;
    let inner_h = max_rows * box_h + (max_rows - 1) * row_gap;
    let h = my + inner_h + 30;
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
            let ly = (sy + dy) / 2;
            let tw = lbl.len() as i32 * 6 + 8;
            fill_rect(&root, midx - tw / 2, ly - 9, midx + tw / 2, ly + 9, &WHITE)?;
            text(&root, lbl, &centered(font_c(11, &GREY)), midx, ly)?;
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
        let lines = wrap(label, 20);
        let total = lines.len().min(3) as i32;
        for (li, ln) in lines.iter().take(3).enumerate() {
            let cy = y0 + box_h / 2 - (total - 1) * 8 + li as i32 * 15;
            text(&root, ln, &centered(font(12)), x0 + box_w / 2, cy)?;
        }
    }
    root.present().map_err(|e| anyhow!("present: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn resolves_markdown_block() {
        let dir = std::env::temp_dir().join("agentic_fig_md");
        let md = "Intro\n\n```figspec\n{\"id\":\"m1\",\"type\":\"matrix\",\"title\":\"M\",\"caption\":\"cap\",\"data\":{\"rows\":[\"r1\"],\"cols\":[\"c1\"],\"cells\":[[\"x\"]]}}\n```\n\nOutro\n";
        let (out, n) = resolve_markdown(md, &dir, "sub").unwrap();
        assert_eq!(n, 1);
        assert!(out.contains("![cap](figures/sub/m1.png)"), "got: {out}");
        assert!(out.contains("Intro") && out.contains("Outro"));
    }
}
