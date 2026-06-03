//! `tier-matrix` figspec — column-grouped sub-cells extending the simple matrix
//! renderer. Used for MITRE-ATT&CK-style tactic columns where each column has a
//! ragged number of technique cells, optionally tinted by severity.
//!
//! Expected figspec shape:
//! ```json
//! {
//!   "id": "...", "type": "tier-matrix", "title": "...", "caption": "...",
//!   "data": {
//!     "tiers": [
//!       {"label": "Reconnaissance", "items": [
//!         {"label":"Active Scanning","severity":2},
//!         {"label":"Gather Victim Identity","severity":1}
//!       ]},
//!       {"label": "Initial Access", "items": [...]},
//!       ...
//!     ]
//!   }
//! }
//! ```
//! `severity` is an integer 0..=4 → cell tint ramp (low → critical).

use std::path::Path;

use anyhow::{Result, anyhow};
use plotters::prelude::*;
use serde_json::Value;

use crate::{
    BORDER, FigSpec, HEADBG, INK, WHITEC, centered, draw_title, fig_seed, fill_rect, font, font_b,
    font_c, hex_color, stroke_rect, text, wrap,
};

struct Item {
    label: String,
    severity: i64,
}

struct Tier {
    label: String,
    items: Vec<Item>,
}

fn parse_tiers(v: &Value) -> Vec<Tier> {
    v.get("tiers")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|t| {
                    let label = t
                        .get("label")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let items = t
                        .get("items")
                        .and_then(Value::as_array)
                        .map(|its| {
                            its.iter()
                                .map(|it| Item {
                                    label: it
                                        .get("label")
                                        .and_then(Value::as_str)
                                        .unwrap_or("")
                                        .to_string(),
                                    severity: it
                                        .get("severity")
                                        .and_then(Value::as_i64)
                                        .unwrap_or(0),
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    Tier { label, items }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 5-stop severity ramp from cool (0) → critical (4).
fn severity_color(s: i64) -> RGBColor {
    match s.clamp(0, 4) {
        0 => hex_color("#E8EDF4"),
        1 => hex_color("#F5E6B3"),
        2 => hex_color("#F3C36E"),
        3 => hex_color("#E68A4A"),
        _ => hex_color("#C2382A"),
    }
}

pub fn render(spec: &FigSpec, out_path: &Path) -> Result<()> {
    let _seed = fig_seed(&serde_json::to_string(&spec.data).unwrap_or_default());

    let tiers = parse_tiers(&spec.data);
    if tiers.is_empty() {
        return Err(anyhow!("tier-matrix: missing data.tiers"));
    }
    let n_cols = tiers.len() as i32;
    let max_rows = tiers.iter().map(|t| t.items.len()).max().unwrap_or(1) as i32;

    let cell_w = 170i32;
    let cell_h = 56i32;
    let head_h = 56i32;
    let ox = 24i32;
    let oy = 60i32;
    let w = ox * 2 + n_cols * cell_w;
    let h = oy + head_h + max_rows * cell_h + 40;

    let root = BitMapBackend::new(out_path, (w as u32, h as u32)).into_drawing_area();
    root.fill(&WHITEC).map_err(|e| anyhow!("{e}"))?;
    draw_title(&root, &spec.title, w)?;

    // headers
    for (c, t) in tiers.iter().enumerate() {
        let x0 = ox + c as i32 * cell_w;
        fill_rect(&root, x0, oy, x0 + cell_w, oy + head_h, &HEADBG)?;
        for (li, ln) in wrap(&t.label, 14).iter().take(2).enumerate() {
            text(
                &root,
                ln,
                &centered(font_b(15, &WHITEC)),
                x0 + cell_w / 2,
                oy + 20 + li as i32 * 19,
            )?;
        }
    }
    // cells
    for (c, t) in tiers.iter().enumerate() {
        let x0 = ox + c as i32 * cell_w;
        for (r, it) in t.items.iter().enumerate() {
            let y0 = oy + head_h + r as i32 * cell_h;
            let bg = severity_color(it.severity);
            fill_rect(&root, x0, y0, x0 + cell_w, y0 + cell_h, &bg)?;
            stroke_rect(&root, x0, y0, x0 + cell_w, y0 + cell_h, &BORDER, 1)?;
            // text colour for legibility
            let style = if it.severity >= 3 {
                centered(font_c(13, &WHITEC))
            } else {
                centered(font_c(13, &INK))
            };
            for (li, ln) in wrap(&it.label, 18).iter().take(3).enumerate() {
                text(
                    &root,
                    ln,
                    &style,
                    x0 + cell_w / 2,
                    y0 + cell_h / 2 - 12 + li as i32 * 17,
                )?;
            }
        }
    }

    // severity legend along the bottom
    let lx0 = ox;
    let ly = h - 30;
    let chip_w = 70i32;
    let labels = [
        ("low", 0_i64),
        ("med-low", 1),
        ("medium", 2),
        ("high", 3),
        ("critical", 4),
    ];
    for (i, (lbl, sev)) in labels.iter().enumerate() {
        let x = lx0 + i as i32 * (chip_w + 8);
        fill_rect(&root, x, ly, x + chip_w, ly + 18, &severity_color(*sev))?;
        stroke_rect(&root, x, ly, x + chip_w, ly + 18, &BORDER, 1)?;
        text(&root, lbl, &centered(font(11)), x + chip_w / 2, ly + 9)?;
    }
    root.present().map_err(|e| anyhow!("present: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;

    #[test]
    fn renders_mitre_3x() {
        let dir = std::env::temp_dir().join("agentic_fig_tiermat_t1");
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("tm.png");
        let spec_json = r#"{"id":"tm1","type":"tier-matrix","title":"MITRE ATT&CK","caption":"c","data":{"tiers":[{"label":"Reconnaissance","items":[{"label":"Active Scanning","severity":2},{"label":"Phishing for Info","severity":3}]},{"label":"Initial Access","items":[{"label":"Phishing","severity":4},{"label":"Drive-by","severity":2},{"label":"Exploit Public-Facing App","severity":3}]},{"label":"Execution","items":[{"label":"Command/Script","severity":3}]}]}}"#;
        let spec = parse(spec_json).unwrap();
        render(&spec, &out).unwrap();
        let meta = std::fs::metadata(&out).unwrap();
        assert!(
            meta.len() > 1000,
            "tier-matrix png too small: {}",
            meta.len()
        );
    }

    #[test]
    fn rejects_missing_tiers() {
        let dir = std::env::temp_dir().join("agentic_fig_tiermat_t2");
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("tm_err.png");
        let spec = parse(r#"{"id":"tm2","type":"tier-matrix","title":"","caption":"","data":{}}"#)
            .unwrap();
        assert!(render(&spec, &out).is_err());
    }
}
