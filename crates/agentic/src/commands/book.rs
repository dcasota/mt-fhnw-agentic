//! `agentic book` — render professional DOCX books from content-store markdown.
//! The Rust book engine: figures via `agentic-figures`, layout via
//! `agentic-export::book` (docx-rs). Replaces the Python bookkit/build_book.

use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

use agentic_core::worktree;
use agentic_export::book::{BookMeta, render_book};

#[derive(Debug, Deserialize)]
struct Manifest {
    books: Vec<BookSpec>,
}

#[derive(Debug, Deserialize)]
struct BookSpec {
    key: String,
    title: String,
    #[serde(default)]
    subtitle: String,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    context: Option<String>,
    chapters: Vec<String>,
}

fn sanitize(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect()
}

pub fn run(
    db_path: &Path,
    project: &str,
    manifest_path: &Path,
    out: &Path,
    only: Option<&str>,
    json_out: bool,
) -> Result<()> {
    let conn = agentic_core::db::open(db_path)?;
    let text = std::fs::read_to_string(manifest_path)
        .with_context(|| format!("reading manifest {}", manifest_path.display()))?;
    let manifest: Manifest = serde_json::from_str(&text).context("parsing manifest JSON")?;
    std::fs::create_dir_all(out)?;
    let fig_base = out.join("_work");
    std::fs::create_dir_all(&fig_base)?;

    let mut built = 0usize;
    for spec in &manifest.books {
        if let Some(k) = only {
            if spec.key != k {
                continue;
            }
        }
        let mut chapters: Vec<(String, String)> = Vec::new();
        let mut figs = 0usize;
        for ch in &spec.chapters {
            let blob = match worktree::read_at(&conn, project, ch) {
                Ok(b) => b,
                Err(_) => {
                    eprintln!("    ! missing chapter {ch}");
                    continue;
                }
            };
            let md = String::from_utf8_lossy(&blob.content).to_string();
            let subdir = sanitize(ch.rsplit('/').next().unwrap_or(ch));
            let (resolved, n) = agentic_figures::resolve_markdown(&md, &fig_base, &subdir)
                .unwrap_or((md.clone(), 0));
            figs += n;
            chapters.push((ch.clone(), resolved));
        }
        let meta = BookMeta {
            title: spec.title.clone(),
            subtitle: spec.subtitle.clone(),
            author: spec.author.clone().unwrap_or_else(|| "Daniel Casota".into()),
            context: spec
                .context
                .clone()
                .unwrap_or_else(|| "MAS Cybersecurity, IWI, FHNW — May 2026".into()),
        };
        let bytes = render_book(&meta, &chapters, &fig_base)?;
        let path = out.join(format!("{}.docx", spec.key));
        std::fs::write(&path, &bytes).with_context(|| format!("writing {}", path.display()))?;
        built += 1;
        if !json_out {
            println!("  + {}.docx  ({} figures, {} chapters)", spec.key, figs, spec.chapters.len());
        }
    }
    if json_out {
        println!("{}", serde_json::json!({ "built": built, "out": out.display().to_string() }));
    } else {
        println!("Built {built} book(s) into {}", out.display());
    }
    Ok(())
}
