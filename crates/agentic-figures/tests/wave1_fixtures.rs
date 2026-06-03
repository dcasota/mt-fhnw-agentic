//! Integration coverage for Wave-1 figspec fixtures. Each fixture in
//! `tests/fixtures/wave1/` is loaded, parsed, and rendered to a PNG so any
//! drift in the public figspec contract is caught at `cargo test` time.

use std::path::{Path, PathBuf};

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("wave1")
}

/// Ensure the image-embed fixture has its source raster on disk (the JSON
/// fixture references `tests/fixtures/wave1/sample.png`).
fn ensure_sample_png() -> PathBuf {
    let p = fixture_dir().join("sample.png");
    if !p.exists() {
        let mut img = image::RgbImage::new(32, 32);
        for y in 0..32 {
            for x in 0..32 {
                img.put_pixel(
                    x,
                    y,
                    image::Rgb([
                        ((x * 8) & 0xFF) as u8,
                        ((y * 8) & 0xFF) as u8,
                        ((x + y) & 0xFF) as u8 * 4,
                    ]),
                );
            }
        }
        img.save(&p).unwrap();
    }
    p
}

#[test]
fn renders_all_wave1_fixtures() {
    ensure_sample_png();
    let out_dir = std::env::temp_dir().join("agentic_fig_wave1_fixtures");
    std::fs::create_dir_all(&out_dir).unwrap();
    let names = [
        "image_embed",
        "sankey",
        "wheel",
        "tier_matrix",
        "callout_diagram",
        "comparison_overlay",
        "table",
    ];
    // image-embed resolves its source_path relative to the current working
    // directory; rustc sets CWD to the crate root for integration tests, so
    // `tests/fixtures/wave1/sample.png` resolves correctly.
    for name in names {
        let spec_path = fixture_dir().join(format!("{name}.figspec.json"));
        let json = std::fs::read_to_string(&spec_path)
            .unwrap_or_else(|e| panic!("read fixture {name}: {e}"));
        let out = out_dir.join(format!("{name}.png"));
        agentic_figures::render_figspec(&json, &out)
            .unwrap_or_else(|e| panic!("render fixture {name}: {e}"));
        let meta = std::fs::metadata(&out)
            .unwrap_or_else(|e| panic!("stat {name}: {e}"));
        assert!(meta.len() > 100, "{name} png too small: {}", meta.len());
    }
}
