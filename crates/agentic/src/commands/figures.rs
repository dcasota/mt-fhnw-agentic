//! `agentic figures` — bulk figure-asset utilities (Wave-5 extraction flow).
//!
//! The Rust figspec renderer in `agentic-figures` only knows how to take a
//! figspec JSON and emit a PNG. It cannot, by design, reach into someone
//! else's `.docx`. This module covers the asset-pipeline steps that surround
//! the renderer — currently only `extract-from-docx`, which unzips a docx
//! and copies every `word/media/<file>` to a flat output directory so the
//! AI-Norms parity flow can author one `image-embed` figspec per raster.
//!
//! No transcoding, no filtering, no rewriting. The optional `--min-bytes`
//! threshold drops the small admonition glyphs the reference book embeds
//! hundreds of times.

use std::fs;
use std::io::{Read, Write};
use std::path::Path;

use anyhow::{Context, Result, anyhow};
use serde_json::json;
use zip::ZipArchive;

use crate::cli::FiguresAction;

pub fn run(action: FiguresAction, json_out: bool) -> Result<()> {
    match action {
        FiguresAction::ExtractFromDocx {
            docx,
            out,
            min_bytes,
        } => extract_from_docx(&docx, &out, min_bytes, json_out),
        FiguresAction::RegulationTimeline { out, target, mode } => {
            regulation_timeline(&out, &target, &mode, json_out)
        }
    }
}

fn regulation_timeline(out: &Path, target: &str, mode: &str, json_out: bool) -> Result<()> {
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create output parent {}", parent.display()))?;
    }
    match mode {
        "abc" => agentic_figures::regulation_timeline_layout::render_abc(out, target)?,
        "ab" => agentic_figures::regulation_timeline_layout::render_ab(out, target)?,
        "c" => agentic_figures::regulation_timeline_layout::render_c_only(out, target)?,
        other => {
            return Err(anyhow!(
                "unknown --mode '{other}'; valid: abc | ab | c"
            ));
        }
    }
    let size = fs::metadata(out).map(|m| m.len()).unwrap_or(0);
    if json_out {
        let v = json!({
            "out": out.display().to_string(),
            "target": target,
            "mode": mode,
            "bytes": size,
        });
        println!("{}", serde_json::to_string_pretty(&v)?);
    } else {
        println!(
            "wrote regulation-timeline (mode={mode}, target={target}) -> {} ({size} bytes)",
            out.display()
        );
    }
    Ok(())
}

fn extract_from_docx(docx: &Path, out: &Path, min_bytes: u64, json_out: bool) -> Result<()> {
    if !docx.exists() {
        return Err(anyhow!("source docx not found: {}", docx.display()));
    }
    fs::create_dir_all(out).with_context(|| format!("create output dir {}", out.display()))?;

    let file = fs::File::open(docx).with_context(|| format!("open docx {}", docx.display()))?;
    let mut zip =
        ZipArchive::new(file).with_context(|| format!("read docx as zip {}", docx.display()))?;

    let mut extracted: u64 = 0;
    let mut skipped_small: u64 = 0;
    let mut total_bytes: u64 = 0;
    let mut names: Vec<String> = Vec::new();

    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        if !entry.is_file() {
            continue;
        }
        let name = entry.name().to_string();
        // Only the embedded rasters under word/media/.
        let Some(rel) = name.strip_prefix("word/media/") else {
            continue;
        };
        if rel.is_empty() {
            continue;
        }
        let size = entry.size();
        if size < min_bytes {
            skipped_small += 1;
            continue;
        }
        let mut bytes = Vec::with_capacity(size as usize);
        entry.read_to_end(&mut bytes)?;
        let dst = out.join(rel);
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut f = fs::File::create(&dst).with_context(|| format!("create {}", dst.display()))?;
        f.write_all(&bytes)?;
        extracted += 1;
        total_bytes += bytes.len() as u64;
        names.push(rel.to_string());
    }

    if json_out {
        let v = json!({
            "extracted": extracted,
            "skipped_small": skipped_small,
            "total_bytes": total_bytes,
            "out_dir": out.display().to_string(),
            "names": names,
        });
        println!("{}", serde_json::to_string_pretty(&v)?);
    } else {
        println!(
            "extracted {extracted} rasters ({total_bytes} bytes) to {} (skipped {skipped_small} < {min_bytes} bytes)",
            out.display()
        );
    }
    Ok(())
}
