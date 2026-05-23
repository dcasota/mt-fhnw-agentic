-- =============================================================================
--  Migration 0004: PQC signatures + crypto keys (non-repudiation)
-- =============================================================================
--  Implements ADR-0039 (PQC-only cryptography). All non-repudiation signing
--  uses ML-DSA (FIPS 204); classical ciphers (Ed25519/RSA/ECDSA) are forbidden.
--  Private keys live in the OS keychain; only public keys + detached
--  signatures are stored in the database.

CREATE TABLE IF NOT EXISTS crypto_keys (
    key_id      TEXT PRIMARY KEY,         -- sha256(public_key)[..16]
    alg         TEXT NOT NULL,            -- e.g. 'ML-DSA-87'
    public_key  TEXT NOT NULL,            -- hex-encoded public key
    sk_ref      TEXT NOT NULL,            -- OS-keychain account holding the secret key
    signer      TEXT,                     -- human-readable signer identity
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    active      INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0,1))
);

CREATE TABLE IF NOT EXISTS signatures (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    target_kind TEXT NOT NULL,            -- 'commit' | 'audit_report' | 'blob' | 'tree'
    target_id   TEXT NOT NULL,            -- the sha256 / digest being signed
    alg         TEXT NOT NULL,            -- e.g. 'ML-DSA-87'
    key_id      TEXT NOT NULL REFERENCES crypto_keys(key_id),
    signature   TEXT NOT NULL,            -- hex-encoded detached signature
    signer      TEXT,
    signed_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    UNIQUE (target_kind, target_id, key_id)
);
CREATE INDEX IF NOT EXISTS idx_signatures_target ON signatures(target_kind, target_id);

INSERT OR IGNORE INTO schema_version (version) VALUES (4);
