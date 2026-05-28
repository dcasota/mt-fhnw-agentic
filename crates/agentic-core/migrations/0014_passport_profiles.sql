-- =============================================================================
--  Migration 0014: passport `profiles` section (perception P-1)
-- =============================================================================
--  Adds 'profiles' to the allowed values of passport_entries.section so the
--  new first-class named profile bundles (see `agentic_core::profile`) can
--  be stored under their own passport section without changing any other
--  semantics.
--
--  SQLite cannot extend a CHECK constraint in-place, so the migration
--  rebuilds the table with the relaxed CHECK list. Two procedural notes:
--
--    * `replaces` carries a self-FK to `passport_entries(id)`. Recreating
--      the table while a FK to its old name exists raises
--      `FOREIGN KEY constraint failed` mid-migration. Wrap the rebuild in
--      `PRAGMA foreign_keys=OFF` for the duration; re-enable after the
--      rename. (SQLite re-checks FKs at the next pragma re-enable, so the
--      schema must be consistent before that point — which it is, because
--      the new table has the same self-FK pointing at the new name after
--      the rename.)
--
--    * The migration drops any leftover `passport_entries_v2` from a
--      prior aborted run before recreating it, so a re-run after a
--      partial failure is idempotent.

PRAGMA foreign_keys = OFF;

DROP TABLE IF EXISTS passport_entries_v2;

CREATE TABLE passport_entries_v2 (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id    TEXT NOT NULL REFERENCES projects(id),
    section       TEXT NOT NULL CHECK (section IN (
        'literature_corpus',
        'claim_intent_manifest',
        'claim_audit_results',
        'temporal_audit_results',
        'timeline',
        'reset_ledger',
        'compliance_reports',
        'verified_facts',
        'profiles'
    )),
    payload_json  TEXT NOT NULL,
    added_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    commit_sha    TEXT REFERENCES commits(sha256),
    replaces      INTEGER REFERENCES passport_entries_v2(id)
);

INSERT INTO passport_entries_v2 (id, project_id, section, payload_json, added_at, commit_sha, replaces)
SELECT id, project_id, section, payload_json, added_at, commit_sha, replaces
FROM passport_entries;

DROP INDEX IF EXISTS idx_passport_project;
DROP TABLE passport_entries;
ALTER TABLE passport_entries_v2 RENAME TO passport_entries;
CREATE INDEX IF NOT EXISTS idx_passport_project ON passport_entries(project_id, section);

PRAGMA foreign_keys = ON;

INSERT OR IGNORE INTO schema_version (version) VALUES (14);
