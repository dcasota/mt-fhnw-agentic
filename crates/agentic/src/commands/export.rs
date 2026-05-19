//! `agentic export <project> --format docx|pdf [--to path]` handler.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};

use agentic_export::{render_docx, render_pdf};

pub fn run(
    db_path: &Path,
    project: &str,
    format: &str,
    to: Option<&Path>,
    prefix: &str,
    title: Option<&str>,
    json: bool,
) -> Result<()> {
    let conn = agentic_core::db::open(db_path)?;

    // Resolve the title: explicit override > project name > project id.
    let project_row = agentic_core::project::get(&conn, project)
        .with_context(|| format!("look up project {project}"))?;
    let resolved_title = title
        .map(str::to_owned)
        .unwrap_or_else(|| project_row.name.clone());

    let bytes = match format {
        "docx" => render_docx(&conn, project, prefix, &resolved_title)?,
        "pdf" => render_pdf(&conn, project, prefix, &resolved_title)?,
        other => return Err(anyhow!("unknown format: {other} (expected docx|pdf)")),
    };

    match to {
        Some(path) => {
            std::fs::write(path, &bytes).with_context(|| format!("write {}", path.display()))?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "project_id": project,
                        "format": format,
                        "bytes": bytes.len(),
                        "to": path.display().to_string(),
                    }))?
                );
            } else {
                println!(
                    "Wrote {} ({} bytes) to {}",
                    format,
                    bytes.len(),
                    path.display()
                );
            }
        }
        None => {
            // Binary safe write: PDFs and DOCXs are not text.
            std::io::stdout()
                .write_all(&bytes)
                .context("write export bytes to stdout")?;
        }
    }
    Ok(())
}

/// Free function helper for [`crate::commands::dispatch`].
pub fn from_args(
    db: &Path,
    project: String,
    format: String,
    to: Option<PathBuf>,
    prefix: String,
    title: Option<String>,
    json: bool,
) -> Result<()> {
    run(
        db,
        &project,
        &format,
        to.as_deref(),
        &prefix,
        title.as_deref(),
        json,
    )
}
