//! `agentic authorize` — grant/list irreversible-action authorisations (ADR-0047 R7).

use std::path::Path;

use anyhow::Result;
use serde_json::json;

use crate::cli::AuthorizeAction;

pub fn run(db_path: &Path, action: AuthorizeAction, json_out: bool) -> Result<()> {
    let conn = agentic_core::db::open(db_path)?;
    match action {
        AuthorizeAction::Grant {
            project,
            action,
            scope,
            rationale,
            by,
        } => {
            if !agentic_core::authz::is_irreversible(&action) {
                eprintln!(
                    "note: '{action}' is not an irreversible action; the grant is recorded but unnecessary"
                );
            }
            let id = agentic_core::authz::grant(&conn, &project, &action, &scope, &rationale, &by)?;
            if json_out {
                println!(
                    "{}",
                    json!({ "id": id, "action": action, "scope": scope, "issued_by": by })
                );
            } else {
                println!("granted authorisation #{id}: {action} (scope '{scope}') by {by}");
            }
        }
        AuthorizeAction::List { project } => {
            let rows = agentic_core::authz::list(&conn, &project)?;
            if json_out {
                let arr: Vec<_> = rows
                    .iter()
                    .map(|a| {
                        json!({
                            "id": a.id, "action": a.action, "scope": a.scope,
                            "rationale": a.rationale, "issued_by": a.issued_by,
                            "ts": a.ts, "consumed_at": a.consumed_at,
                        })
                    })
                    .collect();
                println!("{}", json!({ "authorizations": arr }));
            } else if rows.is_empty() {
                println!("no authorisations for project {project}");
            } else {
                for a in rows {
                    let state = if a.consumed_at.is_some() {
                        "consumed"
                    } else {
                        "valid"
                    };
                    println!(
                        "#{:<4} {:<14} scope={:<24} [{}] by {} \u{2014} {}",
                        a.id, a.action, a.scope, state, a.issued_by, a.rationale
                    );
                }
            }
        }
    }
    Ok(())
}
