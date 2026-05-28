//! `agentic rank` — perception-improvement P-4 surface.
//!
//! Per-section ADR-0046 acceptance summary; read-only against the passport.
//! See `agentic_core::rank_summary` for the join logic.

use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};

use crate::cli::RankAction;

pub fn run(db_path: &Path, action: RankAction, json_out: bool) -> Result<()> {
    let conn = agentic_core::db::open(db_path)?;
    match action {
        RankAction::Summary {
            project,
            section,
            to,
        } => {
            let all = agentic_core::rank_summary::compute(&conn, &project)?;
            let filtered: Vec<_> = if let Some(ref sl) = section {
                match agentic_core::audit_profile::Section::from_slug(sl) {
                    Some(want) => all.into_iter().filter(|r| r.section == want).collect(),
                    None => anyhow::bail!(
                        "unknown section '{sl}'; valid: dimensions, campaigns, projects, student_notes, master_thesis, agentic_handbook, audit, norms, frontmatter, other"
                    ),
                }
            } else {
                all
            };
            let body = if json_out {
                serde_json::to_string_pretty(&filtered)?
            } else {
                agentic_core::rank_summary::render_markdown(&filtered)
            };
            match to {
                Some(path) => {
                    std::fs::write(&path, body.as_bytes())
                        .with_context(|| format!("writing {}", path.display()))?;
                    println!("Wrote rank summary to {}", path.display());
                }
                None => {
                    std::io::stdout().write_all(body.as_bytes())?;
                }
            }
            Ok(())
        }
    }
}
