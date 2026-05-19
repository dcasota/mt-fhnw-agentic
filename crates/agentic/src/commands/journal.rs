use std::path::Path;

use anyhow::Result;
use serde_json::json;

use agentic_core::journal::{self, HallucinationRisk, NewEntry};

use crate::cli::JournalAction;

pub fn run(db_path: &Path, action: JournalAction, json_out: bool) -> Result<()> {
    let conn = agentic_core::db::open(db_path)?;
    match action {
        JournalAction::Append { project, actor, action_type, description, reasoning, hallucination_risk } => {
            let risk = match hallucination_risk.as_deref() {
                None => None,
                Some("None") => Some(HallucinationRisk::None),
                Some("Low") => Some(HallucinationRisk::Low),
                Some("Medium") => Some(HallucinationRisk::Medium),
                Some("High") => Some(HallucinationRisk::High),
                Some(other) => anyhow::bail!("hallucination_risk must be one of None|Low|Medium|High, got {other}"),
            };
            let n = journal::append(&conn, &project, &NewEntry {
                actor: &actor,
                triggered_by: None,
                action_type: &action_type,
                description: &description,
                files_affected: None,
                reasoning: reasoning.as_deref(),
                hallucination_risk: risk,
                user_approval_required: false,
                user_approval_given: None,
                commit_sha: None,
            })?;
            if json_out {
                println!("{}", json!({ "entry_no": n }));
            } else {
                println!("Appended entry {n} for project {project}");
            }
        }
        JournalAction::Show { project, last } => {
            let rows = journal::last(&conn, &project, last)?;
            if json_out {
                println!("{}", serde_json::to_string_pretty(&rows)?);
            } else {
                for e in rows.iter().rev() {
                    println!("\n## Entry {:04} — {} — {}", e.entry_no, e.ts, e.actor);
                    println!("Action:      {}", e.action_type);
                    println!("Description: {}", e.description);
                    if let Some(r) = &e.reasoning {
                        println!("Reasoning:   {r}");
                    }
                    if let Some(h) = &e.hallucination_risk {
                        println!("Risk:        {h}");
                    }
                }
            }
        }
    }
    Ok(())
}
