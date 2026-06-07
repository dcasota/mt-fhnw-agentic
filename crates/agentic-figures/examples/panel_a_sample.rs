//! Standalone Panel A render used for visual diff against the kit's
//! reference PNG during the regulation_timeline port. Run with:
//!
//! ```bash
//! cargo run --release -p agentic-figures --example panel_a_sample
//! ```
//!
//! Writes `target/panel_a_<lang>.png` for each of the six supported
//! languages.

use agentic_figures::regulation_timeline_panel_a::render_panel_a_only;
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let out_dir = PathBuf::from("target/regulation_timeline_panel_a_samples");
    std::fs::create_dir_all(&out_dir)?;
    for lang in ["en", "de", "fr", "it", "rm", "hi"] {
        let path = out_dir.join(format!("panel_a_{lang}.png"));
        render_panel_a_only(&path, lang)?;
        println!("wrote {}", path.display());
    }
    Ok(())
}
