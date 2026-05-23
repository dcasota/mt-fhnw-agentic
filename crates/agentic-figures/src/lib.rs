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

fn font(size: i32) -> TextStyle<'static> {
    ("sans-serif", size).into_font().color(&INK)
}
fn font_c(size: i32, color: &RGBColor) -> TextStyle<'static> {
    ("sans-serif", size).into_font().color(color)
}
fn centered(style: TextStyle<'static>) -> TextStyle<'static> {
    style.pos(Pos::new(HPos::Center, VPos::Center))
}

type Area<'a> = DrawingArea<BitMapBackend<'a>, plotters::coord::Shift>;

fn text(a: &Area<'_>, s: &str, st: &TextStyle<'_>, x: i32, y: i32) -> Result<()> {
    a.draw_text(s, st, (x, y)).map_err(|e| anyhow!("draw_text: {e}"))
}
fn fill_rect(a: &Area<'_>, x0: i32, y0: i32, x1: i32, y1: i32, c: &RGBColor) -> Result<()> {
    a.draw(&Rectangle::new([(x0, y0), (x1, y1)], c.filled()))
        .map_err(|e| anyhow!("rect: {e}"))
}
fn stroke_rect(a: &Area<'_>, x0: i32, y0: i32, x1: i32, y1: i32, c: &RGBColor, w: u32) -> Result<()> {
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
    textwrap::wrap(s, width).into_iter().map(|c| c.to_string()).collect()
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
        id: v.get("id").and_then(Value::as_str).unwrap_or("fig").to_string(),
        kind: v.get("type").and_then(Value::as_str).unwrap_or("bar").to_string(),
        title: v.get("title").and_then(Value::as_str).unwrap_or("").to_string(),
        caption: v.get("caption").and_then(Value::as_str).unwrap_or("").to_string(),
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
    let xlabel = spec.data.get("xlabel").and_then(Value::as_str).unwrap_or("");
    let n = labels.len().max(values.len()).max(1);
    let (w, h) = (1000i32, (220 + n as i32 * if horizontal { 56 } else { 0 }).max(620));
    let root = BitMapBackend::new(path, (w as u32, h as u32)).into_drawing_area();
    root.fill(&WHITE).map_err(|e| anyhow!("{e}"))?;
    text(&root, &spec.title, &centered(font(22)), w / 2, 30)?;
    let maxv = values.iter().cloned().fold(1.0_f64, f64::max) * 1.18;
    let (ml, mr, mt, mb) = (70, 40, 70, 70);
    let pw = w - ml - mr;
    let ph = h - mt - mb;
    if horizontal {
        let bh = (ph / n as i32).min(60);
        for i in 0..n {
            let v = values.get(i).copied().unwrap_or(0.0);
            let y0 = mt + i as i32 * (ph / n as i32) + 6;
            let y1 = y0 + bh - 6;
            let bw = ((v / maxv) * pw as f64) as i32;
            fill_rect(&root, ml, y0, ml + bw, y1, &WONG[i % 8])?;
            text(&root, labels.get(i).map(String::as_str).unwrap_or(""),
                 &font(13).pos(Pos::new(HPos::Right, VPos::Center)), ml - 6, (y0 + y1) / 2)?;
            text(&root, &fmt_num(v), &font_c(12, &INK).pos(Pos::new(HPos::Left, VPos::Center)),
                 ml + bw + 6, (y0 + y1) / 2)?;
        }
    } else {
        let slot = pw / n as i32;
        let bw = (slot as f64 * 0.6) as i32;
        for i in 0..n {
            let v = values.get(i).copied().unwrap_or(0.0);
            let bx = ml + i as i32 * slot + (slot - bw) / 2;
            let bh = ((v / maxv) * ph as f64) as i32;
            let y1 = mt + ph;
            fill_rect(&root, bx, y1 - bh, bx + bw, y1, &WONG[i % 8])?;
            for (li, ln) in wrap(labels.get(i).map(String::as_str).unwrap_or(""), 14).iter().take(2).enumerate() {
                text(&root, ln, &centered(font(12)), bx + bw / 2, y1 + 14 + li as i32 * 14)?;
            }
            text(&root, &fmt_num(v), &centered(font_c(12, &NAVY)), bx + bw / 2, y1 - bh - 12)?;
        }
        line(&root, vec![(ml, mt + ph), (ml + pw, mt + ph)], &GRID, 1)?;
    }
    if !xlabel.is_empty() {
        text(&root, xlabel, &centered(font_c(13, &INK)), w / 2, h - 24)?;
    }
    root.present().map_err(|e| anyhow!("present: {e}"))?;
    Ok(())
}

fn fmt_num(v: f64) -> String {
    if (v.fract()).abs() < 1e-9 { format!("{}", v as i64) } else { format!("{v:.2}") }
}

fn render_line(spec: &FigSpec, path: &Path) -> Result<()> {
    let labels = strs(&spec.data, "labels");
    let values = nums(&spec.data, "values");
    let (w, h) = (1000i32, 600i32);
    let root = BitMapBackend::new(path, (w as u32, h as u32)).into_drawing_area();
    root.fill(&WHITE).map_err(|e| anyhow!("{e}"))?;
    text(&root, &spec.title, &centered(font(22)), w / 2, 30)?;
    let (ml, mr, mt, mb) = (70, 40, 70, 80);
    let pw = w - ml - mr; let ph = h - mt - mb;
    let maxv = values.iter().cloned().fold(1.0_f64, f64::max) * 1.15;
    let n = values.len().max(2);
    line(&root, vec![(ml, mt), (ml, mt + ph), (ml + pw, mt + ph)], &INK, 1)?;
    let pts: Vec<(i32, i32)> = values.iter().enumerate().map(|(i, v)| {
        let x = ml + (i as f64 / (n - 1) as f64 * pw as f64) as i32;
        let y = mt + ph - ((v / maxv) * ph as f64) as i32;
        (x, y)
    }).collect();
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
    let cells: Vec<Vec<String>> = spec.data.get("cells").and_then(Value::as_array).map(|rs| {
        rs.iter().map(|r| r.as_array().map(|cs| cs.iter().map(json_str).collect()).unwrap_or_default()).collect()
    }).unwrap_or_default();
    let nc = cols.len().max(1);
    let nr = rows.len().max(1);
    let rowlab_w = 240i32;
    let cell_w = 150i32;
    let cell_h = 64i32;
    let head_h = 60i32;
    let (w, h) = (rowlab_w + nc as i32 * cell_w + 40, 70 + head_h + nr as i32 * cell_h + 40);
    let root = BitMapBackend::new(path, (w as u32, h as u32)).into_drawing_area();
    root.fill(&WHITE).map_err(|e| anyhow!("{e}"))?;
    text(&root, &spec.title, &centered(font(22)), w / 2, 28)?;
    let ox = 20; let oy = 60;
    // header
    for (c, ch) in cols.iter().enumerate() {
        let x0 = ox + rowlab_w + c as i32 * cell_w;
        fill_rect(&root, x0, oy, x0 + cell_w, oy + head_h, &NAVY)?;
        for (li, ln) in wrap(ch, 16).iter().take(2).enumerate() {
            text(&root, ln, &centered(font_c(13, &WHITE)), x0 + cell_w / 2, oy + 22 + li as i32 * 16)?;
        }
    }
    for r in 0..nr {
        let y0 = oy + head_h + r as i32 * cell_h;
        // row label
        let shade = if r % 2 == 0 { RGBColor(0xF4, 0xF6, 0xFA) } else { WHITE };
        fill_rect(&root, ox, y0, ox + rowlab_w, y0 + cell_h, &shade)?;
        stroke_rect(&root, ox, y0, ox + rowlab_w, y0 + cell_h, &GRID, 1)?;
        for (li, ln) in wrap(rows.get(r).map(String::as_str).unwrap_or(""), 30).iter().take(3).enumerate() {
            text(&root, ln, &font(12).pos(Pos::new(HPos::Left, VPos::Center)),
                 ox + 8, y0 + cell_h / 2 - 12 + li as i32 * 15)?;
        }
        for c in 0..nc {
            let x0 = ox + rowlab_w + c as i32 * cell_w;
            fill_rect(&root, x0, y0, x0 + cell_w, y0 + cell_h, &shade)?;
            stroke_rect(&root, x0, y0, x0 + cell_w, y0 + cell_h, &GRID, 1)?;
            let val = cells.get(r).and_then(|cr| cr.get(c)).map(String::as_str).unwrap_or("");
            for (li, ln) in wrap(val, 18).iter().take(3).enumerate() {
                text(&root, ln, &centered(font(12)), x0 + cell_w / 2, y0 + cell_h / 2 - 10 + li as i32 * 14)?;
            }
        }
    }
    root.present().map_err(|e| anyhow!("present: {e}"))?;
    Ok(())
}

fn render_quadrant(spec: &FigSpec, path: &Path) -> Result<()> {
    let q = spec.data.get("quadrants").cloned().unwrap_or(Value::Null);
    let (w, h) = (1000i32, 760i32);
    let root = BitMapBackend::new(path, (w as u32, h as u32)).into_drawing_area();
    root.fill(&WHITE).map_err(|e| anyhow!("{e}"))?;
    text(&root, &spec.title, &centered(font(22)), w / 2, 28)?;
    let (ox, oy) = (30, 60);
    let qw = (w - 60) / 2; let qh = (h - 90) / 2;
    let cells = [("tl", 0, 0), ("tr", 1, 0), ("bl", 0, 1), ("br", 1, 1)];
    for (i, (key, cx, cy)) in cells.iter().enumerate() {
        let x0 = ox + cx * qw; let y0 = oy + cy * qh;
        fill_rect(&root, x0, y0, x0 + qw, y0 + qh, &RGBColor(0xEE, 0xF2, 0xF8))?;
        stroke_rect(&root, x0, y0, x0 + qw, y0 + qh, &NAVY, 2)?;
        let qd = q.get(key);
        let title = qd.and_then(|d| d.get("title")).and_then(Value::as_str).unwrap_or("");
        text(&root, title, &font_c(15, &WONG[i % 8]).pos(Pos::new(HPos::Left, VPos::Top)), x0 + 14, y0 + 12)?;
        let items: Vec<String> = qd.and_then(|d| d.get("items")).and_then(Value::as_array)
            .map(|a| a.iter().map(json_str).collect()).unwrap_or_default();
        let mut yy = y0 + 46;
        for it in items.iter().take(6) {
            for (li, ln) in wrap(it, 40).iter().take(2).enumerate() {
                let pre = if li == 0 { "• " } else { "  " };
                text(&root, &format!("{pre}{ln}"), &font(12).pos(Pos::new(HPos::Left, VPos::Top)), x0 + 16, yy)?;
                yy += 16;
            }
            yy += 2;
        }
    }
    root.present().map_err(|e| anyhow!("present: {e}"))?;
    Ok(())
}

fn render_flow(spec: &FigSpec, path: &Path) -> Result<()> {
    // Simple left-to-right layered flow: nodes are drawn as boxes in sequence
    // with arrows between consecutive nodes. (A full layered/orthogonal router
    // is a later refinement; this avoids overlap by one node per column.)
    let nodes: Vec<String> = spec.data.get("nodes").and_then(Value::as_array)
        .map(|a| a.iter().map(|nd| nd.get("label").and_then(Value::as_str).map(str::to_string)
            .unwrap_or_else(|| json_str(nd))).collect())
        .unwrap_or_else(|| strs(&spec.data, "steps"));
    let n = nodes.len().max(1);
    let box_w = 200i32; let box_h = 90i32; let gap = 60i32;
    let per_row = ((1100 - 40) / (box_w + gap)).max(1);
    let rows_cnt = (n as i32 + per_row - 1) / per_row;
    let (w, h) = (40 + per_row.min(n as i32) * (box_w + gap), 80 + rows_cnt * (box_h + gap));
    let root = BitMapBackend::new(path, (w as u32, h as u32)).into_drawing_area();
    root.fill(&WHITE).map_err(|e| anyhow!("{e}"))?;
    text(&root, &spec.title, &centered(font(22)), w / 2, 28)?;
    let mut centers = Vec::new();
    for (i, label) in nodes.iter().enumerate() {
        let col = i as i32 % per_row; let row = i as i32 / per_row;
        let x0 = 20 + col * (box_w + gap); let y0 = 60 + row * (box_h + gap);
        fill_rect(&root, x0, y0, x0 + box_w, y0 + box_h, &RGBColor(0xEE, 0xF2, 0xF8))?;
        stroke_rect(&root, x0, y0, x0 + box_w, y0 + box_h, &NAVY, 2)?;
        for (li, ln) in wrap(label, 24).iter().take(3).enumerate() {
            text(&root, ln, &centered(font(13)), x0 + box_w / 2, y0 + box_h / 2 - 12 + li as i32 * 16)?;
        }
        centers.push((x0 + box_w / 2, y0, x0 + box_w, y0 + box_h / 2));
    }
    for i in 1..nodes.len() {
        let (_, _, prev_r_x, prev_r_y) = centers[i - 1];
        let (cur_cx, cur_top, _, cur_l_y) = centers[i];
        if i as i32 % per_row == 0 {
            // wrap to next row: arrow down into the top of the first box
            line(&root, vec![(prev_r_x, prev_r_y), (cur_cx, cur_top)], &WONG[0], 2)?;
        } else {
            line(&root, vec![(prev_r_x, prev_r_y), (cur_cx - box_w / 2, cur_l_y)], &WONG[0], 2)?;
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
