//! Code-enforced irreversible-action authorisation (ADR-0047 R7).
//!
//! Irreversible actions (push-to-main, tag, publish, translate, supersede,
//! content-delete) must carry an audited authorisation record issued by the
//! governance layer (Mission-Control / SDD Cycle). The tool hard-refuses such an
//! action when no valid (matching-scope, unconsumed) grant exists — in every
//! execution context (autonomous cascade, CLI, resumed run), not just an AI
//! session. The grant is created by `agentic authorize grant`.

use anyhow::Result;
use rusqlite::{Connection, params};

/// The irreversible actions that require an authorisation record.
pub const IRREVERSIBLE: &[&str] = &[
    "push_main",
    "tag",
    "publish",
    "translate",
    "supersede",
    "content_delete",
];

/// Is `action` one the policy treats as irreversible?
#[must_use]
pub fn is_irreversible(action: &str) -> bool {
    IRREVERSIBLE.contains(&action)
}

/// Is there a valid (unconsumed, scope-matching) authorisation for `action`?
/// A grant with `scope = '*'` authorises any scope.
pub fn is_authorized(conn: &Connection, project: &str, action: &str, scope: &str) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM action_authorizations \
         WHERE project_id = ?1 AND action = ?2 AND (scope = '*' OR scope = ?3) \
           AND consumed_at IS NULL",
        params![project, action, scope],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// Hard guard: `Ok(())` if authorised, else an error naming how to authorise.
/// Non-irreversible actions are always allowed.
pub fn require(conn: &Connection, project: &str, action: &str, scope: &str) -> Result<()> {
    if !is_irreversible(action) || is_authorized(conn, project, action, scope)? {
        return Ok(());
    }
    anyhow::bail!(
        "action '{action}' (scope '{scope}') is irreversible and requires an audited \
authorisation from Mission-Control / SDD Cycle — run \
`agentic authorize grant --project {project} --action {action} --scope '{scope}' --rationale <why>`"
    )
}

/// Record an authorisation; returns its row id.
pub fn grant(
    conn: &Connection,
    project: &str,
    action: &str,
    scope: &str,
    rationale: &str,
    issued_by: &str,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO action_authorizations (project_id, action, scope, rationale, issued_by) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![project, action, scope, rationale, issued_by],
    )?;
    Ok(conn.last_insert_rowid())
}

/// A recorded authorisation row.
#[derive(Debug, Clone)]
pub struct Authorization {
    pub id: i64,
    pub action: String,
    pub scope: String,
    pub rationale: String,
    pub issued_by: String,
    pub ts: String,
    pub consumed_at: Option<String>,
}

/// All authorisations for a project, oldest first.
pub fn list(conn: &Connection, project: &str) -> Result<Vec<Authorization>> {
    let mut stmt = conn.prepare(
        "SELECT id, action, scope, rationale, issued_by, ts, consumed_at \
         FROM action_authorizations WHERE project_id = ?1 ORDER BY id",
    )?;
    let rows = stmt.query_map(params![project], |r| {
        Ok(Authorization {
            id: r.get(0)?,
            action: r.get(1)?,
            scope: r.get(2)?,
            rationale: r.get(3)?,
            issued_by: r.get(4)?,
            ts: r.get(5)?,
            consumed_at: r.get(6)?,
        })
    })?;
    Ok(rows.filter_map(std::result::Result::ok).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        db::open_in_memory,
        project::{ProjectKind, create as create_project},
    };

    #[test]
    fn require_blocks_then_allows_after_grant() {
        let conn = open_in_memory().unwrap();
        let p = create_project(&conn, "T", ProjectKind::Thesis, "en", None).unwrap();
        // A non-irreversible action is always allowed.
        assert!(require(&conn, &p, "render", "x").is_ok());
        // An irreversible action is refused without a grant…
        assert!(require(&conn, &p, "supersede", "out/sources/x.md").is_err());
        // …and allowed once granted for that scope.
        grant(
            &conn,
            &p,
            "supersede",
            "out/sources/x.md",
            "obsolete draft",
            "sdd-cycle",
        )
        .unwrap();
        assert!(require(&conn, &p, "supersede", "out/sources/x.md").is_ok());
        // A different scope is still refused.
        assert!(require(&conn, &p, "supersede", "out/sources/y.md").is_err());
        assert_eq!(list(&conn, &p).unwrap().len(), 1);
    }

    #[test]
    fn wildcard_scope_authorises_any() {
        let conn = open_in_memory().unwrap();
        let p = create_project(&conn, "T", ProjectKind::Thesis, "en", None).unwrap();
        grant(
            &conn,
            &p,
            "publish",
            "*",
            "release window open",
            "mission-control",
        )
        .unwrap();
        assert!(require(&conn, &p, "publish", "anything").is_ok());
    }
}
