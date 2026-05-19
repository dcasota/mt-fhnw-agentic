-- =============================================================================
--  Migration 0002: wizard_drafts (resumable onboarding state)
-- =============================================================================

CREATE TABLE IF NOT EXISTS wizard_drafts (
    slot        TEXT PRIMARY KEY,
    state_json  TEXT NOT NULL,
    updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

INSERT OR IGNORE INTO schema_version (version) VALUES (2);
