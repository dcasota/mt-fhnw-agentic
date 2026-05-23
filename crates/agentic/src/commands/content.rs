use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::json;

use agentic_core::content::{blob, commit};
use agentic_core::worktree;

use crate::cli::ContentAction;

pub fn run(db_path: &Path, action: ContentAction, json_out: bool) -> Result<()> {
    let conn = agentic_core::db::open(db_path)?;
    match action {
        ContentAction::Put { path, lang } => {
            let bytes =
                std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
            let mime = mime_from_extension(&path);
            let sha = blob::put_blob(&conn, &bytes, &mime, lang.as_deref())?;
            if json_out {
                println!("{}", json!({ "sha256": sha, "size": bytes.len() }));
            } else {
                println!("{sha}  {} bytes  {}", bytes.len(), path.display());
            }
        }
        ContentAction::PutAt {
            path,
            from,
            project,
            lang,
            author,
            message,
        } => {
            let bytes = read_source(&from)?;
            let mime = mime_from_path_str(&path);
            let msg = message.unwrap_or_else(|| format!("put {path}"));
            let commit_sha = worktree::put_at(
                &conn,
                &project,
                &path,
                &bytes,
                &mime,
                lang.as_deref(),
                &author,
                &msg,
            )?;
            if json_out {
                println!(
                    "{}",
                    json!({ "commit": commit_sha, "path": path, "size": bytes.len() })
                );
            } else {
                println!("{commit_sha}  {} bytes  {path}", bytes.len());
            }
        }
        ContentAction::ReadAt { path, project } => {
            let b = worktree::read_at(&conn, &project, &path)?;
            std::io::stdout().write_all(&b.content)?;
        }
        ContentAction::Ls { project, prefix } => {
            let entries = worktree::list(&conn, &project, &prefix)?;
            if json_out {
                println!("{}", serde_json::to_string_pretty(&entries)?);
            } else if entries.is_empty() {
                println!("(empty working tree)");
            } else {
                for (path, sha) in entries {
                    println!("{}  {path}", &sha[..12]);
                }
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
                    println!(
                        "{}  {}  {}  {}",
                        &c.sha256[..12],
                        c.timestamp,
                        c.author,
                        c.message
                    );
                }
            }
        }
        ContentAction::Ingest {
            project,
            root,
            from_list,
            replace,
            author,
            message,
        } => {
            // Collect the relative paths to stage: either an explicit list
            // (e.g. `git ls-files`) or a recursive walk of --root.
            let rels: Vec<String> = match &from_list {
                Some(src) => {
                    let text = if src == "-" {
                        let mut s = String::new();
                        std::io::stdin().read_to_string(&mut s)?;
                        s
                    } else {
                        std::fs::read_to_string(src)
                            .with_context(|| format!("reading list {src}"))?
                    };
                    text.lines()
                        .map(|l| l.trim())
                        .filter(|l| !l.is_empty())
                        .map(|l| l.replace('\\', "/"))
                        .collect()
                }
                None => walk_rel(&root)?,
            };

            let mut files: Vec<(String, Vec<u8>, String, Option<String>)> =
                Vec::with_capacity(rels.len());
            let mut skipped = 0usize;
            for rel in &rels {
                let native = rel.replace('/', std::path::MAIN_SEPARATOR_STR);
                let abs = root.join(&native);
                match std::fs::read(&abs) {
                    Ok(bytes) => {
                        let mime = mime_from_path_str(rel);
                        files.push((rel.clone(), bytes, mime, None));
                    }
                    Err(_) => skipped += 1,
                }
            }
            if files.is_empty() {
                anyhow::bail!(
                    "no readable files to ingest (list had {} entries)",
                    rels.len()
                );
            }
            let msg = message.unwrap_or_else(|| format!("ingest {} files", files.len()));
            let commit_sha = worktree::put_many(&conn, &project, &files, &author, &msg, replace)?;
            if json_out {
                println!(
                    "{}",
                    json!({ "commit": commit_sha, "staged": files.len(), "skipped": skipped })
                );
            } else {
                println!(
                    "{commit_sha}  staged {} files  (skipped {skipped} unreadable)",
                    files.len()
                );
            }
        }
        ContentAction::Checkout {
            project,
            to,
            prefix,
        } => {
            let entries = worktree::list(&conn, &project, &prefix)?;
            let mut n = 0usize;
            for (path, sha) in &entries {
                let b = blob::get_blob(&conn, sha)?;
                let native = path.replace('/', std::path::MAIN_SEPARATOR_STR);
                let target = to.join(&native);
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)
                        .with_context(|| format!("mkdir {}", parent.display()))?;
                }
                std::fs::write(&target, &b.content)
                    .with_context(|| format!("writing {}", target.display()))?;
                n += 1;
            }
            if json_out {
                println!(
                    "{}",
                    json!({ "checked_out": n, "to": to.display().to_string() })
                );
            } else {
                println!("Checked out {n} files to {}", to.display());
            }
        }
    }
    Ok(())
}

/// Recursively list files under `root` as forward-slash relative paths,
/// skipping dot-directories and common build/cache dirs.
fn walk_rel(root: &Path) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in
            std::fs::read_dir(&dir).with_context(|| format!("reading dir {}", dir.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if entry.file_type()?.is_dir() {
                if name.starts_with('.')
                    || name == "target"
                    || name == "node_modules"
                    || name == "__pycache__"
                {
                    continue;
                }
                stack.push(path);
            } else if let Ok(rel) = path.strip_prefix(root) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    out.sort();
    Ok(out)
}

fn read_source(from: &str) -> Result<Vec<u8>> {
    if from == "-" {
        let mut buf = Vec::new();
        std::io::stdin().read_to_end(&mut buf)?;
        Ok(buf)
    } else {
        let p = PathBuf::from(from);
        std::fs::read(&p).with_context(|| format!("reading {}", p.display()))
    }
}

fn mime_from_path_str(path: &str) -> String {
    if let Some(ext) = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
    {
        return match ext {
            "md" => "text/markdown".into(),
            "json" => "application/json".into(),
            "yaml" | "yml" => "application/yaml".into(),
            "toml" => "application/toml".into(),
            "txt" => "text/plain".into(),
            "pdf" => "application/pdf".into(),
            "docx" => {
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document".into()
            }
            "pptx" => {
                "application/vnd.openxmlformats-officedocument.presentationml.presentation".into()
            }
            "png" => "image/png".into(),
            "jpg" | "jpeg" => "image/jpeg".into(),
            _ => "application/octet-stream".into(),
        };
    }
    "application/octet-stream".into()
}

fn mime_from_extension(path: &Path) -> String {
    mime_from_path_str(&path.display().to_string())
}
