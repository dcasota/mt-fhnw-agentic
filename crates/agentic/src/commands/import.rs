//! `agentic import …` handler.

use std::path::Path;

use anyhow::{Result, anyhow};

use agentic_import::{ImportOutcome, import_dir, import_file};

use crate::cli::ImportAction;

pub fn run(db_path: &Path, action: ImportAction, json: bool) -> Result<()> {
    let conn = agentic_core::db::open(db_path)?;
    match action {
        ImportAction::File {
            path,
            project,
            to,
            author,
            message,
            lang,
        } => {
            let outcome = import_file(
                &conn,
                &project,
                &path,
                &to,
                &author,
                message.as_deref().unwrap_or("import file"),
                lang.as_deref(),
            )?;
            report_one(&outcome, json)
        }
        ImportAction::Dir {
            path,
            project,
            prefix,
            author,
            message,
            lang,
        } => {
            let outcomes = import_dir(
                &conn,
                &project,
                &path,
                &prefix,
                &author,
                message.as_deref().unwrap_or("import dir"),
                lang.as_deref(),
            )?;
            report_many(&outcomes, json)
        }
    }
}

fn report_one(o: &ImportOutcome, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(o)?);
    } else {
        println!(
            "Imported {} → {} ({} → {} bytes, format={}, commit={})",
            o.source, o.target_path, o.bytes_in, o.bytes_out, o.format, o.commit_sha
        );
    }
    Ok(())
}

fn report_many(items: &[ImportOutcome], json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(items)?);
        // JSON consumers parse `success` per item; we still want a non-zero
        // exit code when any file failed, otherwise scripts can't tell.
        if items.iter().any(|o| !o.success) {
            return Err(anyhow!(
                "{} of {} file(s) failed (see `success`/`error` fields)",
                items.iter().filter(|o| !o.success).count(),
                items.len()
            ));
        }
        return Ok(());
    }
    if items.is_empty() {
        println!("(no supported files found)");
        return Ok(());
    }
    let (ok, fail): (Vec<_>, Vec<_>) = items.iter().partition(|o| o.success);
    for o in &ok {
        println!(
            "  + {:<6} {} → {} ({} bytes)",
            o.format, o.source, o.target_path, o.bytes_out
        );
    }
    for o in &fail {
        println!(
            "  - {:<6} {} → {} FAILED ({})",
            o.format,
            o.source,
            o.target_path,
            o.error.as_deref().unwrap_or("unknown error")
        );
    }
    println!("\n{} imported, {} failed.", ok.len(), fail.len());
    if !fail.is_empty() {
        return Err(anyhow!(
            "{} of {} file(s) failed to import",
            fail.len(),
            items.len()
        ));
    }
    Ok(())
}
