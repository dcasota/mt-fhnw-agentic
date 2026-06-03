//! `wheel` figspec — polar hub/spokes diagram (e.g. NIST CSF Core wheel).
//!
//! Expected figspec shape:
//! ```json
//! {
//!   "id": "...", "type": "wheel", "title": "...", "caption": "...",
//!   "data": {
//!     "center_label": "CSF Core",
//!     "spokes": [{"label":"GOVERN","color":"#1F497D"},
//!                {"label":"IDENTIFY","color":"#0072B2"}, ...],
//!     "inner_ring": "core",       // optional small text inside hub
//!     "outer_ring": "functions"   // optional outer-ring caption
//!   }
//! }
//! ```

use std::f64::consts::PI;
use std::path::Path;

use anyhow::{Result, anyhow};
use plotters::prelude::*;
use serde_json::Value;

use crate::{
    FigSpec, GREY, INK, NAVY, WHITEC, WONG, centered, draw_title, fig_seed, fill_circle, fill_poly,
    font_b, font_c, hex_color, stroke_circle, text,
};

struct Spoke {
    label: String,
    color: RGBColor,
}

fn parse_spokes(v: &Value) -> Vec<Spoke> {
    v.get("spokes")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .enumerate()
                .map(|(i, s)| {
                    let label = s
                        .get("label")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let color = s
                        .get("color")
                        .and_then(Value::as_str)
                        .map_or_else(|| WONG[i % WONG.len()], hex_color);
                    Spoke { label, color }
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn render(spec: &FigSpec, out_path: &Path) -> Result<()> {
    let _seed = fig_seed(&serde_json::to_string(&spec.data).unwrap_or_default());

    let spokes = parse_spokes(&spec.data);
    if spokes.is_empty() {
        return Err(anyhow!("wheel: missing data.spokes"));
    }
    let center_label = spec
        .data
        .get("center_label")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let inner_ring = spec
        .data
        .get("inner_ring")
        .and_then(Value::as_str)
        .unwrap_or("");
    let outer_ring = spec
        .data
        .get("outer_ring")
        .and_then(Value::as_str)
        .unwrap_or("");

    let (w, h) = (820i32, 820i32);
    let cx = w / 2;
    let cy = h / 2 + 10;
    let r_outer = 320i32;
    let r_inner = 140i32;
    let r_hub = 100i32;

    let root = BitMapBackend::new(out_path, (w as u32, h as u32)).into_drawing_area();
    root.fill(&WHITEC).map_err(|e| anyhow!("{e}"))?;
    draw_title(&root, &spec.title, w)?;

    let n = spokes.len() as f64;
    let start_angle = -PI / 2.0; // 12 o'clock

    // Draw each wedge.
    for (i, sp) in spokes.iter().enumerate() {
        let a0 = start_angle + (i as f64) * 2.0 * PI / n;
        let a1 = start_angle + ((i + 1) as f64) * 2.0 * PI / n;
        let pts = sample_wedge(cx, cy, r_inner, r_outer, a0, a1, 18);
        fill_poly(&root, pts.clone(), &lighten(&sp.color))?;
        // outline
        let outline_style = ShapeStyle::from(&sp.color).stroke_width(2);
        root.draw(&PathElement::new(pts, outline_style))
            .map_err(|e| anyhow!("wedge outline: {e}"))?;
        // wedge label at mid-radius
        let am = (a0 + a1) / 2.0;
        let rm = (r_inner + r_outer) / 2;
        #[allow(clippy::cast_possible_truncation)]
        let lx = cx + (f64::from(rm) * am.cos()) as i32;
        #[allow(clippy::cast_possible_truncation)]
        let ly = cy + (f64::from(rm) * am.sin()) as i32;
        text(&root, &sp.label, &centered(font_b(15, &INK)), lx, ly)?;
    }

    // outer caption (curved-style: just centered above top wedge)
    if !outer_ring.is_empty() {
        text(
            &root,
            outer_ring,
            &centered(font_c(13, &GREY)),
            cx,
            cy - r_outer - 18,
        )?;
    }

    // Hub.
    fill_circle(&root, cx, cy, r_hub, &NAVY)?;
    stroke_circle(&root, cx, cy, r_hub, &INK, 2)?;
    text(
        &root,
        &center_label,
        &centered(font_b(18, &WHITEC)),
        cx,
        cy - 6,
    )?;
    if !inner_ring.is_empty() {
        text(
            &root,
            inner_ring,
            &centered(font_c(11, &WHITEC)),
            cx,
            cy + 18,
        )?;
    }
    root.present().map_err(|e| anyhow!("present: {e}"))?;
    Ok(())
}

fn lighten(c: &RGBColor) -> RGBColor {
    RGBColor(
        ((u16::from(c.0) + 255 * 3) / 4) as u8,
        ((u16::from(c.1) + 255 * 3) / 4) as u8,
        ((u16::from(c.2) + 255 * 3) / 4) as u8,
    )
}

/// Sample a wedge polygon (annulus sector) along its two radial edges and two
/// arcs. `n` samples per arc.
fn sample_wedge(
    cx: i32,
    cy: i32,
    r_inner: i32,
    r_outer: i32,
    a0: f64,
    a1: f64,
    n: i32,
) -> Vec<(i32, i32)> {
    let mut pts = Vec::with_capacity((2 * n + 2) as usize);
    // outer arc from a0 to a1
    for k in 0..=n {
        let t = a0 + (a1 - a0) * f64::from(k) / f64::from(n);
        #[allow(clippy::cast_possible_truncation)]
        {
            pts.push((
                cx + (f64::from(r_outer) * t.cos()) as i32,
                cy + (f64::from(r_outer) * t.sin()) as i32,
            ));
        }
    }
    // inner arc from a1 back to a0
    for k in 0..=n {
        let t = a1 - (a1 - a0) * f64::from(k) / f64::from(n);
        #[allow(clippy::cast_possible_truncation)]
        {
            pts.push((
                cx + (f64::from(r_inner) * t.cos()) as i32,
                cy + (f64::from(r_inner) * t.sin()) as i32,
            ));
        }
    }
    pts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;

    #[test]
    fn renders_csf_wheel() {
        let dir = std::env::temp_dir().join("agentic_fig_wheel_t1");
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("wheel.png");
        let spec_json = r##"{"id":"w1","type":"wheel","title":"NIST CSF","caption":"c","data":{"center_label":"CSF Core","spokes":[{"label":"GOVERN","color":"#1F497D"},{"label":"IDENTIFY","color":"#0072B2"},{"label":"PROTECT","color":"#009E73"},{"label":"DETECT","color":"#E69F00"},{"label":"RESPOND","color":"#D55E00"},{"label":"RECOVER","color":"#CC79A7"}]}}"##;
        let spec = parse(spec_json).unwrap();
        render(&spec, &out).unwrap();
        let meta = std::fs::metadata(&out).unwrap();
        assert!(meta.len() > 1000, "wheel png too small: {}", meta.len());
    }

    #[test]
    fn rejects_missing_spokes() {
        let dir = std::env::temp_dir().join("agentic_fig_wheel_t2");
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("wheel_err.png");
        let spec = parse(
            r#"{"id":"w2","type":"wheel","title":"","caption":"","data":{"center_label":"x"}}"#,
        )
        .unwrap();
        assert!(render(&spec, &out).is_err());
    }
}
