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
//!
//! ## Round V iter-3 (parity drawing-class bucket, 2026-06-03)
//!
//! When a figspec specifies NEITHER `width_in` nor `height_in` AND the
//! source PNG is wider than the default embed width (4 in @ 96 DPI →
//! 384 px), we now shrink to 384 px on the long edge (aspect-preserving).
//! Rationale: the AI-Norms reference book embeds its 55 sourced raster
//! screenshots at page-fit small widths (3-4 in, 2.7 M-3.6 M EMU) — that
//! is what places them in the parity gate's `other` bucket
//! (1 M < cx < 5 M EMU). The previous byte-perfect passthrough preserved
//! each PNG's natural width which — for the typical 500-1000 px sourced
//! raster — lands at the 5.4 M EMU cap applied downstream by
//! `book::image_dims_to_emu`, i.e. squarely in the `figure` bucket.
//! Result: parity iter-2 showed FIGURE=125 vs reference 78 and OTHER=8
//! vs 55 — exactly the 47-image drift this default closes. Sources that
//! are ALREADY at or below the default width keep their byte-perfect
//! passthrough (no transcoding artifacts for icons / thumbnail PNGs).
//! In-house data-rich figure renderers (wheel / sankey / callout /
//! overlay / tier_matrix / table) emit at their own wide dimensions
//! (820-1120 px) and don't go through this renderer at all, so they
//! continue to land in the `figure` bucket. Callers that want larger or
//! full-page embedded images set `width_in` explicitly, preserving the
//! override path.

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};
use image::imageops::FilterType;
use serde_json::Value;

use crate::FigSpec;

/// 96 DPI is the de-facto DOCX/web pixel-per-inch baseline; matches Word.
const DPI: f64 = 96.0;

