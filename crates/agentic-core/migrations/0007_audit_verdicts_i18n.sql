-- =============================================================================
--  Migration 0007: allow checkpoint 'i18n' in audit_verdicts
-- =============================================================================
--  ADR-0041 (every `agentic check` records its verdict): the new
--  `agentic check i18n` gate (language-interoperability coverage of engine
--  chrome) records checkpoint "i18n". SQLite cannot ALTER a CHECK constraint,
--  so the table is rebuilt with the expanded allow-list and the existing rows
--  are copied across.

PRAGMA foreign_keys = OFF;

CREATE TABLE audit_verdicts_new (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id      TEXT NOT NULL REFERENCES projects(id),
    iteration       INTEGER,
    checkpoint      TEXT NOT NULL CHECK (checkpoint IN (
        'pre_iteration','post_iteration','pre_commit',
        'stage_25_integrity','stage_45_integrity','submission_freeze',
        'raise','prisma_trace','claim_audit','writing_quality','temporal','compliance_consolidated',
        -- gate checkers (ADR-0041: every `agentic check` records its verdict)
        'self','tree','deliverable','citation_tracker','contamination','bibliography','aibom','docs',
        'facts_integrity','i18n'
    )),
    verdict         TEXT NOT NULL CHECK (verdict IN ('PASS','WARN','FAIL')),
    findings_json   TEXT,
    blocking_finding_ids_json TEXT,
    remediation_actions_json  TEXT,
    override_round  INTEGER NOT NULL DEFAULT 0 CHECK (override_round BETWEEN 0 AND 3),
    sidecar_ref     TEXT,
    ts              TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

INSERT INTO audit_verdicts_new
    SELECT id, project_id, iteration, checkpoint, verdict, findings_json,
           blocking_finding_ids_json, remediation_actions_json, override_round,
           sidecar_ref, ts
    FROM audit_verdicts;

DROP TABLE audit_verdicts;
ALTER TABLE audit_verdicts_new RENAME TO audit_verdicts;
CREATE INDEX IF NOT EXISTS idx_verdicts_project_iter ON audit_verdicts(project_id, iteration);

PRAGMA foreign_keys = ON;

INSERT OR IGNORE INTO schema_version (version) VALUES (7);
