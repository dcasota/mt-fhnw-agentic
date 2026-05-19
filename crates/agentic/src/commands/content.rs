use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::json;

use agentic_core::content::{commit, blob};

use crate::cli::ContentAction;

pub fn run(db_path: &Path, action: ContentAction, json_out: bool) -> Result<()> {
    let conn = agentic_core::db::open(db_path)?;
    match action {
        ContentAction::Put { path, lang } => {
            let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
            let mime = mime_from_extension(&path);
            let sha = blob::put_blob(&conn, &bytes, &mime, lang.as_deref())?;
            if json_out {
                println!("{}", json!({ "sha256": sha, "size": bytes.len() }));
            } else {
                println!("{sha}  {} bytes  {}", bytes.len(), path.display());
            }
        }
        ContentAction::Get { sha, to } => {
            let b = blob::get_blob(&conn, &sha)?;
            if let Some(target) = to {
                std::fs::write(&target, &b.content)?;
                if !json_out {
                    println!("Wrote {} bytes to {}", b.size_bytes, target.display());
                }
            } else {
                std::io::stdout().write_all(&b.content)?;
            }
        }
        ContentAction::Log { limit } => {
            let commits = commit::log(&conn, limit)?;
            if json_out {
                println!("{}", serde_json::to_string_pretty(&commits)?);
            } else if commits.is_empty() {
                println!("(no commits yet)");
            } else {
                for c in commits {
                    println!("{}  {}  {}  {}", &c.sha256[..12], c.timestamp, c.author, c.message);
                }
            }
        }
    }
    Ok(())
}

fn mime_from_extension(path: &Path) -> String {
    match path.extension().and_then(|e| e.to_str()) {
        Some("md") => "text/markdown".into(),
        Some("json") => "application/json".into(),
        Some("yaml" | "yml") => "application/yaml".into(),
        Some("toml") => "application/toml".into(),
        Some("txt") => "text/plain".into(),
        Some("pdf") => "application/pdf".into(),
        Some("docx") => "application/vnd.openxmlformats-officedocument.wordprocessingml.document".into(),
        Some("pptx") => "application/vnd.openxmlformats-officedocument.presentationml.presentation".into(),
        Some("png") => "image/png".into(),
        Some("jpg" | "jpeg") => "image/jpeg".into(),
        _ => "application/octet-stream".into(),
    }
}