/// Default embed width when neither `width_in` nor `height_in` is given,
/// in inches. 4 in × 96 DPI = 384 px → ~3.66 M EMU after the downstream
/// `book::image_dims_to_emu` mapping — places the drawing in the parity
/// gate's `other` bucket (1 M < cx < 5 M EMU), matching the reference
/// book's 55 sourced raster embeds. See module docs.
pub const DEFAULT_EMBED_WIDTH_IN: f64 = 4.0;

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

    // Round V iter-3 (parity drawing-class bucket, 2026-06-03):
    // when neither width_in nor height_in is supplied AND the source is
    // wider than the default embed width, shrink to the default
    // (4 in @ 96 DPI = 384 px), aspect-preserving. This lands the
    // resulting PNG in the parity gate's `other` drawing-class bucket
    // (1 M < cx < 5 M EMU), matching the AI-Norms reference book's
    // treatment of sourced raster screenshots. See module docs.
    //
    // Byte-perfect passthrough is preserved for sources that are
    // ALREADY at or below the default width (no quality loss for
    // pre-sized inputs like icons or thumbnail PNGs).
    let img = image::open(&src_path)
        .map_err(|e| anyhow!("image-embed: decode {}: {e}", src_path.display()))?;
    let (cur_w, cur_h) = (img.width(), img.height());

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let default_w_px: u32 = (DEFAULT_EMBED_WIDTH_IN * DPI).round() as u32;

    // Compute final (target_w, target_h) considering:
    //   - explicit width_in / height_in (highest priority)
    //   - DEFAULT_EMBED_WIDTH_IN when neither is supplied AND the source
    //     is wider than the default (shrink-only, never enlarge)
    //   - byte-perfect copy otherwise
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let (target_w, target_h): (u32, u32) = if w_in.is_some() || h_in.is_some() {
        let tw = w_in
            .map(|w| (w * DPI).round() as u32)
            .unwrap_or(cur_w)
            .max(1);
        let th = h_in
            .map(|h| (h * DPI).round() as u32)
            .unwrap_or(cur_h)
            .max(1);
        (tw, th)
    } else if cur_w > default_w_px {
        // Shrink to default width, preserve aspect.
        let tw = default_w_px;
        let th = ((u64::from(cur_h) * u64::from(tw)) / u64::from(cur_w.max(1))).max(1) as u32;
        (tw, th)
    } else {
        // Already at or below default → byte-perfect copy preserves bytes.
        std::fs::copy(&src_path, out_path).map_err(|e| {
            anyhow!(
                "image-embed: copy {} → {}: {e}",
                src_path.display(),
                out_path.display()
            )
        })?;
        return Ok(());
    };

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

    /// Build an N×M PNG fixture for tests that need a specific source
    /// size (the 4×4 helper above is below DEFAULT_EMBED_WIDTH_IN so it
    /// always passthroughs).
    fn make_sized_png(dir: &Path, name: &str, w: u32, h: u32) -> PathBuf {
        let p = dir.join(name);
        let mut img = image::RgbImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                img.put_pixel(x, y, image::Rgb([(x & 0xFF) as u8, (y & 0xFF) as u8, 64]));
            }
        }
        img.save(&p).unwrap();
        p
    }

    #[test]
    fn renders_passthrough_copy_for_small_source() {
        // Source (4×4) is below DEFAULT_EMBED_WIDTH_IN (384 px) → preserves
        // byte-perfect passthrough; no transcoding artifacts.
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

    /// Round V iter-3: a wide sourced raster with NO `width_in` /
    /// `height_in` must shrink to 4-inch default (384 px), preserving
    /// aspect ratio. This is what moves sourced-raster screenshots from
    /// the parity gate's `figure` bucket (≥5 M EMU) into the `other`
    /// bucket (1 M < cx < 5 M EMU), matching the AI-Norms reference.
    #[test]
    fn renders_default_shrinks_wide_source_to_default_width() {
        let dir = std::env::temp_dir().join("agentic_fig_imgembed_t3_default");
        std::fs::create_dir_all(&dir).unwrap();
        // 800×500 source — natural width @96 DPI is 8.33 in, far above
        // the 4-in default. Must shrink to 384×240 (aspect 800:500 = 1.6).
        let src = make_sized_png(&dir, "wide.png", 800, 500);
        let out = dir.join("out_default.png");
        let spec_json = format!(
            r#"{{"id":"ie3","type":"image-embed","title":"","caption":"c","data":{{"source_path":{:?}}}}}"#,
            src.to_string_lossy()
        );
        let spec = parse(&spec_json).unwrap();
        render(&spec, &out).unwrap();
        let decoded = image::open(&out).unwrap();
        assert_eq!(
            decoded.width(),
            (DEFAULT_EMBED_WIDTH_IN * DPI) as u32,
            "wide source must shrink to default embed width (384 px)"
        );
        // Aspect ratio preserved within ±1 px (Lanczos rounding).
        let expected_h = ((500u64 * 384u64) / 800u64) as u32;
        let delta = (decoded.height() as i64 - expected_h as i64).abs();
        assert!(
            delta <= 1,
            "expected height ≈ {expected_h}, got {} (delta {delta})",
            decoded.height()
        );
    }

    /// Explicit `width_in` must override the 4-inch default, so callers
    /// can still embed full-page or oversized images (the override path
    /// is what keeps the existing captioned-figure embeds reachable).
    #[test]
    fn renders_default_respects_explicit_width_override() {
        let dir = std::env::temp_dir().join("agentic_fig_imgembed_t4_override");
        std::fs::create_dir_all(&dir).unwrap();
        let src = make_sized_png(&dir, "wide_override.png", 800, 500);
        let out = dir.join("out_override.png");
        // width_in=6.0 → 576 px (much wider than 384-px default).
        let spec_json = format!(
            r#"{{"id":"ie4","type":"image-embed","title":"","caption":"c","data":{{"source_path":{:?},"width_in":6.0}}}}"#,
            src.to_string_lossy()
        );
        let spec = parse(&spec_json).unwrap();
        render(&spec, &out).unwrap();
        let decoded = image::open(&out).unwrap();
        assert_eq!(
            decoded.width(),
            (6.0 * DPI) as u32,
            "explicit width_in must take precedence over the 4-inch default"
        );
    }
}
