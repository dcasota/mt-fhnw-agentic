-- =============================================================================
--  agentic-core schema, migration 0001
--  See plan section 3 ("SQLite schema -- Git-in-SQLite").
--  All tables are append-only except `refs` (branches/tags move) and `projects.status`.
-- =============================================================================

PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;

-- ===== Content-addressed storage ============================================

CREATE TABLE IF NOT EXISTS blobs (
    sha256       TEXT PRIMARY KEY,
    mime         TEXT NOT NULL,
    encoding     TEXT NOT NULL CHECK (encoding IN ('utf-8', 'base64')),
    content      BLOB NOT NULL,
    size_bytes   INTEGER NOT NULL,
    lang         TEXT CHECK (lang IS NULL OR lang IN ('en','de','fr','it','rm','hi')),
    created_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);
CREATE INDEX IF NOT EXISTS idx_blobs_lang ON blobs(lang);
CREATE INDEX IF NOT EXISTS idx_blobs_mime ON blobs(mime);

CREATE TABLE IF NOT EXISTS trees (
    sha256        TEXT PRIMARY KEY,
    -- JSON array of {"name": "...", "kind": "blob"|"tree", "target": "<sha256>", "mode": "..."}
    entries_json  TEXT NOT NULL,
    created_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE TABLE IF NOT EXISTS commits (
    sha256        TEXT PRIMARY KEY,
    tree          TEXT NOT NULL REFERENCES trees(sha256),
    parent        TEXT REFERENCES commits(sha256),
    parent_2      TEXT REFERENCES commits(sha256),       -- for merges
    author        TEXT NOT NULL,
    actor_kind    TEXT NOT NULL CHECK (actor_kind IN ('human', 'ai', 'hook', 'system')),
    actor_detail  TEXT,                                  -- e.g. 'claude-opus-4-7' or username
    iteration     INTEGER,
    message       TEXT NOT NULL,
    timestamp     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);
CREATE INDEX IF NOT EXISTS idx_commits_iteration ON commits(iteration);
CREATE INDEX IF NOT EXISTS idx_commits_timestamp ON commits(timestamp);

CREATE TABLE IF NOT EXISTS refs (
    name          TEXT PRIMARY KEY,
    kind          TEXT NOT NULL CHECK (kind IN ('branch', 'tag')),
    commit_sha    TEXT NOT NULL REFERENCES commits(sha256),
    updated_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

-- ===== Projects ==============================================================

CREATE TABLE IF NOT EXISTS projects (
    id            TEXT PRIMARY KEY,        -- ULID
    name          TEXT NOT NULL,
    kind          TEXT NOT NULL CHECK (kind IN ('thesis', 'sub_paper', 'standalone', 'portfolio_root')),
    parent_id     TEXT REFERENCES projects(id),
    working_lang  TEXT NOT NULL CHECK (working_lang IN ('en','de','fr','it','rm','hi')),
    status        TEXT NOT NULL CHECK (status IN ('active','frozen','archived')) DEFAULT 'active',
    head_ref      TEXT REFERENCES refs(name),
    metadata_json TEXT,                    -- title, supervisors, institution, track, etc.
    created_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    updated_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);
CREATE INDEX IF NOT EXISTS idx_projects_parent ON projects(parent_id);
CREATE INDEX IF NOT EXISTS idx_projects_status ON projects(status);

-- ===== Passport (cross-session append-only state) ============================

CREATE TABLE IF NOT EXISTS passport_entries (
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
        'verified_facts'
    )),
    payload_json  TEXT NOT NULL,
    added_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    commit_sha    TEXT REFERENCES commits(sha256),
    replaces      INTEGER REFERENCES passport_entries(id)
);
CREATE INDEX IF NOT EXISTS idx_passport_project ON passport_entries(project_id, section);

-- ===== Journal ==============================================================

CREATE TABLE IF NOT EXISTS journal_entries (
    id                     INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id             TEXT NOT NULL REFERENCES projects(id),
    entry_no               INTEGER NOT NULL,
    actor                  TEXT NOT NULL,
    triggered_by           TEXT,
    action_type            TEXT NOT NULL,
    description            TEXT NOT NULL,
    files_affected_json    TEXT,
    reasoning              TEXT,
    hallucination_risk     TEXT CHECK (hallucination_risk IS NULL OR hallucination_risk IN ('None','Low','Medium','High')),
    user_approval_required INTEGER NOT NULL DEFAULT 0 CHECK (user_approval_required IN (0,1)),
    user_approval_given    TEXT,
    ts                     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    commit_sha             TEXT REFERENCES commits(sha256),
    UNIQUE (project_id, entry_no)
);
CREATE INDEX IF NOT EXISTS idx_journal_project ON journal_entries(project_id);

-- ===== Audit (replaces iterations/iteration_N.audit.jsonl + sidecars + verdicts)

