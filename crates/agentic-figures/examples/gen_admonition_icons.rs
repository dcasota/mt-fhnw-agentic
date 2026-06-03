//! Round V (visual parity, 2026-06-03): emit the three admonition icons
//! (tip / note / warning) at the bumped 330×330 resolution into a target
//! directory. Used to refresh `specs/figures/raster/ai_norms/icon_*.png`.
//!
//! Usage: `cargo run -p agentic-figures --example gen_admonition_icons -- <out_dir>`

use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let out_dir: PathBuf = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("usage: gen_admonition_icons <out_dir>"))?;
    std::fs::create_dir_all(&out_dir)?;
    for kind in ["tip", "note", "warning"] {
        let spec = format!(
            "{{\"id\":\"icon_{kind}\",\"type\":\"icon\",\"title\":\"\",\"caption\":\"\",\"data\":{{\"variant\":\"{kind}\"}}}}"
        );
        let out_path = out_dir.join(format!("icon_{kind}.png"));
        agentic_figures::render_figspec(&spec, &out_path)?;
        let meta = std::fs::metadata(&out_path)?;
        println!("wrote {} ({} bytes)", out_path.display(), meta.len());
    }
    Ok(())
}
