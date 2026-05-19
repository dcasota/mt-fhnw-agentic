-- =============================================================================
--  Migration 0003: embeddings (vectors keyed by blob + model)
-- =============================================================================
--  One row per (blob, model, chunk_idx). For now we embed whole documents
--  (chunk_idx=0, chunk_text=full body). Chunking is left for a later phase.
--
--  Vector bytes are stored as little-endian f32 packed end-to-end.

CREATE TABLE IF NOT EXISTS embeddings (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    blob_sha    TEXT NOT NULL REFERENCES blobs(sha256),
    model       TEXT NOT NULL,
    chunk_idx   INTEGER NOT NULL DEFAULT 0,
    chunk_text  TEXT NOT NULL,
    dims        INTEGER NOT NULL,
    vector      BLOB NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    UNIQUE (blob_sha, model, chunk_idx)
);
CREATE INDEX IF NOT EXISTS idx_embeddings_model ON embeddings(model);
CREATE INDEX IF NOT EXISTS idx_embeddings_blob  ON embeddings(blob_sha);

INSERT OR IGNORE INTO schema_version (version) VALUES (3);
