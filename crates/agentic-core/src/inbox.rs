//! Inbox lifecycle — the DB-native port of the Scramblings inbox "meccano".
//!
//! Scramblings encoded inbox state by *file location* (`inbox/` →
//! `iterations/NNN/archive/`) with a move-not-copy invariant ("empty inbox =
//! done") and `text[:80]` lexical dedup. Here state is explicit
//! (`queued → ranked → justified → accepted → archived | skipped`), the content
//! blob in the store is the permanent archive (so retirement deletes only the
//! disk copy, never the content), and dedup is exact (SHA-256, built-in) plus
//! semantic (embedding cosine) — more accurate than lexical prefix matching.

use std::collections::HashMap;

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::{Error, Result, content::blob, embeddings, worktree};

pub const PREFIX: &str = "inbox/";
/// Default cosine threshold for semantic near-duplicate flagging (SemHash-style).
pub const NEAR_DUP_THRESHOLD: f32 = 0.90;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboxItem {
    pub id: i64,
    pub path: String,
    pub content_sha: Option<String>,
    pub state: String,
    pub score: Option<f64>,
    pub placement: Option<String>,
    pub justification: Option<String>,
    pub dup_of: Option<String>,
    pub accepted_by: Option<String>,
    pub entered_at: String,
    pub updated_at: String,
}

/// Register every inbox blob in the content-store HEAD as a `queued` item
/// (idempotent — existing rows keep their state, only sha/dup_of refresh).
/// Exact-duplicate detection: if an inbox blob's SHA also appears at another
/// path in the working tree, `dup_of` records that path.
pub fn register(conn: &Connection, project_id: &str) -> Result<usize> {
    let all = worktree::list(conn, project_id, "")?; // (path, sha), sorted
    let mut by_sha: HashMap<String, Vec<String>> = HashMap::new();
    for (p, s) in &all {
        by_sha.entry(s.clone()).or_default().push(p.clone());
    }
    let mut added = 0usize;
    for (path, sha) in all.iter().filter(|(p, _)| p.starts_with(PREFIX)) {
        let dup = by_sha
            .get(sha)
            .and_then(|paths| paths.iter().find(|p| *p != path).cloned());
        let existing: Option<i64> = conn
            .query_row(
                "SELECT id FROM inbox_items WHERE project_id = ?1 AND path = ?2",
                params![project_id, path],
                |r| r.get(0),
            )
            .optional()?;
        if existing.is_none() {
            conn.execute(
                "INSERT INTO inbox_items (project_id, path, content_sha, dup_of, state) \
                 VALUES (?1, ?2, ?3, ?4, 'queued')",
                params![project_id, path, sha, dup],
            )?;
            added += 1;
        } else {
            conn.execute(
                "UPDATE inbox_items SET content_sha = ?3, dup_of = ?4, \
                 updated_at = strftime('%Y-%m-%dT%H:%M:%SZ','now') \
                 WHERE project_id = ?1 AND path = ?2",
                params![project_id, path, sha, dup],
            )?;
        }
    }
    Ok(added)
}

pub fn list(conn: &Connection, project_id: &str) -> Result<Vec<InboxItem>> {
    let mut stmt = conn.prepare(
        "SELECT id, path, content_sha, state, score, placement, justification, dup_of, \
                accepted_by, entered_at, updated_at \
         FROM inbox_items WHERE project_id = ?1 ORDER BY path",
    )?;
    let rows = stmt
        .query_map(params![project_id], |r| {
            Ok(InboxItem {
                id: r.get(0)?,
                path: r.get(1)?,
                content_sha: r.get(2)?,
                state: r.get(3)?,
                score: r.get(4)?,
                placement: r.get(5)?,
                justification: r.get(6)?,
                dup_of: r.get(7)?,
                accepted_by: r.get(8)?,
                entered_at: r.get(9)?,
                updated_at: r.get(10)?,
            })
        })?
        .collect::<std::result::Result<_, _>>()?;
    Ok(rows)
}

