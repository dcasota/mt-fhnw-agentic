//! `callout-diagram` figspec — annotation-pin lines from external labels to
//! component pins (Google SAIF risk-map style).
//!
//! Coordinate space: x ∈ [0,1], y ∈ [0,1] over the plottable area (so specs
//! are resolution-independent). An optional base raster underlay is supported
//! and is composited beneath the annotations by re-encoding through plotters.
//!
//! Expected figspec shape:
//! ```json
//! {
//!   "id": "...", "type": "callout-diagram", "title": "...", "caption": "...",
//!   "data": {
//!     "base_image": "path/to/diagram.png",   // optional
//!     "components": [
//!       {"x":0.30,"y":0.42,"label":"Model","color":"#1F497D"},
//!       {"x":0.71,"y":0.20,"label":"Prompt Injection","color":"#C2382A"}
//!     ]
//!   }
//! }
//! ```

use std::path::Path;

use anyhow::{Result, anyhow};
use plotters::prelude::*;
use serde_json::Value;

use crate::{
    BORDER, FigSpec, GREY, INK, NAVY, WHITEC, centered, draw_title, fig_seed, fill_circle,
    fill_rect, font_b, font_c, hex_color, line, stroke_circle, stroke_rect, text, wrap,
};

struct Comp {
    x: f64,
    y: f64,
    label: String,
    color: RGBColor,
}

