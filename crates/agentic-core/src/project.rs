//! Project metadata.

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use ulid::Ulid;

use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectKind {
    Thesis,
    SubPaper,
    Standalone,
    PortfolioRoot,
}

impl ProjectKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Thesis => "thesis",
            Self::SubPaper => "sub_paper",
            Self::Standalone => "standalone",
            Self::PortfolioRoot => "portfolio_root",
        }
    }
}

impl std::str::FromStr for ProjectKind {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "thesis" => Ok(Self::Thesis),
            "sub_paper" => Ok(Self::SubPaper),
            "standalone" => Ok(Self::Standalone),
            "portfolio_root" => Ok(Self::PortfolioRoot),
            other => Err(Error::InvalidInput(format!(
                "unknown project kind: {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProjectStatus {
    Active,
    Frozen,
    Archived,
}

impl ProjectStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Frozen => "frozen",
            Self::Archived => "archived",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub kind: ProjectKind,
    pub parent_id: Option<String>,
    pub working_lang: String,
    pub status: ProjectStatus,
    pub head_ref: Option<String>,
    pub metadata_json: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Create a new project; returns its ULID.
pub fn create(
    conn: &Connection,
    name: &str,
    kind: ProjectKind,
    working_lang: &str,
    parent_id: Option<&str>,
) -> Result<String> {
    if !matches!(working_lang, "en" | "de" | "fr" | "it" | "rm" | "hi") {
        return Err(Error::InvalidInput(format!(
            "unsupported language: {working_lang}"
        )));
    }
    let id = Ulid::new().to_string();
    conn.execute(
        "INSERT INTO projects (id, name, kind, parent_id, working_lang) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, name, kind.as_str(), parent_id, working_lang],
    )?;
    Ok(id)
}

/// Fetch by ID.
pub fn get(conn: &Connection, id: &str) -> Result<Project> {
    use std::str::FromStr;
    let row = conn
        .query_row(
            "SELECT id, name, kind, parent_id, working_lang, status, head_ref, metadata_json, created_at, updated_at \
             FROM projects WHERE id = ?1",
            params![id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| Error::ProjectNotFound(id.to_owned()))?;
    Ok(Project {
        id: row.0,
        name: row.1,
        kind: ProjectKind::from_str(&row.2)?,
        parent_id: row.3,
        working_lang: row.4,
        status: match row.5.as_str() {
            "active" => ProjectStatus::Active,
            "frozen" => ProjectStatus::Frozen,
            "archived" => ProjectStatus::Archived,
            other => return Err(Error::Encoding(format!("unknown status: {other}"))),
        },
        head_ref: row.6,
        metadata_json: row.7,
        created_at: row.8,
        updated_at: row.9,
    })
}

/// List all projects.
pub fn list(conn: &Connection) -> Result<Vec<Project>> {
    use std::str::FromStr;
    let mut stmt = conn.prepare(
        "SELECT id, name, kind, parent_id, working_lang, status, head_ref, metadata_json, created_at, updated_at \
         FROM projects ORDER BY created_at DESC",
    )?;
    let rows: Vec<Project> = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, String>(9)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?
        .into_iter()
        .map(|tup| {
            Ok(Project {
                id: tup.0,
                name: tup.1,
                kind: ProjectKind::from_str(&tup.2)?,
                parent_id: tup.3,
                working_lang: tup.4,
                status: match tup.5.as_str() {
                    "active" => ProjectStatus::Active,
                    "frozen" => ProjectStatus::Frozen,
                    "archived" => ProjectStatus::Archived,
                    other => return Err(Error::Encoding(format!("unknown status: {other}"))),
                },
                head_ref: tup.6,
                metadata_json: tup.7,
                created_at: tup.8,
                updated_at: tup.9,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use pretty_assertions::assert_eq;

    #[test]
    fn create_and_get() {
        let conn = open_in_memory().unwrap();
        let id = create(&conn, "My Thesis", ProjectKind::Thesis, "de", None).unwrap();
        let p = get(&conn, &id).unwrap();
        assert_eq!(p.name, "My Thesis");
        assert_eq!(p.kind, ProjectKind::Thesis);
        assert_eq!(p.working_lang, "de");
        assert_eq!(p.status, ProjectStatus::Active);
    }

    #[test]
    fn rejects_unknown_lang() {
        let conn = open_in_memory().unwrap();
        let err = create(&conn, "x", ProjectKind::Standalone, "es", None).unwrap_err();
        assert!(matches!(err, Error::InvalidInput(_)));
    }

    #[test]
    fn portfolio_with_children() {
        let conn = open_in_memory().unwrap();
        let root = create(&conn, "Portfolio", ProjectKind::PortfolioRoot, "de", None).unwrap();
        let child = create(&conn, "Sub", ProjectKind::SubPaper, "en", Some(&root)).unwrap();
        let p = get(&conn, &child).unwrap();
        assert_eq!(p.parent_id.as_deref(), Some(root.as_str()));
        let all = list(&conn).unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn unknown_project() {
        let conn = open_in_memory().unwrap();
        let err = get(&conn, "01JKNONEXISTENT").unwrap_err();
        assert!(matches!(err, Error::ProjectNotFound(_)));
    }
}
