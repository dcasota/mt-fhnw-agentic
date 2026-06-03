//! `image-embed` figspec — embed a pre-existing PNG/JPEG raster as-is.
//!
//! This is the priority Wave-1 renderer: it unblocks the ~109 sourced rasters
//! in the AI Norms reference book that already have correct, vetted imagery on
//! disk. The renderer copies the source bytes through unchanged unless explicit
//! `width_in` / `height_in` are supplied, in which case the image is decoded
//! and resampled to the requested pixel size (96 DPI).
//!
//! NOTE: no third-party fetch is ever performed; only local paths (absolute or
//! relative to the working directory) are accepted, per Wave-1 constraints.
//!
//! Expected figspec shape:
//! ```json
//! {
//!   "id": "...", "type": "image-embed", "title": "...", "caption": "...",
//!   "data": {
//!     "source_path": "path/to/file.png",
//!     "width_in":  6.5,   // optional, inches at 96 DPI
//!     "height_in": 4.0    // optional, inches at 96 DPI
//!   }
//! }
//! ```

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};
use image::imageops::FilterType;
use serde_json::Value;

use crate::FigSpec;

/// 96 DPI is the de-facto DOCX/web pixel-per-inch baseline; matches Word.
const DPI: f64 = 96.0;

/// Resolve the source path: accept absolute paths verbatim, otherwise resolve
/// relative to the current working directory (matches Wave-1 contract).
fn resolve_source(s: &str) -> PathBuf {
    let p = Path::new(s);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir().map_or_else(|_| p.to_path_buf(), |cwd| cwd.join(p))
    }
}

pub fn render(spec: &FigSpec, out_path: &Path) -> Result<()> {
    let src = spec
        .data
        .get("source_path")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("image-embed: missing data.source_path"))?;
    let src_path = resolve_source(src);
    if !src_path.exists() {
        return Err(anyhow!(
            "image-embed: source not found: {}",
            src_path.display()
        ));
    }

    let w_in = spec.data.get("width_in").and_then(Value::as_f64);
    let h_in = spec.data.get("height_in").and_then(Value::as_f64);

    // Fast path: no explicit dimensions → byte-perfect copy (pure pass-through,
    // no transcode, no re-encoding artifacts).
    if w_in.is_none() && h_in.is_none() {
        std::fs::copy(&src_path, out_path)
            .map_err(|e| anyhow!("image-embed: copy {} → {}: {e}", src_path.display(), out_path.display()))?;
        return Ok(());
    }

    // Decode → resample → re-encode as PNG.
    let img = image::open(&src_path)
        .map_err(|e| anyhow!("image-embed: decode {}: {e}", src_path.display()))?;
    let (cur_w, cur_h) = (img.width(), img.height());
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let target_w: u32 = w_in
        .map(|w| (w * DPI).round() as u32)
        .unwrap_or(cur_w)
        .max(1);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let target_h: u32 = h_in
        .map(|h| (h * DPI).round() as u32)
        .unwrap_or(cur_h)
        .max(1);
    let resized = img.resize_exact(target_w, target_h, FilterType::Lanczos3);
    resized
        .save(out_path)
        .map_err(|e| anyhow!("image-embed: encode {}: {e}", out_path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;

    /// Build a tiny on-disk PNG fixture, return its path.
    fn make_fixture_png(dir: &Path) -> PathBuf {
        let p = dir.join("src.png");
        // 4x4 RGB gradient — small but valid PNG.
        let mut img = image::RgbImage::new(4, 4);
        for y in 0..4 {
            for x in 0..4 {
                img.put_pixel(x, y, image::Rgb([(x * 60) as u8, (y * 60) as u8, 128]));
            }
        }
        img.save(&p).unwrap();
        p
    }

    #[test]
    fn renders_passthrough_copy() {
        let dir = std::env::temp_dir().join("agentic_fig_imgembed_t1");
        std::fs::create_dir_all(&dir).unwrap();
        let src = make_fixture_png(&dir);
        let out = dir.join("out.png");
        let spec_json = format!(
            r#"{{"id":"ie1","type":"image-embed","title":"","caption":"c","data":{{"source_path":{:?}}}}}"#,
            src.to_string_lossy()
        );
        let spec = parse(&spec_json).unwrap();
        render(&spec, &out).unwrap();
        // byte-perfect copy → sizes match
        assert_eq!(
            std::fs::metadata(&src).unwrap().len(),
            std::fs::metadata(&out).unwrap().len()
        );
    }

    #[test]
    fn renders_resize_nonempty() {
        let dir = std::env::temp_dir().join("agentic_fig_imgembed_t2");
        std::fs::create_dir_all(&dir).unwrap();
        let src = make_fixture_png(&dir);
        let out = dir.join("out_resized.png");
        let spec_json = format!(
            r#"{{"id":"ie2","type":"image-embed","title":"","caption":"c","data":{{"source_path":{:?},"width_in":1.0,"height_in":1.0}}}}"#,
            src.to_string_lossy()
        );
        let spec = parse(&spec_json).unwrap();
        render(&spec, &out).unwrap();
        let decoded = image::open(&out).unwrap();
        // 1.0in × 96 DPI = 96 px
        assert_eq!(decoded.width(), 96);
        assert_eq!(decoded.height(), 96);
        assert!(std::fs::metadata(&out).unwrap().len() > 50);
    }
}
