//! Database open + migration runner.
//!
//! Migrations live in `migrations/NNNN_*.sql` and are compiled into the binary
//! via `include_str!`. The runner is monotonic: each migration's version is
//! recorded in `schema_version`; the next run only applies migrations whose
//! version is greater than the maximum already-applied version.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};
use tracing::{debug, info};

use crate::{Error, Result};

/// Newest schema version known to this build.
pub const NEWEST_SCHEMA_VERSION: u32 = 3;

const MIGRATIONS: &[(u32, &str, &str)] = &[
    (
        1,
        "0001_initial",
        include_str!("../migrations/0001_initial.sql"),
    ),
    (
        2,
        "0002_wizard_drafts",
        include_str!("../migrations/0002_wizard_drafts.sql"),
    ),
    (
        3,
        "0003_embeddings",
        include_str!("../migrations/0003_embeddings.sql"),
    ),
];

/// Open `path`, creating it if missing, and apply pending migrations.
pub fn open(path: impl AsRef<Path>) -> Result<Connection> {
    let path: PathBuf = path.as_ref().to_path_buf();
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let conn = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_URI,
    )?;
    conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL;")?;
    migrate(&conn)?;
    Ok(conn)
}

/// Open an in-memory DB (used by tests).
pub fn open_in_memory() -> Result<Connection> {
    let conn = Connection::open_in_memory()?;
    conn.execute_batch("PRAGMA foreign_keys = ON;")?;
    migrate(&conn)?;
    Ok(conn)
}

/// Apply any pending migrations.
pub fn migrate(conn: &Connection) -> Result<()> {
    // Bootstrap: ensure `schema_version` exists.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_version (
            version    INTEGER PRIMARY KEY,
            applied_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
        );",
    )?;

    let current: u32 = conn
        .query_row(
            "SELECT IFNULL(MAX(version), 0) FROM schema_version",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);

    if current > NEWEST_SCHEMA_VERSION {
        return Err(Error::UnsupportedSchema(current, NEWEST_SCHEMA_VERSION));
    }

    for (version, name, sql) in MIGRATIONS {
        if *version > current {
            info!(version, name, "applying migration");
            conn.execute_batch(sql).map_err(|source| Error::Migration {
                version: *version,
                source,
            })?;
            debug!(version, name, "migration applied");
        }
    }
    Ok(())
}

/// Convenience: read the current `schema_version` value.
pub fn current_version(conn: &Connection) -> Result<u32> {
    Ok(conn
        .query_row(
            "SELECT IFNULL(MAX(version), 0) FROM schema_version",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn opens_in_memory_and_runs_migrations() {
        let conn = open_in_memory().unwrap();
        assert_eq!(current_version(&conn).unwrap(), NEWEST_SCHEMA_VERSION);
    }

    #[test]
    fn migrate_is_idempotent() {
        let conn = open_in_memory().unwrap();
        migrate(&conn).unwrap();
        migrate(&conn).unwrap();
        assert_eq!(current_version(&conn).unwrap(), NEWEST_SCHEMA_VERSION);
    }

    #[test]
    fn rejects_future_schema() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER PRIMARY KEY, applied_at TEXT);
             INSERT INTO schema_version (version, applied_at) VALUES (9999, '2099-01-01T00:00:00Z');",
        )
        .unwrap();
        let err = migrate(&conn).unwrap_err();
        assert!(matches!(err, Error::UnsupportedSchema(9999, _)));
    }
}