CREATE TABLE IF NOT EXISTS audit_rows (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id    TEXT NOT NULL REFERENCES projects(id),
    iteration     INTEGER,
    ts            TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    agent         TEXT NOT NULL,
    action        TEXT NOT NULL,
    target        TEXT,
    result        TEXT NOT NULL CHECK (result IN ('pass','warn','fail','ok','info')),
    sidecar_json  TEXT,
    tags_json     TEXT,
    duration_ms   INTEGER,
    tokens_used   INTEGER,
    model         TEXT
);
CREATE INDEX IF NOT EXISTS idx_audit_project_iter ON audit_rows(project_id, iteration);
CREATE INDEX IF NOT EXISTS idx_audit_agent ON audit_rows(agent);

CREATE TABLE IF NOT EXISTS audit_verdicts (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id      TEXT NOT NULL REFERENCES projects(id),
    iteration       INTEGER,
    checkpoint      TEXT NOT NULL CHECK (checkpoint IN (
        'pre_iteration','post_iteration','pre_commit',
        'stage_25_integrity','stage_45_integrity','submission_freeze',
        'raise','prisma_trace','claim_audit','writing_quality','temporal','compliance_consolidated',
        -- gate checkers (ADR-0041: every `agentic check` records its verdict)
        'self','tree','deliverable','citation_tracker','contamination','bibliography','aibom','docs'
    )),
    verdict         TEXT NOT NULL CHECK (verdict IN ('PASS','WARN','FAIL')),
    findings_json   TEXT,
    blocking_finding_ids_json TEXT,
    remediation_actions_json  TEXT,
    override_round  INTEGER NOT NULL DEFAULT 0 CHECK (override_round BETWEEN 0 AND 3),
    sidecar_ref     TEXT,
    ts              TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);
CREATE INDEX IF NOT EXISTS idx_verdicts_project_iter ON audit_verdicts(project_id, iteration);

-- ===== Schemas (JSON Schemas live in DB, not in files) =======================

CREATE TABLE IF NOT EXISTS schemas (
    name          TEXT PRIMARY KEY,
    version       TEXT NOT NULL,
    json_schema   TEXT NOT NULL,
    description   TEXT,
    updated_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

-- ===== Protocols (procedural docs live in DB) ================================

CREATE TABLE IF NOT EXISTS protocols (
    name          TEXT PRIMARY KEY,
    version       TEXT NOT NULL,
    body_blob_sha TEXT NOT NULL REFERENCES blobs(sha256),
    related_adrs_json  TEXT,
    related_tools_json TEXT,
    updated_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

-- ===== ADRs (decision records live in DB) ====================================

CREATE TABLE IF NOT EXISTS adrs (
    number        INTEGER PRIMARY KEY,
    title         TEXT NOT NULL,
    status        TEXT NOT NULL CHECK (status IN ('Proposed','Accepted','Deprecated','Superseded')),
    superseded_by INTEGER REFERENCES adrs(number),
    body_blob_sha TEXT NOT NULL REFERENCES blobs(sha256),
    date          TEXT NOT NULL,
    updated_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

-- ===== i18n strings (UI + document) ==========================================

CREATE TABLE IF NOT EXISTS i18n_strings (
    key           TEXT NOT NULL,
    lang          TEXT NOT NULL CHECK (lang IN ('en','de','fr','it','rm','hi')),
    value         TEXT NOT NULL,
    context       TEXT CHECK (context IN ('ui','document','declaration','wizard')),
    updated_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    PRIMARY KEY (key, lang)
);
CREATE INDEX IF NOT EXISTS idx_i18n_lang ON i18n_strings(lang, context);

-- ===== API cache (Crossref / OpenAlex / S2 / LLM responses) ==================

CREATE TABLE IF NOT EXISTS api_cache (
    cache_key     TEXT PRIMARY KEY,
    service       TEXT NOT NULL,
    endpoint      TEXT,
    request_json  TEXT,
    response_json TEXT NOT NULL,
    fetched_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    ttl_seconds   INTEGER,
    bytes         INTEGER
);
CREATE INDEX IF NOT EXISTS idx_cache_service ON api_cache(service);

-- ===== Sprint contracts ======================================================

CREATE TABLE IF NOT EXISTS sprint_contracts (
    contract_id        TEXT PRIMARY KEY,
    project_id         TEXT NOT NULL REFERENCES projects(id),
    agent              TEXT NOT NULL,
    phase              TEXT NOT NULL CHECK (phase IN (
        'phase_1_blind_commitment','phase_2_visible_application','dissent'
    )),
    scoring_plan_json  TEXT,
    applied_results_json TEXT,
    dissent_json       TEXT,
    linked_phase_1_id  TEXT REFERENCES sprint_contracts(contract_id),
    created_at         TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);
CREATE INDEX IF NOT EXISTS idx_sprint_project ON sprint_contracts(project_id);

-- ===== Full-text search (FTS5 virtual table) =================================

CREATE VIRTUAL TABLE IF NOT EXISTS fts USING fts5(
    blob_sha,
    project_id UNINDEXED,
    lang UNINDEXED,
    title,
    body,
    tokenize = 'unicode61 remove_diacritics 2'
);

-- ===== Schema version pin (used by the migration runner) =====================

CREATE TABLE IF NOT EXISTS schema_version (
    version       INTEGER PRIMARY KEY,
    applied_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

INSERT OR IGNORE INTO schema_version (version) VALUES (1);
