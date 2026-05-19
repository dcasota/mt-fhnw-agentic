//! Vector embeddings DAO.
//!
//! Stores `(blob_sha, model, chunk_idx) → f32 vector`. One row per chunk;
//! whole-document embeddings use `chunk_idx = 0`. Vectors are packed as
//! little-endian f32 sequences in the `vector` BLOB column.

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::{Error, Result};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Embedding {
    pub id: i64,
    pub blob_sha: String,
    pub model: String,
    pub chunk_idx: i64,
    pub chunk_text: String,
    pub dims: i64,
    pub vector: Vec<f32>,
    pub created_at: String,
}

/// Insert (or replace) an embedding for the given blob + model + chunk.
pub fn put_embedding(
    conn: &Connection,
    blob_sha: &str,
    model: &str,
    chunk_idx: i64,
    chunk_text: &str,
    vector: &[f32],
) -> Result<i64> {
    let bytes = vector_to_bytes(vector);
    conn.execute(
        "INSERT INTO embeddings (blob_sha, model, chunk_idx, chunk_text, dims, vector)
              VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(blob_sha, model, chunk_idx) DO UPDATE SET
              chunk_text = excluded.chunk_text,
              dims       = excluded.dims,
              vector     = excluded.vector,
              created_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')",
        params![
            blob_sha,
            model,
            chunk_idx,
            chunk_text,
            vector.len() as i64,
            bytes
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Fetch one embedding by `(blob_sha, model, chunk_idx)`.
pub fn get_embedding(
    conn: &Connection,
    blob_sha: &str,
    model: &str,
    chunk_idx: i64,
) -> Result<Option<Embedding>> {
    let row = conn
        .query_row(
            "SELECT id, blob_sha, model, chunk_idx, chunk_text, dims, vector, created_at
             FROM embeddings
             WHERE blob_sha = ?1 AND model = ?2 AND chunk_idx = ?3",
            params![blob_sha, model, chunk_idx],
            row_to_embedding,
        )
        .optional()?;
    Ok(row)
}

/// All embeddings for a given model, ordered by `blob_sha, chunk_idx`.
pub fn list_by_model(conn: &Connection, model: &str) -> Result<Vec<Embedding>> {
    let mut stmt = conn.prepare(
        "SELECT id, blob_sha, model, chunk_idx, chunk_text, dims, vector, created_at
         FROM embeddings
         WHERE model = ?1
         ORDER BY blob_sha, chunk_idx",
    )?;
    let rows: Result<Vec<Embedding>> = stmt
        .query_map(params![model], row_to_embedding)?
        .map(|r| r.map_err(Error::from))
        .collect();
    rows
}

/// Cosine similarity between two equal-length vectors.
/// Returns `0.0` when either norm is zero (avoids NaN).
#[must_use]
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    let denom = na.sqrt() * nb.sqrt();
    if denom == 0.0 { 0.0 } else { dot / denom }
}

fn row_to_embedding(row: &rusqlite::Row<'_>) -> rusqlite::Result<Embedding> {
    let bytes: Vec<u8> = row.get(6)?;
    let dims: i64 = row.get(5)?;
    Ok(Embedding {
        id: row.get(0)?,
        blob_sha: row.get(1)?,
        model: row.get(2)?,
        chunk_idx: row.get(3)?,
        chunk_text: row.get(4)?,
        dims,
        vector: bytes_to_vector(&bytes, dims as usize),
        created_at: row.get(7)?,
    })
}

fn vector_to_bytes(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for f in v {
        out.extend_from_slice(&f.to_le_bytes());
    }
    out
}

fn bytes_to_vector(bytes: &[u8], dims: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(dims);
    for chunk in bytes.chunks_exact(4) {
        let arr: [u8; 4] = chunk.try_into().unwrap_or([0; 4]);
        out.push(f32::from_le_bytes(arr));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::blob::put_blob;
    use crate::db::open_in_memory;

    #[test]
    fn round_trip_with_replace() {
        let conn = open_in_memory().unwrap();
        let sha = put_blob(&conn, b"hello", "text/markdown", Some("en")).unwrap();
        let v = vec![0.1, -0.2, 0.3, 0.4];
        put_embedding(&conn, &sha, "model-x", 0, "hello", &v).unwrap();
        let loaded = get_embedding(&conn, &sha, "model-x", 0).unwrap().unwrap();
        assert_eq!(loaded.dims, 4);
        for (a, b) in loaded.vector.iter().zip(v.iter()) {
            assert!((a - b).abs() < 1e-6);
        }

        // Replace overwrites in place.
        let v2 = vec![1.0, 2.0, 3.0, 4.0];
        put_embedding(&conn, &sha, "model-x", 0, "hello", &v2).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM embeddings", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
        let loaded = get_embedding(&conn, &sha, "model-x", 0).unwrap().unwrap();
        assert!((loaded.vector[0] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn list_by_model_orders_by_blob_then_chunk() {
        let conn = open_in_memory().unwrap();
        let a = put_blob(&conn, b"a", "text/plain", None).unwrap();
        let b = put_blob(&conn, b"b", "text/plain", None).unwrap();
        // Insert chunks in non-sorted order to exercise the ORDER BY.
        put_embedding(&conn, &b, "m", 1, "b1", &[1.0]).unwrap();
        put_embedding(&conn, &a, "m", 1, "a1", &[2.0]).unwrap();
        put_embedding(&conn, &b, "m", 0, "b0", &[2.5]).unwrap();
        put_embedding(&conn, &a, "m", 0, "a0", &[3.0]).unwrap();
        let rows = list_by_model(&conn, "m").unwrap();
        assert_eq!(rows.len(), 4);
        // Lex sort by blob_sha (hex of SHA-256), then by chunk_idx.
        let (lo, hi) = if a < b { (&a, &b) } else { (&b, &a) };
        assert_eq!(&rows[0].blob_sha, lo);
        assert_eq!(rows[0].chunk_idx, 0);
        assert_eq!(&rows[1].blob_sha, lo);
        assert_eq!(rows[1].chunk_idx, 1);
        assert_eq!(&rows[2].blob_sha, hi);
        assert_eq!(rows[2].chunk_idx, 0);
        assert_eq!(&rows[3].blob_sha, hi);
        assert_eq!(rows[3].chunk_idx, 1);
    }

    #[test]
    fn cosine_self_is_one_orthogonal_is_zero() {
        let a = [1.0_f32, 0.0, 0.0];
        let b = [0.0_f32, 1.0, 0.0];
        let c = [2.0_f32, 0.0, 0.0];
        assert!((cosine(&a, &a) - 1.0).abs() < 1e-6);
        assert!(cosine(&a, &b).abs() < 1e-6);
        assert!((cosine(&a, &c) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_handles_zero_and_mismatch() {
        assert_eq!(cosine(&[0.0, 0.0], &[1.0, 0.0]), 0.0);
        assert_eq!(cosine(&[1.0, 2.0], &[1.0]), 0.0);
    }
}
