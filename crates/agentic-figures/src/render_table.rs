//! `table` figspec — Word-table equivalent for inline-figure positioning. Some
//! figures in the AI Norms book are really tables (regulatory matrix, control
//! mapping); a Word table cannot live inside a figure caption, so they ship as
//! a rendered raster instead.
//!
//! Expected figspec shape:
//! ```json
//! {
//!   "id": "...", "type": "table", "title": "...", "caption": "...",
//!   "data": {
//!     "header": ["Control", "Source", "Status"],
//!     "rows": [
//!       ["AC-1", "NIST 800-53", "Implemented"],
//!       ["AC-2", "NIST 800-53", "Partial"]
//!     ]
//!   }
//! }
//! ```

use std::path::Path;

use anyhow::{Result, anyhow};
use plotters::prelude::*;
use serde_json::Value;

use crate::{
    BORDER, FigSpec, GREY, HEADBG, WHITEC, centered, draw_title, fig_seed, fill_rect, font,
    font_b, font_c, json_str, stroke_rect, text, wrap,
};

fn parse_row(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| a.iter().map(json_str).collect())
        .unwrap_or_default()
}

pub fn render(spec: &FigSpec, out_path: &Path) -> Result<()> {
    let _seed = fig_seed(&serde_json::to_string(&spec.data).unwrap_or_default());

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
        .map(|a| a.iter().map(parse_row).collect())
        .unwrap_or_default();

    if header.is_empty() && rows.is_empty() {
        return Err(anyhow!("table: missing header/rows"));
    }
    let n_cols = header.len().max(rows.iter().map(Vec::len).max().unwrap_or(0)).max(1);

    // Column-width hints (proportional to max content length per column).
    let col_chars: Vec<usize> = (0..n_cols)
        .map(|c| {
            let head_len = header.get(c).map(|s| s.chars().count()).unwrap_or(0);
            let max_cell = rows
                .iter()
                .map(|r| r.get(c).map(|s| s.chars().count()).unwrap_or(0))
                .max()
                .unwrap_or(0);
            head_len.max(max_cell).max(6).min(36)
        })
        .collect();
    let total_chars: usize = col_chars.iter().sum::<usize>().max(1);

    let table_w_target = 1080i32; // body width budget
    let col_widths: Vec<i32> = col_chars
        .iter()
        .map(|c| ((f64::from(table_w_target) * (*c as f64) / total_chars as f64) as i32).max(60))
        .collect();
    let table_w: i32 = col_widths.iter().sum();

    let head_h = 44i32;
    let row_h = 36i32;
    let ox = 20i32;
    let oy = 60i32;
    let w = table_w + ox * 2;
    let h = oy + head_h + row_h * rows.len() as i32 + 60;

    let root = BitMapBackend::new(out_path, (w as u32, h as u32)).into_drawing_area();
    root.fill(&WHITEC).map_err(|e| anyhow!("{e}"))?;
    draw_title(&root, &spec.title, w)?;

    // header row
    let mut x = ox;
    for (c, ch) in header.iter().enumerate() {
        let cw = col_widths[c];
        fill_rect(&root, x, oy, x + cw, oy + head_h, &HEADBG)?;
        stroke_rect(&root, x, oy, x + cw, oy + head_h, &BORDER, 1)?;
        for (li, ln) in wrap(ch, (col_chars[c]).max(8)).iter().take(2).enumerate() {
            text(
                &root,
                ln,
                &centered(font_b(13, &WHITEC)),
                x + cw / 2,
                oy + 14 + li as i32 * 18,
            )?;
        }
        x += cw;
    }
    // body rows
    for (r, row) in rows.iter().enumerate() {
        let y = oy + head_h + r as i32 * row_h;
        let shade = if r % 2 == 0 {
            RGBColor(0xF4, 0xF6, 0xFA)
        } else {
            WHITEC
        };
        let mut x = ox;
        for c in 0..n_cols {
            let cw = col_widths[c];
            fill_rect(&root, x, y, x + cw, y + row_h, &shade)?;
            stroke_rect(&root, x, y, x + cw, y + row_h, &BORDER, 1)?;
            let val = row.get(c).map(String::as_str).unwrap_or("");
            for (li, ln) in wrap(val, col_chars[c].max(8)).iter().take(2).enumerate() {
                text(
                    &root,
                    ln,
                    &centered(font(12)),
                    x + cw / 2,
                    y + 12 + li as i32 * 16,
                )?;
            }
            x += cw;
        }
    }

    if !spec.caption.is_empty() {
        text(
            &root,
            &spec.caption,
            &centered(font_c(11, &GREY)),
            w / 2,
            h - 20,
        )?;
    }
    root.present().map_err(|e| anyhow!("present: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;

    #[test]
    fn renders_table_basic() {
        let dir = std::env::temp_dir().join("agentic_fig_table_t1");
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("tab.png");
        let spec_json = r#"{"id":"tb1","type":"table","title":"Controls","caption":"c","data":{"header":["Control","Source","Status"],"rows":[["AC-1","NIST 800-53","Implemented"],["AC-2","NIST 800-53","Partial"],["AT-1","ISO 27001","Implemented"]]}}"#;
        let spec = parse(spec_json).unwrap();
        render(&spec, &out).unwrap();
        let meta = std::fs::metadata(&out).unwrap();
        assert!(meta.len() > 1000, "table png too small: {}", meta.len());
    }

    #[test]
    fn rejects_empty() {
        let dir = std::env::temp_dir().join("agentic_fig_table_t2");
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("tab_err.png");
        let spec =
            parse(r#"{"id":"tb2","type":"table","title":"","caption":"","data":{}}"#).unwrap();
        assert!(render(&spec, &out).is_err());
    }
}