fn parse_components(v: &Value) -> Vec<Comp> {
    v.get("components")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|c| Comp {
                    x: c.get("x").and_then(Value::as_f64).unwrap_or(0.5).clamp(0.0, 1.0),
                    y: c.get("y").and_then(Value::as_f64).unwrap_or(0.5).clamp(0.0, 1.0),
                    label: c
                        .get("label")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    color: c
                        .get("color")
                        .and_then(Value::as_str)
                        .map_or(NAVY, hex_color),
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn render(spec: &FigSpec, out_path: &Path) -> Result<()> {
    let _seed = fig_seed(&serde_json::to_string(&spec.data).unwrap_or_default());
    let comps = parse_components(&spec.data);
    if comps.is_empty() {
        return Err(anyhow!("callout-diagram: missing data.components"));
    }
    let base_image = spec
        .data
        .get("base_image")
        .and_then(Value::as_str)
        .map(str::to_string);

    let (w, h) = (1100i32, 700i32);
    let (ml, mr, mt, mb) = (180i32, 180i32, 80i32, 60i32);
    let pw = w - ml - mr;
    let ph = h - mt - mb;

    let root = BitMapBackend::new(out_path, (w as u32, h as u32)).into_drawing_area();
    root.fill(&WHITEC).map_err(|e| anyhow!("{e}"))?;
    draw_title(&root, &spec.title, w)?;

    // Render the base underlay if provided & decodable. Failure is non-fatal:
    // we degrade to a blank canvas so downstream callers never lose the
    // annotation layer entirely (non-repudiation requirement).
    if let Some(bi) = &base_image {
        if let Ok(img) = image::open(bi) {
            let resized = img.resize_exact(
                pw as u32,
                ph as u32,
                image::imageops::FilterType::Triangle,
            );
            let rgb = resized.to_rgb8();
            for (px, py, pix) in rgb.enumerate_pixels() {
                let c = RGBColor(pix.0[0], pix.0[1], pix.0[2]);
                let _ = root.draw_pixel((ml + px as i32, mt + py as i32), &c);
            }
        }
    } else {
        // Sketch a faint dashed frame so the annotation context is obvious.
        stroke_rect(&root, ml, mt, ml + pw, mt + ph, &BORDER, 1)?;
        text(
            &root,
            "(base diagram)",
            &centered(font_c(13, &GREY)),
            ml + pw / 2,
            mt + ph / 2,
        )?;
    }

    // Component pins + connector lines + side labels (split left vs right by x).
    let mut left: Vec<&Comp> = comps.iter().filter(|c| c.x < 0.5).collect();
    let mut right: Vec<&Comp> = comps.iter().filter(|c| c.x >= 0.5).collect();
    left.sort_by(|a, b| a.y.partial_cmp(&b.y).unwrap_or(std::cmp::Ordering::Equal));
    right.sort_by(|a, b| a.y.partial_cmp(&b.y).unwrap_or(std::cmp::Ordering::Equal));

    let n_l = left.len().max(1) as i32;
    let n_r = right.len().max(1) as i32;
    let slot_l = ph / n_l;
    let slot_r = ph / n_r;

    let plot_xy = |c: &Comp| -> (i32, i32) {
        (
            ml + (c.x * f64::from(pw)) as i32,
            mt + (c.y * f64::from(ph)) as i32,
        )
    };

    for (i, c) in left.iter().enumerate() {
        let (cx, cy) = plot_xy(c);
        let lx = ml - 12;
        let ly = mt + slot_l * i as i32 + slot_l / 2;
        line(&root, vec![(cx, cy), (lx, ly)], &c.color, 2)?;
        // pin
        fill_circle(&root, cx, cy, 6, &c.color)?;
        stroke_circle(&root, cx, cy, 6, &INK, 1)?;
        // label box (right-aligned)
        let lines: Vec<String> = wrap(&c.label, 18);
        let bw = 160;
        let bh = 20 + 16 * (lines.len() as i32);
        let bx1 = lx;
        let bx0 = bx1 - bw;
        let by0 = ly - bh / 2;
        fill_rect(&root, bx0, by0, bx1, by0 + bh, &WHITEC)?;
        stroke_rect(&root, bx0, by0, bx1, by0 + bh, &c.color, 1)?;
        for (li, ln) in lines.iter().enumerate() {
            text(
                &root,
                ln,
                &centered(font_b(12, &INK)),
                bx0 + bw / 2,
                by0 + 14 + li as i32 * 16,
            )?;
        }
    }
    for (i, c) in right.iter().enumerate() {
        let (cx, cy) = plot_xy(c);
        let lx = ml + pw + 12;
        let ly = mt + slot_r * i as i32 + slot_r / 2;
        line(&root, vec![(cx, cy), (lx, ly)], &c.color, 2)?;
        fill_circle(&root, cx, cy, 6, &c.color)?;
        stroke_circle(&root, cx, cy, 6, &INK, 1)?;
        let lines: Vec<String> = wrap(&c.label, 18);
        let bw = 160;
        let bh = 20 + 16 * (lines.len() as i32);
        let bx0 = lx;
        let bx1 = bx0 + bw;
        let by0 = ly - bh / 2;
        fill_rect(&root, bx0, by0, bx1, by0 + bh, &WHITEC)?;
        stroke_rect(&root, bx0, by0, bx1, by0 + bh, &c.color, 1)?;
        for (li, ln) in lines.iter().enumerate() {
            text(
                &root,
                ln,
                &centered(font_b(12, &INK)),
                bx0 + bw / 2,
                by0 + 14 + li as i32 * 16,
            )?;
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
    fn renders_callout_no_base() {
        let dir = std::env::temp_dir().join("agentic_fig_callout_t1");
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("co.png");
        let spec_json = r##"{"id":"co1","type":"callout-diagram","title":"SAIF","caption":"c","data":{"components":[{"x":0.30,"y":0.42,"label":"Model","color":"#1F497D"},{"x":0.71,"y":0.20,"label":"Prompt Injection","color":"#C2382A"},{"x":0.20,"y":0.70,"label":"Data Source","color":"#009E73"}]}}"##;
        let spec = parse(spec_json).unwrap();
        render(&spec, &out).unwrap();
        let meta = std::fs::metadata(&out).unwrap();
        assert!(meta.len() > 1000, "callout png too small: {}", meta.len());
    }

    #[test]
    fn rejects_empty_components() {
        let dir = std::env::temp_dir().join("agentic_fig_callout_t2");
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("co_err.png");
        let spec = parse(
            r#"{"id":"co2","type":"callout-diagram","title":"","caption":"","data":{}}"#,
        )
        .unwrap();
        assert!(render(&spec, &out).is_err());
    }
}
