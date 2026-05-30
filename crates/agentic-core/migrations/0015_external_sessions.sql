-- =============================================================================
--  Migration 0015: external-platform AIBOM sessions (ADR-0053)
-- =============================================================================
--  Records AI-platform sessions (grok.com, gemini.google.com, chatgpt.com,
--  claude.ai, perplexity.ai, etc.) that the author exported and ingested via
--  `agentic external-session import`. The captured session content is stored
--  as a content-addressed blob in `blobs`; this row references it + carries
--  the author attestation that makes the entry the AIBOM anchor for that
--  external interaction (ADR-0053 §5.5 trust model).

CREATE TABLE IF NOT EXISTS external_sessions (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id          TEXT NOT NULL REFERENCES projects(id),
    -- Free-form platform identifier so future platforms don't need a
    -- schema change; validated at insert time by the import command.
    -- Known values: grok, gemini, chatgpt, claude, perplexity, other.
    platform            TEXT NOT NULL,
    -- Platform's own session/conversation identifier if extractable from
    -- the export (e.g. grok share-link slug, gemini conversation_id).
    -- Optional — older formats may not carry one.
    session_id          TEXT,
    -- ISO-8601 UTC timestamp of when the import ran (when the row was
    -- created). Set by the command.
    captured_at         TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now')),
    -- ISO-8601 UTC of the session's start, parsed from the export when
    -- available.
    session_started_at  TEXT,
    session_ended_at    TEXT,
    -- Free-form model identifier from the export, e.g. "grok-4",
    -- "gemini-2.5-pro". Optional.
    model_hint          TEXT,
    -- Number of turns (user + assistant pairs) extracted by the parser.
    -- 0 for stub-parser imports (raw-store only).
    turn_count          INTEGER NOT NULL,
    -- SHA256 of the RAW exported file (the audit anchor — renaming or
    -- modifying the file changes the SHA).
    blob_sha            TEXT NOT NULL REFERENCES blobs(sha256),
    -- SHA256 of the NORMALISED JSON view of the turns (queryable form).
    -- Same blob as raw for stub imports.
    normalised_sha      TEXT NOT NULL REFERENCES blobs(sha256),
    -- Author's one-line statement of provenance; e.g.
    -- "exported 2026-05-30 14:22 UTC from grok.com share-link
    --  https://grok.com/share/abc123 to ~/Downloads/grok-share-abc123.json".
    user_attestation    TEXT NOT NULL,
    -- Optional free-form notes (privacy summary, why this session
    -- matters, link to the thesis paragraph it informed).
    notes               TEXT
);
CREATE INDEX IF NOT EXISTS idx_external_sessions_project
    ON external_sessions(project_id);
CREATE INDEX IF NOT EXISTS idx_external_sessions_platform
    ON external_sessions(project_id, platform);

INSERT OR IGNORE INTO schema_version (version) VALUES (15);
