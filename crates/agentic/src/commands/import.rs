//! `agentic import …` handler.

use std::path::Path;

use anyhow::Result;

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
        return Ok(());
    }
    if items.is_empty() {
        println!("(no supported files found)");
        return Ok(());
    }
    for o in items {
        println!(
            "  {:<6} {} → {} ({} bytes)",
            o.format, o.source, o.target_path, o.bytes_out
        );
    }
    println!("\nImported {} file(s).", items.len());
    Ok(())
}
