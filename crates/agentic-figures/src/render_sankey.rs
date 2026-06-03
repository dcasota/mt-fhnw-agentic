//! `sankey` figspec — weighted-flow diagram with column layout.
//!
//! Wave-1 parity renderer for the AI Norms book figures:
//!   - Morgan Stanley humanoid value-chain flow
//!   - Humanoid-100 component fan-out
//!   - AI data-centre stack
//!
//! Expected figspec shape:
//! ```json
//! {
//!   "id": "...", "type": "sankey", "title": "...", "caption": "...",
//!   "data": {
//!     "nodes": [{"id":"a","label":"Alpha","column":0,"color":"#1F497D"}, ...],
//!     "flows": [{"source":"a","target":"b","weight":5.0}, ...]
//!   }
//! }
//! ```

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Result, anyhow};
use plotters::prelude::*;
use serde_json::Value;

use crate::{
    Area, FigSpec, GREY, INK, WHITEC, WONG, draw_title, fig_seed, fill_rect, font_b, font_c,
    hex_color, stroke_rect, text,
};

#[derive(Clone)]
struct Node {
    label: String,
    column: i64,
    color: RGBColor,
}

fn parse_nodes(v: &Value) -> Vec<(String, Node)> {
    v.get("nodes")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .enumerate()
                .map(|(i, n)| {
                    let id = n
                        .get("id")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("n{i}"));
                    let label = n
                        .get("label")
                        .and_then(Value::as_str)
                        .unwrap_or(&id)
                        .to_string();
                    let column = n.get("column").and_then(Value::as_i64).unwrap_or(0);
                    let color = n
                        .get("color")
                        .and_then(Value::as_str)
                        .map_or_else(|| WONG[i % WONG.len()], hex_color);
                    (
                        id,
                        Node {
                            label,
                            column,
                            color,
                        },
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

#[derive(Clone, Copy)]
struct Flow {
    src: usize,
    dst: usize,
    weight: f64,
}

pub fn render(spec: &FigSpec, out_path: &Path) -> Result<()> {
    // Deterministic seed (unused for layout decisions, but a stable per-figure
    // hash is required by Wave-1 contract; any future jitter MUST derive from
    // this seed and this seed alone).
    let _seed = fig_seed(&serde_json::to_string(&spec.data).unwrap_or_default());

    let nodes = parse_nodes(&spec.data);
    if nodes.is_empty() {
        return Err(anyhow!("sankey: missing data.nodes"));
    }
    let idx: HashMap<String, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, (id, _))| (id.clone(), i))
        .collect();
    let flows: Vec<Flow> = spec
        .data
        .get("flows")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|f| {
                    let s = f.get("source").and_then(Value::as_str)?;
                    let d = f.get("target").and_then(Value::as_str)?;
                    let w = f.get("weight").and_then(Value::as_f64).unwrap_or(1.0);
                    Some(Flow {
                        src: *idx.get(s)?,
                        dst: *idx.get(d)?,
                        weight: w.max(0.0),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    // Group by column → stable y-order = order-of-appearance per column.
    let max_col = nodes.iter().map(|(_, n)| n.column).max().unwrap_or(0);
    let mut cols: Vec<Vec<usize>> = vec![Vec::new(); (max_col + 1) as usize];
    for (i, (_, n)) in nodes.iter().enumerate() {
        let c = n.column.clamp(0, max_col) as usize;
        cols[c].push(i);
    }

    // Canvas geometry.
    let (w, h) = (1120i32, 640i32);
    let (ml, mr, mt, mb) = (40i32, 40i32, 70i32, 50i32);
    let pw = w - ml - mr;
    let ph = h - mt - mb;
    let n_cols = cols.len() as i32;
    let node_w = 28i32;
    let col_gap = (pw - n_cols * node_w) / (n_cols - 1).max(1);

    // Each node's "outflow" = sum of flows leaving it; "inflow" = sum entering.
    // Height proportional to max(outflow, inflow). Total column weight scales ph.
    let mut out_w = vec![0.0_f64; nodes.len()];
    let mut in_w = vec![0.0_f64; nodes.len()];
    for f in &flows {
        out_w[f.src] += f.weight;
        in_w[f.dst] += f.weight;
    }
    let node_total: Vec<f64> = out_w
        .iter()
        .zip(in_w.iter())
        .map(|(o, i)| o.max(*i).max(1.0))
        .collect();
    let col_totals: Vec<f64> = cols
        .iter()
        .map(|members| members.iter().map(|&i| node_total[i]).sum::<f64>().max(1.0))
        .collect();
    let scale_for_col: Vec<f64> = col_totals.iter().map(|t| f64::from(ph - 40) / t).collect();

    // Position each node (top-left + h).
    let mut node_pos: Vec<(i32, i32, i32)> = vec![(0, 0, 0); nodes.len()]; // x, y, h
    for (ci, members) in cols.iter().enumerate() {
        let x = ml + ci as i32 * (node_w + col_gap);
        let mut y = mt + 20;
        for &m in members {
            #[allow(clippy::cast_possible_truncation)]
            let nh = (node_total[m] * scale_for_col[ci]).max(18.0) as i32;
            node_pos[m] = (x, y, nh);
            y += nh + 10;
        }
    }

    // Render.
    let root = BitMapBackend::new(out_path, (w as u32, h as u32)).into_drawing_area();
    root.fill(&WHITEC).map_err(|e| anyhow!("{e}"))?;
    draw_title(&root, &spec.title, w)?;

    // Flow ribbons first (under nodes). Each node tracks its current source-y /
    // dest-y cursors so stacked flows do not overlap.
    let mut src_cursor: Vec<i32> = node_pos.iter().map(|(_, y, _)| *y).collect();
    let mut dst_cursor: Vec<i32> = node_pos.iter().map(|(_, y, _)| *y).collect();
    for f in &flows {
        let (sx, sy, sh) = node_pos[f.src];
        let (dx, dy, dh) = node_pos[f.dst];
        let _ = (sy, dy); // silence: cursors track these
        #[allow(clippy::cast_possible_truncation)]
        let band_src =
            (f.weight * scale_for_col[nodes[f.src].1.column.max(0) as usize]).max(3.0) as i32;
        #[allow(clippy::cast_possible_truncation)]
        let band_dst =
            (f.weight * scale_for_col[nodes[f.dst].1.column.max(0) as usize]).max(3.0) as i32;
        let ay0 = src_cursor[f.src];
        let ay1 = (ay0 + band_src).min(node_pos[f.src].1 + sh);
        let by0 = dst_cursor[f.dst];
        let by1 = (by0 + band_dst).min(node_pos[f.dst].1 + dh);
        src_cursor[f.src] = ay1;
        dst_cursor[f.dst] = by1;
        let x0 = sx + node_w;
        let x1 = dx;
        let color = nodes[f.src].1.color;
        // Quadrilateral ribbon.
        draw_ribbon(&root, x0, ay0, ay1, x1, by0, by1, &color)?;
    }

    // Nodes on top.
    for (i, (_, n)) in nodes.iter().enumerate() {
        let (x, y, nh) = node_pos[i];
        fill_rect(&root, x, y, x + node_w, y + nh, &n.color)?;
        stroke_rect(&root, x, y, x + node_w, y + nh, &INK, 1)?;
        // Label to the right of the rectangle (or to the left for last column).
        let last_col = nodes[i].1.column == max_col;
        if last_col {
            text(
                &root,
                &n.label,
                &font_b(13, &INK).pos(plotters::style::text_anchor::Pos::new(
                    plotters::style::text_anchor::HPos::Right,
                    plotters::style::text_anchor::VPos::Center,
                )),
                x - 6,
                y + nh / 2,
            )?;
        } else {
            text(
                &root,
                &n.label,
                &font_b(13, &INK),
                x + node_w + 6,
                y + nh / 2,
            )?;
        }
    }
    // Legend strip at bottom-left.
    text(
        &root,
        "ribbon width = weight",
        &font_c(11, &GREY),
        ml,
        h - 20,
    )?;
    root.present().map_err(|e| anyhow!("present: {e}"))?;
    Ok(())
}

/// Filled quadrilateral with two parallel left/right edges (Sankey ribbon).
fn draw_ribbon(
    a: &Area<'_>,
    x0: i32,
    sy0: i32,
    sy1: i32,
    x1: i32,
    dy0: i32,
    dy1: i32,
    color: &RGBColor,
) -> Result<()> {
    // Faint translucent fill (~38% alpha equivalent achieved by lightening).
    let lighten = |c: &RGBColor| {
        RGBColor(
            ((u16::from(c.0) + 255 * 2) / 3) as u8,
            ((u16::from(c.1) + 255 * 2) / 3) as u8,
            ((u16::from(c.2) + 255 * 2) / 3) as u8,
        )
    };
    let fill_c = lighten(color);
    let pts = vec![(x0, sy0), (x1, dy0), (x1, dy1), (x0, sy1)];
    a.draw(&Polygon::new(pts.clone(), fill_c.filled()))
        .map_err(|e| anyhow!("ribbon: {e}"))?;
    a.draw(&PathElement::new(
        vec![(x0, sy0), (x1, dy0)],
        ShapeStyle::from(color).stroke_width(1),
    ))
    .map_err(|e| anyhow!("ribbon top: {e}"))?;
    a.draw(&PathElement::new(
        vec![(x0, sy1), (x1, dy1)],
        ShapeStyle::from(color).stroke_width(1),
    ))
    .map_err(|e| anyhow!("ribbon bot: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;

    #[test]
    fn renders_simple_sankey() {
        let dir = std::env::temp_dir().join("agentic_fig_sankey_t1");
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("sankey.png");
        let spec_json = r##"{"id":"s1","type":"sankey","title":"Stack","caption":"c","data":{"nodes":[{"id":"a","label":"Raw","column":0,"color":"#1F497D"},{"id":"b","label":"Refined","column":1,"color":"#0072B2"},{"id":"c","label":"Output","column":2,"color":"#009E73"}],"flows":[{"source":"a","target":"b","weight":4},{"source":"b","target":"c","weight":3},{"source":"a","target":"c","weight":1}]}}"##;
        let spec = parse(spec_json).unwrap();
        render(&spec, &out).unwrap();
        let meta = std::fs::metadata(&out).unwrap();
        assert!(meta.len() > 1000, "sankey png too small: {}", meta.len());
    }

    #[test]
    fn rejects_missing_nodes() {
        let dir = std::env::temp_dir().join("agentic_fig_sankey_t2");
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("sankey_err.png");
        let spec =
            parse(r#"{"id":"s2","type":"sankey","title":"","caption":"","data":{"flows":[]}}"#)
                .unwrap();
        assert!(render(&spec, &out).is_err());
    }
}
