//! Commit = an immutable snapshot of a tree, with parents + author + message.

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::{Error, Result};

use super::hash;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Commit {
    pub sha256:       String,
    pub tree:         String,
    pub parent:       Option<String>,
    pub parent_2:     Option<String>,
    pub author:       String,
    pub actor_kind:   String,   // human | ai | hook | system
    pub actor_detail: Option<String>,
    pub iteration:    Option<i64>,
    pub message:      String,
    pub timestamp:    String,
}

/// Create a new commit. The hash incorporates tree + parents + author + message + timestamp.
pub fn put_commit(
    conn: &Connection,
    tree: &str,
    parent: Option<&str>,
    parent_2: Option<&str>,
    author: &str,
    actor_kind: &str,
    actor_detail: Option<&str>,
    iteration: Option<i64>,
    message: &str,
) -> Result<String> {
    if !matches!(actor_kind, "human" | "ai" | "hook" | "system") {
        return Err(Error::InvalidInput(format!(
            "actor_kind must be one of human|ai|hook|system, got {actor_kind}"
        )));
    }
    let timestamp = chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();
    let blueprint = format!(
        "tree={tree}\nparent={}\nparent_2={}\nauthor={author}\nactor={actor_kind}/{}\niteration={}\nmessage={message}\nts={timestamp}",
        parent.unwrap_or(""),
        parent_2.unwrap_or(""),
        actor_detail.unwrap_or(""),
        iteration.map(|n| n.to_string()).unwrap_or_default(),
    );
    let sha = hash(blueprint.as_bytes());
    conn.execute(
        "INSERT OR IGNORE INTO commits (sha256, tree, parent, parent_2, author, actor_kind, actor_detail, iteration, message, timestamp) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            sha, tree, parent, parent_2, author, actor_kind, actor_detail, iteration, message, timestamp
        ],
    )?;
    Ok(sha)
}

/// Fetch a commit by SHA-256.
pub fn get_commit(conn: &Connection, sha: &str) -> Result<Commit> {
    let commit = conn
        .query_row(
            "SELECT sha256, tree, parent, parent_2, author, actor_kind, actor_detail, iteration, message, timestamp \
             FROM commits WHERE sha256 = ?1",
            params![sha],
            |row| {
                Ok(Commit {
                    sha256: row.get(0)?,
                    tree: row.get(1)?,
                    parent: row.get(2)?,
                    parent_2: row.get(3)?,
                    author: row.get(4)?,
                    actor_kind: row.get(5)?,
                    actor_detail: row.get(6)?,
                    iteration: row.get(7)?,
                    message: row.get(8)?,
                    timestamp: row.get(9)?,
                })
            },
        )
        .optional()?
        .ok_or_else(|| Error::CommitNotFound(sha.to_owned()))?;
    Ok(commit)
}

/// Walk commits in chronological order (newest first).
pub fn log(conn: &Connection, limit: usize) -> Result<Vec<Commit>> {
    let mut stmt = conn.prepare(
        "SELECT sha256, tree, parent, parent_2, author, actor_kind, actor_detail, iteration, message, timestamp \
         FROM commits ORDER BY timestamp DESC LIMIT ?1",
    )?;
    let rows: Vec<Commit> = stmt
        .query_map(params![limit as i64], |row| {
            Ok(Commit {
                sha256: row.get(0)?,
                tree: row.get(1)?,
                parent: row.get(2)?,
                parent_2: row.get(3)?,
                author: row.get(4)?,
                actor_kind: row.get(5)?,
                actor_detail: row.get(6)?,
                iteration: row.get(7)?,
                message: row.get(8)?,
                timestamp: row.get(9)?,
            })
        })?
        .collect::<std::result::Result<_, _>>()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::{blob::put_blob, tree::{EntryKind, TreeEntry, put_tree}};
    use crate::db::open_in_memory;
    use pretty_assertions::assert_eq;

    fn seed_tree(conn: &Connection) -> String {
        let blob_sha = put_blob(conn, b"hi", "text/plain", None).unwrap();
        put_tree(conn, vec![TreeEntry {
            name: "hi.md".into(),
            kind: EntryKind::Blob,
            target: blob_sha,
            mode: "100644".into(),
        }]).unwrap()
    }

    #[test]
    fn create_and_fetch_commit() {
        let conn = open_in_memory().unwrap();
        let tree = seed_tree(&conn);
        let sha = put_commit(&conn, &tree, None, None, "test", "human", None, None, "init").unwrap();
        let c = get_commit(&conn, &sha).unwrap();
        assert_eq!(c.message, "init");
        assert_eq!(c.actor_kind, "human");
        assert_eq!(c.tree, tree);
    }

    #[test]
    fn rejects_bad_actor_kind() {
        let conn = open_in_memory().unwrap();
        let tree = seed_tree(&conn);
        let err = put_commit(&conn, &tree, None, None, "test", "bogus", None, None, "x").unwrap_err();
        assert!(matches!(err, Error::InvalidInput(_)));
    }

    #[test]
    fn log_returns_newest_first() {
        let conn = open_in_memory().unwrap();
        let tree = seed_tree(&conn);
        let c1 = put_commit(&conn, &tree, None, None, "test", "human", None, None, "first").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let c2 = put_commit(&conn, &tree, Some(&c1), None, "test", "human", None, None, "second").unwrap();
        let log = log(&conn, 10).unwrap();
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].sha256, c2);
        assert_eq!(log[1].sha256, c1);
    }
}
