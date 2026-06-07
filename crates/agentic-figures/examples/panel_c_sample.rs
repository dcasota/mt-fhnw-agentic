//! Standalone Panel C render used for visual diff against the kit's
//! reference PNG during the regulation_timeline port.

use agentic_figures::regulation_timeline_panel_c::render_panel_c_only;
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let out_dir = PathBuf::from("target/regulation_timeline_panel_c_samples");
    std::fs::create_dir_all(&out_dir)?;
    for lang in ["en", "de", "fr", "it", "rm", "hi"] {
        let path = out_dir.join(format!("panel_c_{lang}.png"));
        render_panel_c_only(&path, lang)?;
        println!("wrote {}", path.display());
    }
    Ok(())
}
