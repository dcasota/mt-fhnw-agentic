-- =============================================================================
--  Migration 0013: cascade step checkpoints (ADR-0047 R2/R3/R9)
-- =============================================================================
--  Records which expensive cascade steps (regenerate / merge / build) have
--  completed for a given input fingerprint, so `cascade run --resume` can skip
--  them when the content store is unchanged (input-delta gating + resume).
--  `--force-full` clears the rows; a changed fingerprint naturally invalidates
--  them (no match).

CREATE TABLE IF NOT EXISTS cascade_steps (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id   TEXT NOT NULL REFERENCES projects(id),
    fingerprint  TEXT NOT NULL,
    step_label   TEXT NOT NULL,
    ts           TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now'))
);
CREATE INDEX IF NOT EXISTS idx_cascade_steps ON cascade_steps(project_id, fingerprint);

INSERT OR IGNORE INTO schema_version (version) VALUES (13);
