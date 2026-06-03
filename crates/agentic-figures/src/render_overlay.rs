//! `comparison-overlay` figspec — two-circle Venn (or side-by-side pane) for
//! ISO/NIST framework deltas, regulatory diff views, etc.
//!
//! Expected figspec shape:
//! ```json
//! {
//!   "id": "...", "type": "comparison-overlay", "title": "...", "caption": "...",
//!   "data": {
//!     "mode": "venn",                       // or "side-by-side" (default "venn")
//!     "left":  {"label":"ISO 42001",  "items":["AIMS","Risk mgmt"]},
//!     "right": {"label":"NIST AI RMF","items":["GOVERN","MAP","MEASURE","MANAGE"]},
//!     "intersection": ["Risk mgmt"]
//!   }
//! }
//! ```

use std::path::Path;

use anyhow::{Result, anyhow};
use plotters::prelude::*;
use serde_json::Value;

use crate::{
    BORDER, FigSpec, NAVY, WHITEC, centered, draw_title, fig_seed, fill_circle, fill_rect, font,
    font_b, stroke_circle, stroke_rect, text, wrap,
};

struct Side {
    label: String,
    items: Vec<String>,
}

fn parse_side(v: Option<&Value>, default_label: &str) -> Side {
    let l = v
        .and_then(|x| x.get("label"))
        .and_then(Value::as_str)
        .unwrap_or(default_label)
        .to_string();
    let items = v
        .and_then(|x| x.get("items"))
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|i| i.as_str().unwrap_or("").to_string())
                .collect()
        })
        .unwrap_or_default();
    Side { label: l, items }
}

pub fn render(spec: &FigSpec, out_path: &Path) -> Result<()> {
    let _seed = fig_seed(&serde_json::to_string(&spec.data).unwrap_or_default());

    let mode = spec
        .data
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or("venn");
    let left = parse_side(spec.data.get("left"), "Left");
    let right = parse_side(spec.data.get("right"), "Right");
    let intersection: Vec<String> = spec
        .data
        .get("intersection")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|i| i.as_str().unwrap_or("").to_string())
                .collect()
        })
        .unwrap_or_default();

    if left.items.is_empty() && right.items.is_empty() && intersection.is_empty() {
        return Err(anyhow!(
            "comparison-overlay: no items in left/right/intersection"
        ));
    }

    match mode {
        "side-by-side" | "side_by_side" => {
            render_pane(spec, out_path, &left, &right, &intersection)
        }
        _ => render_venn(spec, out_path, &left, &right, &intersection),
    }
}

fn render_venn(
    spec: &FigSpec,
    out_path: &Path,
    left: &Side,
    right: &Side,
    intersection: &[String],
) -> Result<()> {
    let (w, h) = (1100i32, 720i32);
    let root = BitMapBackend::new(out_path, (w as u32, h as u32)).into_drawing_area();
    root.fill(&WHITEC).map_err(|e| anyhow!("{e}"))?;
    draw_title(&root, &spec.title, w)?;

    let r = 230;
    let cy = h / 2 + 10;
    let cx_l = w / 2 - r * 6 / 10;
    let cx_r = w / 2 + r * 6 / 10;

    // Transparent-feel: very pale fills.
    let pale_l = RGBColor(0xCF, 0xDB, 0xEB);
    let pale_r = RGBColor(0xEB, 0xE2, 0xCF);
    fill_circle(&root, cx_l, cy, r, &pale_l)?;
    fill_circle(&root, cx_r, cy, r, &pale_r)?;
    stroke_circle(&root, cx_l, cy, r, &NAVY, 2)?;
    stroke_circle(&root, cx_r, cy, r, &RGBColor(0xC7, 0x7F, 0x18), 2)?;

    // Titles above each circle.
    text(
        &root,
        &left.label,
        &centered(font_b(16, &NAVY)),
        cx_l,
        cy - r - 14,
    )?;
    text(
        &root,
        &right.label,
        &centered(font_b(16, &RGBColor(0xC7, 0x7F, 0x18))),
        cx_r,
        cy - r - 14,
    )?;

    // Left-only items (left half, away from intersection).
    let only_l: Vec<&String> = left
        .items
        .iter()
        .filter(|i| !intersection.iter().any(|j| j == *i))
        .collect();
    let only_r: Vec<&String> = right
        .items
        .iter()
        .filter(|i| !intersection.iter().any(|j| j == *i))
        .collect();

    draw_item_column(&root, cx_l - r * 6 / 10, cy, &only_l, 18)?;
    draw_item_column(&root, cx_r + r * 6 / 10, cy, &only_r, 18)?;
    draw_item_column(
        &root,
        (cx_l + cx_r) / 2,
        cy,
        &intersection.iter().collect::<Vec<&String>>(),
        16,
    )?;
    root.present().map_err(|e| anyhow!("present: {e}"))?;
    Ok(())
}

