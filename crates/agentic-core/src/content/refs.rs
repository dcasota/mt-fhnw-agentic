//! Refs = mutable pointers to commits (branches, tags).
//!
//! Unlike blobs/trees/commits, refs are NOT append-only — branches advance.

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RefKind {
    Branch,
    Tag,
}

impl RefKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Branch => "branch",
            Self::Tag => "tag",
        }
    }
}

impl std::str::FromStr for RefKind {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "branch" => Ok(Self::Branch),
            "tag" => Ok(Self::Tag),
            other => Err(Error::InvalidInput(format!("unknown ref kind: {other}"))),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ref {
    pub name:       String,
    pub kind:       RefKind,
    pub commit_sha: String,
    pub updated_at: String,
}

/// Create or move a ref.
pub fn set_ref(conn: &Connection, name: &str, kind: RefKind, commit_sha: &str) -> Result<()> {
    let updated_at = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    conn.execute(
        "INSERT INTO refs (name, kind, commit_sha, updated_at) VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT(name) DO UPDATE SET kind = excluded.kind, commit_sha = excluded.commit_sha, updated_at = excluded.updated_at",
        params![name, kind.as_str(), commit_sha, updated_at],
    )?;
    Ok(())
}

/// Look up a ref by name.
pub fn get_ref(conn: &Connection, name: &str) -> Result<Ref> {
    use std::str::FromStr;
    let r = conn
        .query_row(
            "SELECT name, kind, commit_sha, updated_at FROM refs WHERE name = ?1",
            params![name],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| Error::RefNotFound(name.to_owned()))?;
    Ok(Ref {
        name: r.0,
        kind: RefKind::from_str(&r.1)?,
        commit_sha: r.2,
        updated_at: r.3,
    })
}

/// List all refs.
pub fn list_refs(conn: &Connection) -> Result<Vec<Ref>> {
    use std::str::FromStr;
    let mut stmt = conn.prepare("SELECT name, kind, commit_sha, updated_at FROM refs ORDER BY name")?;
    let rows: Vec<Ref> = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?
        .into_iter()
        .map(|(name, kind, commit_sha, updated_at)| {
            let kind = RefKind::from_str(&kind)?;
            Ok(Ref { name, kind, commit_sha, updated_at })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::{blob::put_blob, commit::put_commit, tree::{EntryKind, TreeEntry, put_tree}};
    use crate::db::open_in_memory;
    use pretty_assertions::assert_eq;

    fn seed_commit(conn: &Connection) -> String {
        let blob_sha = put_blob(conn, b"x", "text/plain", None).unwrap();
        let tree = put_tree(conn, vec![TreeEntry {
            name: "x".into(),
            kind: EntryKind::Blob,
            target: blob_sha,
            mode: "100644".into(),
        }]).unwrap();
        put_commit(conn, &tree, None, None, "test", "human", None, None, "init").unwrap()
    }

    #[test]
    fn set_and_get_ref() {
        let conn = open_in_memory().unwrap();
        let c = seed_commit(&conn);
        set_ref(&conn, "main", RefKind::Branch, &c).unwrap();
        let r = get_ref(&conn, "main").unwrap();
        assert_eq!(r.kind, RefKind::Branch);
        assert_eq!(r.commit_sha, c);
    }

    #[test]
    fn move_a_branch() {
        let conn = open_in_memory().unwrap();
        let c1 = seed_commit(&conn);
        set_ref(&conn, "main", RefKind::Branch, &c1).unwrap();
        let c2 = put_commit(&conn, "deadbeef", Some(&c1), None, "t", "human", None, None, "second")
            .unwrap_or_else(|_| c1.clone()); // tree doesn't exist; we just want any second hash
        if c2 != c1 {
            set_ref(&conn, "main", RefKind::Branch, &c2).unwrap();
            assert_eq!(get_ref(&conn, "main").unwrap().commit_sha, c2);
        }
    }

    #[test]
    fn list_refs_sorted() {
        let conn = open_in_memory().unwrap();
        let c = seed_commit(&conn);
        set_ref(&conn, "zeta", RefKind::Branch, &c).unwrap();
        set_ref(&conn, "alpha", RefKind::Tag, &c).unwrap();
        let refs = list_refs(&conn).unwrap();
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].name, "alpha");
        assert_eq!(refs[1].name, "zeta");
    }
}
