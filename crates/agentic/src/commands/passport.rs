use std::io::Read;
use std::path::Path;
use std::str::FromStr;

use anyhow::Result;
use serde_json::json;

use agentic_core::passport::{self, Section};

use crate::cli::PassportAction;

pub fn run(db_path: &Path, action: PassportAction, json_out: bool) -> Result<()> {
    let conn = agentic_core::db::open(db_path)?;
    match action {
        PassportAction::Append {
            project,
            section,
            payload,
            replaces,
        } => {
            let section = Section::from_str(&section)?;
            let payload_json = if payload == "-" {
                let mut s = String::new();
                std::io::stdin().read_to_string(&mut s)?;
                s
            } else {
                payload
            };
            let id = passport::append(&conn, &project, section, &payload_json, None, replaces)?;
            if json_out {
                println!("{}", json!({ "id": id }));
            } else {
                println!(
                    "Appended passport entry {id} in section {} for project {project}",
                    section.as_str()
                );
            }
        }
        PassportAction::Read { project, section } => {
            let section = Section::from_str(&section)?;
            let entries = passport::current(&conn, &project, section)?;
            if json_out {
                println!("{}", serde_json::to_string_pretty(&entries)?);
            } else {
                for e in &entries {
                    println!("--- entry {} (added {}) ---", e.id, e.added_at);
                    println!("{}", e.payload_json);
                }
                println!(
                    "\n({} current entr{}.)",
                    entries.len(),
                    if entries.len() == 1 { "y" } else { "ies" }
                );
            }
        }
        PassportAction::Validate { project } => {
            let report = passport::validate(&conn, &project)?;
            if json_out {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!("Total entries:  {}", report.total_entries);
                println!("JSON errors:    {}", report.json_errors.len());
                for (id, section) in &report.json_errors {
                    println!("  - id={id} section={section}");
                }
            }
        }
    }
    Ok(())
}
