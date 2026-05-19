//! Material Passport — append-only per-project section log.
//!
//! See ADR-0016 for the policy. Each call to [`append`] adds one row to
//! `passport_entries`; `replaces` lets a new row supersede an older one
//! while still keeping it in history.

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Section {
    LiteratureCorpus,
    ClaimIntentManifest,
    ClaimAuditResults,
    TemporalAuditResults,
    Timeline,
    ResetLedger,
    ComplianceReports,
}

impl Section {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LiteratureCorpus => "literature_corpus",
            Self::ClaimIntentManifest => "claim_intent_manifest",
            Self::ClaimAuditResults => "claim_audit_results",
            Self::TemporalAuditResults => "temporal_audit_results",
            Self::Timeline => "timeline",
            Self::ResetLedger => "reset_ledger",
            Self::ComplianceReports => "compliance_reports",
        }
    }
}

impl std::str::FromStr for Section {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "literature_corpus" => Ok(Self::LiteratureCorpus),
            "claim_intent_manifest" => Ok(Self::ClaimIntentManifest),
            "claim_audit_results" => Ok(Self::ClaimAuditResults),
            "temporal_audit_results" => Ok(Self::TemporalAuditResults),
            "timeline" => Ok(Self::Timeline),
            "reset_ledger" => Ok(Self::ResetLedger),
            "compliance_reports" => Ok(Self::ComplianceReports),
            other => Err(Error::InvalidInput(format!("unknown passport section: {other}"))),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub id:         i64,
    pub project_id: String,
    pub section:    String,
    pub payload_json: String,
    pub added_at:   String,
    pub commit_sha: Option<String>,
    pub replaces:   Option<i64>,
}

/// Append a payload to the given section. `payload_json` must be valid JSON.
pub fn append(
    conn: &Connection,
    project_id: &str,
    section: Section,
    payload_json: &str,
    commit_sha: Option<&str>,
    replaces: Option<i64>,
) -> Result<i64> {
    // Validate JSON before persisting (cheap parse).
    serde_json::from_str::<serde_json::Value>(payload_json)?;
    let mut stmt = conn.prepare(
        "INSERT INTO passport_entries (project_id, section, payload_json, commit_sha, replaces) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;
    stmt.execute(params![project_id, section.as_str(), payload_json, commit_sha, replaces])?;
    Ok(conn.last_insert_rowid())
}

/// Read all (non-superseded) entries for a section. An entry is "current" if
/// no other entry replaces it.
pub fn current(conn: &Connection, project_id: &str, section: Section) -> Result<Vec<Entry>> {
    let mut stmt = conn.prepare(
        "SELECT p.id, p.project_id, p.section, p.payload_json, p.added_at, p.commit_sha, p.replaces \
         FROM passport_entries p \
         WHERE p.project_id = ?1 AND p.section = ?2 \
           AND NOT EXISTS (SELECT 1 FROM passport_entries q WHERE q.replaces = p.id) \
         ORDER BY p.id",
    )?;
    let rows: Vec<Entry> = stmt
        .query_map(params![project_id, section.as_str()], |row| {
            Ok(Entry {
                id: row.get(0)?,
                project_id: row.get(1)?,
                section: row.get(2)?,
                payload_json: row.get(3)?,
                added_at: row.get(4)?,
                commit_sha: row.get(5)?,
                replaces: row.get(6)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Validate the passport's structural invariants (append-only-ness, JSON validity).
pub fn validate(conn: &Connection, project_id: &str) -> Result<ValidationReport> {
    let mut stmt = conn.prepare(
        "SELECT id, section, payload_json FROM passport_entries WHERE project_id = ?1",
    )?;
    let mut total = 0usize;
    let mut json_errors = Vec::new();
    let rows = stmt.query_map(params![project_id], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    for row in rows {
        let (id, section, payload_json) = row?;
        total += 1;
        if serde_json::from_str::<serde_json::Value>(&payload_json).is_err() {
            json_errors.push((id, section));
        }
    }
    Ok(ValidationReport {
        total_entries: total,
        json_errors,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationReport {
    pub total_entries: usize,
    pub json_errors:   Vec<(i64, String)>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{db::open_in_memory, project::{ProjectKind, create as create_project}};
    use pretty_assertions::assert_eq;

    #[test]
    fn append_and_current() {
        let conn = open_in_memory().unwrap();
        let pid = create_project(&conn, "P", ProjectKind::Thesis, "en", None).unwrap();
        let id1 = append(&conn, &pid, Section::Timeline, r#"{"event": "start"}"#, None, None).unwrap();
        let id2 = append(&conn, &pid, Section::Timeline, r#"{"event": "mid"}"#, None, None).unwrap();
        assert_ne!(id1, id2);
        let entries = current(&conn, &pid, Section::Timeline).unwrap();
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn replaces_makes_the_old_one_not_current() {
        let conn = open_in_memory().unwrap();
        let pid = create_project(&conn, "P", ProjectKind::Thesis, "en", None).unwrap();
        let old = append(&conn, &pid, Section::Timeline, r#"{"event": "v1"}"#, None, None).unwrap();
        let _new = append(&conn, &pid, Section::Timeline, r#"{"event": "v2"}"#, None, Some(old)).unwrap();
        let entries = current(&conn, &pid, Section::Timeline).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].payload_json.contains("\"v2\""));
    }

    #[test]
    fn rejects_invalid_json() {
        let conn = open_in_memory().unwrap();
        let pid = create_project(&conn, "P", ProjectKind::Thesis, "en", None).unwrap();
        let err = append(&conn, &pid, Section::Timeline, "not json", None, None).unwrap_err();
        assert!(matches!(err, Error::Json(_)));
    }

    #[test]
    fn validate_reports_total() {
        let conn = open_in_memory().unwrap();
        let pid = create_project(&conn, "P", ProjectKind::Thesis, "en", None).unwrap();
        append(&conn, &pid, Section::Timeline, r#"{"a":1}"#, None, None).unwrap();
        append(&conn, &pid, Section::Timeline, r#"{"b":2}"#, None, None).unwrap();
        let report = validate(&conn, &pid).unwrap();
        assert_eq!(report.total_entries, 2);
        assert!(report.json_errors.is_empty());
    }
}
