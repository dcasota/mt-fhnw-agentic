//! `agentic promote` — the SDD-Cycle promotion gate (ADR-0047 R6/R8).
//!
//! The Cascade is advisory (it records every verdict but never blocks). Promotion
//! is where governance decides: this reads the LATEST verdict per gate
//! checkpoint, BLOCKS if any is FAIL, and on a clean run records the promotion as
//! a `submission_freeze` verdict and issues the audited authorisations for any
//! held irreversible actions (so e.g. `publish`/`translate` become permitted).

use std::path::Path;

use anyhow::{Result, bail};
use rusqlite::params;
use serde_json::json;

pub fn run(
    db_path: &Path,
    project: &str,
    issue: &[String],
    snapshot: Option<&str>,
    json_out: bool,
) -> Result<()> {
    let conn = agentic_core::db::open(db_path)?;

    // Latest verdict per checkpoint (excluding the promotion marker itself).
    let mut stmt = conn.prepare(
        "SELECT checkpoint, verdict FROM audit_verdicts a \
         WHERE project_id = ?1 AND checkpoint != 'submission_freeze' \
           AND id = (SELECT MAX(id) FROM audit_verdicts b \
                     WHERE b.project_id = a.project_id AND b.checkpoint = a.checkpoint)",
    )?;
    let rows: Vec<(String, String)> = stmt
        .query_map(params![project], |r| Ok((r.get(0)?, r.get(1)?)))?
        .filter_map(std::result::Result::ok)
        .collect();

    let blocking: Vec<String> = rows
        .iter()
        .filter(|(_, v)| v == "FAIL")
        .map(|(c, _)| c.clone())
        .collect();
    let evaluated = rows.len();

    if !blocking.is_empty() {
        // Record the blocked promotion, then refuse (governance blocks on FAIL).
        let _ = conn.execute(
            "INSERT INTO audit_verdicts (project_id, checkpoint, verdict, findings_json) \
             VALUES (?1, 'submission_freeze', 'FAIL', ?2)",
            params![project, json!({ "blocking": blocking }).to_string()],
        );
        bail!(
            "promotion BLOCKED: {} gate(s) FAIL ({}). Resolve them (or record an audited \
override) before promoting.",
            blocking.len(),
            blocking.join(", ")
        );
    }

    // Clean run → record the promotion and issue the requested authorisations.
    let findings = json!({
        "evaluated": evaluated,
        "snapshot": snapshot,
        "issued": issue,
    });
    conn.execute(
        "INSERT INTO audit_verdicts (project_id, checkpoint, verdict, findings_json) \
         VALUES (?1, 'submission_freeze', 'PASS', ?2)",
        params![project, findings.to_string()],
    )?;
    let mut issued = Vec::new();
    for action in issue {
        let id = agentic_core::authz::grant(
            &conn,
            project,
            action,
            "*",
            "issued on SDD-Cycle promotion",
            "sdd-cycle",
        )?;
        issued.push(json!({ "id": id, "action": action }));
    }

    if json_out {
        println!(
            "{}",
            json!({
                "promoted": true, "evaluated": evaluated,
                "snapshot": snapshot, "issued": issued,
            })
        );
    } else {
        println!("PROMOTED — {evaluated} gate verdict(s) clean (no FAIL).");
        if let Some(s) = snapshot {
            println!("  citable snapshot: {s}");
        }
        for action in issue {
            println!("  issued authorisation: {action} (scope '*')");
        }
    }
    Ok(())
}