/// Mark an item accepted (acceptance level reached). `hitl` records whether the
/// decision was human-confirmed or autonomous.
pub fn accept(
    conn: &Connection,
    project_id: &str,
    path: &str,
    score: Option<f64>,
    placement: Option<&str>,
    justification: Option<&str>,
    hitl: bool,
) -> Result<()> {
    let by = if hitl { "hitl" } else { "auto" };
    let updated = conn.execute(
        "UPDATE inbox_items SET state='accepted', \
             score=COALESCE(?3,score), placement=COALESCE(?4,placement), \
             justification=COALESCE(?5,justification), accepted_by=?6, \
             updated_at=strftime('%Y-%m-%dT%H:%M:%SZ','now') \
         WHERE project_id=?1 AND path=?2",
        params![project_id, path, score, placement, justification, by],
    )?;
    if updated == 0 {
        return Err(Error::InvalidInput(format!(
            "inbox item not registered: {path} (run `inbox register`)"
        )));
    }
    Ok(())
}

/// Mark an item skipped (e.g. infrastructure/non-input like a folder README).
pub fn skip(conn: &Connection, project_id: &str, path: &str) -> Result<()> {
    let updated = conn.execute(
        "UPDATE inbox_items SET state='skipped', updated_at=strftime('%Y-%m-%dT%H:%M:%SZ','now') \
         WHERE project_id=?1 AND path=?2",
        params![project_id, path],
    )?;
    if updated == 0 {
        return Err(Error::InvalidInput(format!("inbox item not registered: {path}")));
    }
    Ok(())
}

/// Precondition-checked retire. The content blob MUST exist in the DB (so the
/// item is recoverable) and the item must be accepted/justified/skipped. Sets
/// state to 'archived' and returns the content SHA. The caller deletes the
/// on-disk file and journals the move (the DB blob is the permanent archive).
pub fn retire(conn: &Connection, project_id: &str, path: &str) -> Result<String> {
    let row = conn
        .query_row(
            "SELECT content_sha, state FROM inbox_items WHERE project_id=?1 AND path=?2",
            params![project_id, path],
            |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, String>(1)?)),
        )
        .optional()?
        .ok_or_else(|| {
            Error::InvalidInput(format!(
                "inbox item not registered: {path} (run `inbox register`)"
            ))
        })?;
    let (sha, state) = row;
    let sha = sha.ok_or_else(|| {
        Error::InvalidInput(format!("inbox item {path} has no content_sha; cannot retire safely"))
    })?;
    // Blob must exist — the retire is only safe if the content is recoverable.
    blob::get_blob(conn, &sha).map_err(|_| {
        Error::InvalidInput(format!(
            "blob {sha} for {path} is missing from the content store — refusing to retire"
        ))
    })?;
    if !matches!(state.as_str(), "accepted" | "justified" | "skipped" | "archived") {
        return Err(Error::InvalidInput(format!(
            "inbox item {path} is '{state}'; accept/justify/skip it before retiring"
        )));
    }
    conn.execute(
        "UPDATE inbox_items SET state='archived', updated_at=strftime('%Y-%m-%dT%H:%M:%SZ','now') \
         WHERE project_id=?1 AND path=?2",
        params![project_id, path],
    )?;
    Ok(sha)
}

/// Semantic near-duplicate: the best cosine match (≥ `threshold`) for `sha`
/// among all other whole-document embeddings of `model`. Empty if `model` has no
/// embedding for `sha` (run `agentic embed` first) — exact dedup via SHA still
/// works without embeddings.
pub fn nearest_duplicate(
    conn: &Connection,
    model: &str,
    sha: &str,
    threshold: f32,
) -> Result<Option<(String, f32)>> {
    let Some(target) = embeddings::get_embedding(conn, sha, model, 0)? else {
        return Ok(None);
    };
    let mut best: Option<(String, f32)> = None;
    for e in embeddings::list_by_model(conn, model)? {
        if e.blob_sha == sha || e.chunk_idx != 0 {
            continue;
        }
        let c = embeddings::cosine(&target.vector, &e.vector);
        if c >= threshold && best.as_ref().is_none_or(|(_, bc)| c > *bc) {
            best = Some((e.blob_sha.clone(), c));
        }
    }
    Ok(best)
}
