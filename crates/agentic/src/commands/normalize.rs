//! `agentic normalize` — deterministic deliverable normalisation over the
//! content store (Rust port of normalize_deliverable.py). Reads markdown blobs
//! under a prefix, normalises them, and writes the changed ones back in one
//! commit.

use std::path::Path;

use anyhow::Result;
use serde_json::json;

use agentic_core::worktree;
use agentic_checks::normalize::normalize;

pub fn run(db_path: &Path, project: &str, prefix: &str, json_out: bool) -> Result<()> {
    let conn = agentic_core::db::open(db_path)?;
    let entries = worktree::list(&conn, project, prefix)?;
    let mut changed: Vec<(String, Vec<u8>, String, Option<String>)> = Vec::new();
    for (path, _sha) in entries.iter().filter(|(p, _)| p.ends_with(".md")) {
        let blob = worktree::read_at(&conn, project, path)?;
        let orig = String::from_utf8_lossy(&blob.content).to_string();
        let norm = normalize(&orig);
        if norm != orig {
            changed.push((path.clone(), norm.into_bytes(), "text/markdown".to_string(), None));
        }
    }
    let n = changed.len();
    if n > 0 {
        worktree::put_many(
            &conn,
            project,
            &changed,
            "normalize",
            &format!("normalize {n} deliverable(s)"),
            false,
        )?;
    }
    if json_out {
        println!("{}", json!({ "normalized": n }));
    } else {
        println!("normalized {n} deliverable(s) under '{prefix}'");
    }
    Ok(())
}