fn draw_item_column(
    a: &crate::Area<'_>,
    cx: i32,
    cy: i32,
    items: &[&String],
    width: usize,
) -> Result<()> {
    let n = items.len() as i32;
    let lh = 18;
    let total = n * lh;
    let mut y = cy - total / 2;
    for it in items.iter().take(10) {
        for ln in wrap(it, width).iter().take(2) {
            text(a, ln, &centered(font(12)), cx, y)?;
            y += lh;
        }
    }
    Ok(())
}

fn render_pane(
    spec: &FigSpec,
    out_path: &Path,
    left: &Side,
    right: &Side,
    intersection: &[String],
) -> Result<()> {
    let (w, h) = (1120i32, 720i32);
    let root = BitMapBackend::new(out_path, (w as u32, h as u32)).into_drawing_area();
    root.fill(&WHITEC).map_err(|e| anyhow!("{e}"))?;
    draw_title(&root, &spec.title, w)?;

    let ml = 30;
    let mt = 80;
    let mb = 30;
    let pane_w = (w - ml * 2 - 30) / 3;
    let pane_h = h - mt - mb;
    let panes = [
        (ml, &left.label, &left.items[..]),
        (ml + pane_w + 15, &"Shared".to_string(), intersection),
        (ml + (pane_w + 15) * 2, &right.label, &right.items[..]),
    ];
    for (x, lbl, items) in panes {
        fill_rect(&root, x, mt, x + pane_w, mt + 40, &NAVY)?;
        text(
            &root,
            lbl,
            &centered(font_b(15, &WHITEC)),
            x + pane_w / 2,
            mt + 20,
        )?;
        stroke_rect(&root, x, mt, x + pane_w, mt + pane_h, &BORDER, 1)?;
        let mut y = mt + 60;
        for it in items.iter().take(20) {
            for (li, ln) in wrap(it, 24).iter().take(2).enumerate() {
                text(
                    &root,
                    &format!("{}{ln}", if li == 0 { "• " } else { "  " }),
                    &font(13).pos(plotters::style::text_anchor::Pos::new(
                        plotters::style::text_anchor::HPos::Left,
                        plotters::style::text_anchor::VPos::Top,
                    )),
                    x + 12,
                    y,
                )?;
                y += 18;
            }
            y += 4;
        }
    }
    root.present().map_err(|e| anyhow!("present: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;

    #[test]
    fn renders_venn() {
        let dir = std::env::temp_dir().join("agentic_fig_overlay_t1");
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("ovl.png");
        let spec_json = r#"{"id":"ov1","type":"comparison-overlay","title":"42001 vs RMF","caption":"c","data":{"mode":"venn","left":{"label":"ISO 42001","items":["AIMS","Context","Leadership"]},"right":{"label":"NIST AI RMF","items":["GOVERN","MAP","MEASURE","MANAGE"]},"intersection":["Risk mgmt","Leadership"]}}"#;
        let spec = parse(spec_json).unwrap();
        render(&spec, &out).unwrap();
        let meta = std::fs::metadata(&out).unwrap();
        assert!(meta.len() > 1000, "venn png too small: {}", meta.len());
    }

    #[test]
    fn renders_side_by_side() {
        let dir = std::env::temp_dir().join("agentic_fig_overlay_t2");
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("ovl2.png");
        let spec_json = r#"{"id":"ov2","type":"comparison-overlay","title":"diff","caption":"c","data":{"mode":"side-by-side","left":{"label":"A","items":["x","y"]},"right":{"label":"B","items":["y","z"]},"intersection":["y"]}}"#;
        let spec = parse(spec_json).unwrap();
        render(&spec, &out).unwrap();
        let meta = std::fs::metadata(&out).unwrap();
        assert!(meta.len() > 1000);
    }
}
