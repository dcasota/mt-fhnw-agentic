//! Standalone ABC composite render — the layout the thesis §2.3 uses.

use agentic_figures::regulation_timeline_layout::render_abc;
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let out_dir = PathBuf::from("target/regulation_timeline_abc_samples");
    std::fs::create_dir_all(&out_dir)?;
    for lang in ["en", "de", "fr", "it", "rm", "hi"] {
        let path = out_dir.join(format!("regulation_timeline_v3_ABC_{lang}.png"));
        render_abc(&path, lang)?;
        let size = std::fs::metadata(&path).unwrap().len();
        println!("wrote {} ({} bytes)", path.display(), size);
    }
    Ok(())
}
