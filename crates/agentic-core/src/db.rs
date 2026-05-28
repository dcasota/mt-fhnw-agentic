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
pub const NEWEST_SCHEMA_VERSION: u32 = 14;

/// `CREATE TABLE IF NOT EXISTS` for tables that must exist even when a live DB
/// predates the migration that introduces them. The migration runner only
/// applies migrations whose version exceeds the recorded max, so a DB that was
/// already at the newest version when a new table was added would otherwise
/// never gain it. This runs unconditionally on every [`open`].
const ENSURE_TABLES: &str = "\
CREATE TABLE IF NOT EXISTS orchestration_sessions (
    id              TEXT PRIMARY KEY,
    project_id      TEXT NOT NULL,
    task            TEXT NOT NULL,
    budget_usd      REAL NOT NULL DEFAULT 3.0,
    model           TEXT,
    status          TEXT NOT NULL DEFAULT 'pending'
                    CHECK (status IN ('pending','running','done','failed','ratelimited')),
    transcript_path TEXT,
    exit_summary    TEXT,
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now')),
    closed_at       TEXT
);
CREATE INDEX IF NOT EXISTS idx_orchestration_project ON orchestration_sessions(project_id, status);
";

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
    (
        4,
        "0004_audit_signatures",
        include_str!("../migrations/0004_audit_signatures.sql"),
    ),
    (
        5,
        "0005_inbox_items",
        include_str!("../migrations/0005_inbox_items.sql"),
    ),
    (
        6,
        "0006_orchestration_sessions",
        include_str!("../migrations/0006_orchestration_sessions.sql"),
    ),
    (
        7,
        "0007_audit_verdicts_i18n",
        include_str!("../migrations/0007_audit_verdicts_i18n.sql"),
    ),
    (
        8,
        "0008_audit_verdicts_bookkit",
        include_str!("../migrations/0008_audit_verdicts_bookkit.sql"),
    ),
    (
        9,
        "0009_audit_verdicts_nine_gates",
        include_str!("../migrations/0009_audit_verdicts_nine_gates.sql"),
    ),
    (
        10,
        "0010_audit_verdicts_ars_gates",
        include_str!("../migrations/0010_audit_verdicts_ars_gates.sql"),
    ),
    (
        11,
        "0011_authz_and_tombstones",
        include_str!("../migrations/0011_authz_and_tombstones.sql"),
    ),
    (
        12,
        "0012_audit_verdicts_freshness",
        include_str!("../migrations/0012_audit_verdicts_freshness.sql"),
    ),
    (
        13,
        "0013_cascade_checkpoints",
        include_str!("../migrations/0013_cascade_checkpoints.sql"),
    ),
    (
        14,
        "0014_passport_profiles",
        include_str!("../migrations/0014_passport_profiles.sql"),
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
    // Safety net for live DBs already at the newest version when a table was
    // added (the monotonic runner would otherwise skip the migration).
    conn.execute_batch(ENSURE_TABLES)?;
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
