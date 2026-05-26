-- =============================================================================
--  Migration 0011: irreversible-action authorisations + content tombstones
-- =============================================================================
--  ADR-0047 (R5/R7): irreversible actions (push-to-main, tag, publish,
--  translate, supersede, content_delete) are code-enforced — they require an
--  audited authorisation record issued by Mission-Control / SDD Cycle. Content
--  is never hard-deleted; it is superseded (tombstoned), and the live set is
--  defined by manifest membership + the absence of a tombstone.

CREATE TABLE IF NOT EXISTS action_authorizations (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id   TEXT NOT NULL REFERENCES projects(id),
    action       TEXT NOT NULL,                 -- push_main | tag | publish | translate | supersede | content_delete
    scope        TEXT NOT NULL DEFAULT '*',     -- path/target the grant covers; '*' = any
    rationale    TEXT NOT NULL,
    issued_by    TEXT NOT NULL,                 -- the granting governance actor
    ts           TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now')),
    consumed_at  TEXT                            -- set when a single-use grant is consumed (NULL = still valid)
);
CREATE INDEX IF NOT EXISTS idx_authz_project_action ON action_authorizations(project_id, action);

CREATE TABLE IF NOT EXISTS tombstones (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id       TEXT NOT NULL REFERENCES projects(id),
    path             TEXT NOT NULL,
    reason           TEXT NOT NULL,
    authorization_id INTEGER REFERENCES action_authorizations(id),
    ts               TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now'))
);
CREATE INDEX IF NOT EXISTS idx_tombstone_project_path ON tombstones(project_id, path);

INSERT OR IGNORE INTO schema_version (version) VALUES (11);
