//! `agentic profile` — perception-improvement P-1.
//!
//! CLI for the first-class named profile bundles backed by
//! `agentic_core::profile`. Put / get / list / resolve.

use std::path::Path;

use anyhow::{Context, Result, anyhow};

use crate::cli::ProfileAction;

pub fn run(db_path: &Path, action: ProfileAction, json_out: bool) -> Result<()> {
    let conn = agentic_core::db::open(db_path)?;
    match action {
        ProfileAction::Put {
            project,
            name,
            attach,
            settings,
            settings_file,
        } => {
            let settings_text = match (settings, settings_file) {
                (Some(s), None) => s,
                (None, Some(f)) => std::fs::read_to_string(&f)
                    .with_context(|| format!("reading {}", f.display()))?,
                (None, None) => "{}".to_string(),
                (Some(_), Some(_)) => {
                    anyhow::bail!("--settings and --settings-file are mutually exclusive")
                }
            };
            let settings_json: serde_json::Value =
                serde_json::from_str(&settings_text).context("parsing settings JSON")?;
            let profile = agentic_core::profile::Profile {
                name: name.clone(),
                attach_sections: attach,
                settings: settings_json,
            };
            let id = agentic_core::profile::put(&conn, &project, &profile)?;
            if json_out {
                println!(
                    "{}",
                    serde_json::json!({"entry_id": id, "name": name, "ok": true})
                );
            } else {
                println!("Wrote profile '{name}' as passport entry {id}");
            }
        }
        ProfileAction::Get { project, name } => {
            let p = agentic_core::profile::get(&conn, &project, &name)?
                .ok_or_else(|| anyhow!("no profile named '{name}'"))?;
            println!("{}", serde_json::to_string_pretty(&p)?);
        }
        ProfileAction::List { project } => {
            let all = agentic_core::profile::list(&conn, &project)?;
            if json_out {
                let pretty: Vec<_> = all
                    .iter()
                    .map(|(id, p)| serde_json::json!({"entry_id": id, "profile": p}))
                    .collect();
                println!("{}", serde_json::to_string_pretty(&pretty)?);
            } else if all.is_empty() {
                println!("(no profiles)");
            } else {
                println!("{} profile(s):", all.len());
                for (id, p) in &all {
                    let attaches = if p.attach_sections.is_empty() {
                        "(unattached)".to_string()
                    } else {
                        p.attach_sections.join(", ")
                    };
                    println!("  #{id:<5} {name:<32} → {attaches}", name = p.name);
                }
            }
        }
        ProfileAction::Resolve { project, section } => {
            match agentic_core::profile::resolve_for_section(&conn, &project, &section)? {
                Some(p) => println!("{}", serde_json::to_string_pretty(&p)?),
                None => {
                    if json_out {
                        println!("null");
                    } else {
                        println!("(no profile attached to '{section}')");
                    }
                }
            }
        }
    }
    Ok(())
}
