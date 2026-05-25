-- =============================================================================
--  Migration 0006: orchestration sessions (in-tool replacement for the external
--  PowerShell launcher — launch_session.ps1 + run_*_all.ps1 + sessions/manifest.json)
-- =============================================================================
--  Each row is one bounded autonomous sub-session that shells out to the `claude`
--  CLI with a fixed budget and a guard system-prompt. The lifecycle state lives
--  in the DB (the source of truth) instead of a manifest.json file; transcripts
--  are tee'd to sessions/<id>.jsonl on disk for human inspection.

CREATE TABLE IF NOT EXISTS orchestration_sessions (
    id              TEXT PRIMARY KEY,        -- "<yyyyMMdd-HHmmss>-<short id>" (e.g. 20260525-141200-dim01)
    project_id      TEXT NOT NULL,
    task            TEXT NOT NULL,
    budget_usd      REAL NOT NULL DEFAULT 3.0,
    model           TEXT,                    -- optional model override
    status          TEXT NOT NULL DEFAULT 'pending'
                    CHECK (status IN ('pending','running','done','failed','ratelimited')),
    transcript_path TEXT,
    exit_summary    TEXT,
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now')),
    closed_at       TEXT
);
CREATE INDEX IF NOT EXISTS idx_orchestration_project ON orchestration_sessions(project_id, status);

INSERT OR IGNORE INTO schema_version (version) VALUES (6);
