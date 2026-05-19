//! Journal (per-project provenance log). Append-only.

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum HallucinationRisk {
    None,
    Low,
    Medium,
    High,
}

impl HallucinationRisk {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct NewEntry<'a> {
    pub actor:                  &'a str,
    pub triggered_by:           Option<&'a str>,
    pub action_type:            &'a str,
    pub description:            &'a str,
    pub files_affected:         Option<Vec<String>>,
    pub reasoning:              Option<&'a str>,
    pub hallucination_risk:     Option<HallucinationRisk>,
    pub user_approval_required: bool,
    pub user_approval_given:    Option<&'a str>,
    pub commit_sha:             Option<&'a str>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub id:                     i64,
    pub project_id:             String,
    pub entry_no:               i64,
    pub actor:                  String,
    pub triggered_by:           Option<String>,
    pub action_type:            String,
    pub description:            String,
    pub files_affected_json:    Option<String>,
    pub reasoning:              Option<String>,
    pub hallucination_risk:     Option<String>,
    pub user_approval_required: bool,
    pub user_approval_given:    Option<String>,
    pub ts:                     String,
    pub commit_sha:             Option<String>,
}

/// Append a new journal entry; returns the newly assigned `entry_no` (per project).
pub fn append(conn: &Connection, project_id: &str, e: &NewEntry) -> Result<i64> {
    let next_no: i64 = conn.query_row(
        "SELECT IFNULL(MAX(entry_no), 0) + 1 FROM journal_entries WHERE project_id = ?1",
        params![project_id],
        |row| row.get(0),
    )?;
    let files_json = e.files_affected.as_ref().map(|v| serde_json::to_string(v).unwrap_or_default());
    conn.execute(
        "INSERT INTO journal_entries (project_id, entry_no, actor, triggered_by, action_type, description, files_affected_json, reasoning, hallucination_risk, user_approval_required, user_approval_given, commit_sha) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            project_id,
            next_no,
            e.actor,
            e.triggered_by,
            e.action_type,
            e.description,
            files_json,
            e.reasoning,
            e.hallucination_risk.map(HallucinationRisk::as_str),
            i64::from(e.user_approval_required),
            e.user_approval_given,
            e.commit_sha,
        ],
    )?;
    Ok(next_no)
}

/// Fetch the last `limit` entries for a project, newest first.
pub fn last(conn: &Connection, project_id: &str, limit: usize) -> Result<Vec<Entry>> {
    let mut stmt = conn.prepare(
        "SELECT id, project_id, entry_no, actor, triggered_by, action_type, description, files_affected_json, reasoning, hallucination_risk, user_approval_required, user_approval_given, ts, commit_sha \
         FROM journal_entries WHERE project_id = ?1 ORDER BY entry_no DESC LIMIT ?2",
    )?;
    let rows: Vec<Entry> = stmt
        .query_map(params![project_id, limit as i64], |row| {
            let approval: i64 = row.get(10)?;
            Ok(Entry {
                id: row.get(0)?,
                project_id: row.get(1)?,
                entry_no: row.get(2)?,
                actor: row.get(3)?,
                triggered_by: row.get(4)?,
                action_type: row.get(5)?,
                description: row.get(6)?,
                files_affected_json: row.get(7)?,
                reasoning: row.get(8)?,
                hallucination_risk: row.get(9)?,
                user_approval_required: approval != 0,
                user_approval_given: row.get(11)?,
                ts: row.get(12)?,
                commit_sha: row.get(13)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{db::open_in_memory, project::{ProjectKind, create as create_project}};
    use pretty_assertions::assert_eq;

    #[test]
    fn append_monotonic_entry_no() {
        let conn = open_in_memory().unwrap();
        let pid = create_project(&conn, "P", ProjectKind::Thesis, "en", None).unwrap();
        let n1 = append(&conn, &pid, &NewEntry { actor: "u", action_type: "x", description: "a", ..Default::default() }).unwrap();
        let n2 = append(&conn, &pid, &NewEntry { actor: "u", action_type: "x", description: "b", ..Default::default() }).unwrap();
        assert_eq!(n1, 1);
        assert_eq!(n2, 2);
    }

    #[test]
    fn last_returns_newest_first() {
        let conn = open_in_memory().unwrap();
        let pid = create_project(&conn, "P", ProjectKind::Thesis, "en", None).unwrap();
        append(&conn, &pid, &NewEntry { actor: "u", action_type: "x", description: "first", ..Default::default() }).unwrap();
        append(&conn, &pid, &NewEntry { actor: "u", action_type: "x", description: "second", ..Default::default() }).unwrap();
        let entries = last(&conn, &pid, 10).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].description, "second");
        assert_eq!(entries[1].description, "first");
    }

    #[test]
    fn files_affected_roundtrip() {
        let conn = open_in_memory().unwrap();
        let pid = create_project(&conn, "P", ProjectKind::Thesis, "en", None).unwrap();
        append(&conn, &pid, &NewEntry {
            actor: "u",
            action_type: "x",
            description: "edit",
            files_affected: Some(vec!["thesis/ch1.md".into(), "thesis/ch2.md".into()]),
            ..Default::default()
        }).unwrap();
        let entries = last(&conn, &pid, 1).unwrap();
        let v: Vec<String> = serde_json::from_str(entries[0].files_affected_json.as_ref().unwrap()).unwrap();
        assert_eq!(v, vec!["thesis/ch1.md", "thesis/ch2.md"]);
    }
}
