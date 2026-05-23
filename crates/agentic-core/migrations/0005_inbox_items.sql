-- =============================================================================
--  Migration 0005: inbox lifecycle (explicit state, replacing "location = state")
-- =============================================================================
--  Ports the Scramblings inbox "meccano" to the DB: instead of encoding state by
--  file location (inbox/ -> iterations/NNN/archive/), each inbox item carries an
--  explicit lifecycle state. The content blob is the permanent archive (the DB is
--  the source of truth), so retirement = mark 'archived' + delete the disk copy;
--  nothing is destroyed. Dedup is exact (SHA-256, built-in) + semantic
--  (embedding cosine) rather than the original lexical text[:80] method.

CREATE TABLE IF NOT EXISTS inbox_items (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id    TEXT NOT NULL REFERENCES projects(id),
    path          TEXT NOT NULL,            -- e.g. inbox/briefing-decks-2026.md
    content_sha   TEXT,                     -- blob SHA in the content store
    state         TEXT NOT NULL DEFAULT 'queued'
                  CHECK (state IN ('queued','ranked','justified','accepted','archived','skipped')),
    score         REAL,                     -- ranking score (0..1)
    placement     TEXT,                     -- thesis_main | thesis_appendix | lowrankings
    justification TEXT,                     -- passport id / ranking-deliverable path / note
    dup_of        TEXT,                     -- another path sharing this content (exact dup)
    accepted_by   TEXT,                     -- 'auto' | 'hitl'
    entered_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    updated_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    UNIQUE (project_id, path)
);
CREATE INDEX IF NOT EXISTS idx_inbox_state ON inbox_items(project_id, state);

INSERT OR IGNORE INTO schema_version (version) VALUES (5);
