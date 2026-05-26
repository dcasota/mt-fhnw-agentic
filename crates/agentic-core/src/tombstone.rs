//! Content tombstones (ADR-0047 R5).
//!
//! The content store is append-only; nothing is hard-deleted. A path is retired
//! by recording a *tombstone* (with the authorising grant). Tombstoned paths are
//! filtered out of [`crate::worktree::list`], so every gate and build that walks
//! the working tree automatically skips superseded content, while the full
//! history (blobs + the tombstone record) is retained for audit.

use std::collections::HashSet;

use anyhow::Result;
use rusqlite::{Connection, params};

/// Record a tombstone retiring `path` (with the authorising grant id, if any).
pub fn add(
    conn: &Connection,
    project: &str,
    path: &str,
    reason: &str,
    authorization_id: Option<i64>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO tombstones (project_id, path, reason, authorization_id) \
         VALUES (?1, ?2, ?3, ?4)",
        params![project, path, reason, authorization_id],
    )?;
    Ok(())
}

/// The set of tombstoned paths for a project.
pub fn tombstoned_paths(conn: &Connection, project: &str) -> Result<HashSet<String>> {
    let mut stmt = conn.prepare("SELECT DISTINCT path FROM tombstones WHERE project_id = ?1")?;
    let rows = stmt.query_map(params![project], |r| r.get::<_, String>(0))?;
    Ok(rows.filter_map(std::result::Result::ok).collect())
}

/// Is `path` tombstoned in `project`?
pub fn is_tombstoned(conn: &Connection, project: &str, path: &str) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM tombstones WHERE project_id = ?1 AND path = ?2",
        params![project, path],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        db::open_in_memory,
        project::{ProjectKind, create as create_project},
    };

    #[test]
    fn tombstone_recorded_and_listed() {
        let conn = open_in_memory().unwrap();
        let p = create_project(&conn, "T", ProjectKind::Thesis, "en", None).unwrap();
        assert!(!is_tombstoned(&conn, &p, "out/sources/x.md").unwrap());
        add(&conn, &p, "out/sources/x.md", "superseded by merge", None).unwrap();
        assert!(is_tombstoned(&conn, &p, "out/sources/x.md").unwrap());
        assert!(
            tombstoned_paths(&conn, &p)
                .unwrap()
                .contains("out/sources/x.md")
        );
    }
}
