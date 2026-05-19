use std::path::Path;
use std::str::FromStr;

use anyhow::Result;
use serde_json::json;

use agentic_core::project::{self, ProjectKind};

use crate::cli::ProjectAction;

pub fn run(db_path: &Path, action: ProjectAction, json_out: bool) -> Result<()> {
    let conn = agentic_core::db::open(db_path)?;
    match action {
        ProjectAction::New {
            name,
            kind,
            working_lang,
            parent,
        } => {
            let kind = ProjectKind::from_str(&kind)?;
            let id = project::create(&conn, &name, kind, &working_lang, parent.as_deref())?;
            if json_out {
                println!("{}", json!({ "id": id, "name": name }));
            } else {
                println!("Created project {id} ({name})");
            }
        }
        ProjectAction::List => {
            let projects = project::list(&conn)?;
            if json_out {
                println!("{}", serde_json::to_string_pretty(&projects)?);
            } else if projects.is_empty() {
                println!("No projects yet. Run `agentic project new <name>`.");
            } else {
                println!("{:26} {:14} {:5} {}", "ID", "KIND", "LANG", "NAME");
                for p in projects {
                    println!(
                        "{:26} {:14} {:5} {}",
                        p.id,
                        p.kind.as_str(),
                        p.working_lang,
                        p.name
                    );
                }
            }
        }
        ProjectAction::Status { id } => {
            let id =
                id.ok_or_else(|| anyhow::anyhow!("--id required (no 'current project' yet)"))?;
            let p = project::get(&conn, &id)?;
            if json_out {
                println!("{}", serde_json::to_string_pretty(&p)?);
            } else {
                println!("Project: {} ({})", p.name, p.id);
                println!("  Kind:    {}", p.kind.as_str());
                println!("  Lang:    {}", p.working_lang);
                println!("  Status:  {:?}", p.status);
                println!("  Created: {}", p.created_at);
            }
        }
        ProjectAction::Archive { id: _ } => {
            anyhow::bail!("project archive: not implemented in P0");
        }
    }
    Ok(())
}
