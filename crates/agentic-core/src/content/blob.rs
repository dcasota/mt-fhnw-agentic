//! Blob = a chunk of bytes (text or binary), content-addressed by SHA-256.

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::{Error, Result};

use super::hash;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Blob {
    pub sha256: String,
    pub mime: String,
    pub encoding: String,
    pub content: Vec<u8>,
    pub size_bytes: i64,
    pub lang: Option<String>,
    pub created_at: String,
}

/// Insert `content` if not present; return the SHA-256 hash either way.
///
/// `mime` should be a valid mime-type (e.g. `text/markdown`, `application/json`).
/// `lang` is optional and must be one of `en|de|fr|it|rm|hi` if set.
pub fn put_blob(
    conn: &Connection,
    content: &[u8],
    mime: &str,
    lang: Option<&str>,
) -> Result<String> {
    let sha = hash(content);
    let encoding = if std::str::from_utf8(content).is_ok() {
        "utf-8"
    } else {
        "base64"
    };
    conn.execute(
        "INSERT OR IGNORE INTO blobs (sha256, mime, encoding, content, size_bytes, lang) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![sha, mime, encoding, content, content.len() as i64, lang],
    )?;
    Ok(sha)
}

/// Fetch a blob by its hex SHA-256.
pub fn get_blob(conn: &Connection, sha: &str) -> Result<Blob> {
    let blob = conn
        .query_row(
            "SELECT sha256, mime, encoding, content, size_bytes, lang, created_at \
             FROM blobs WHERE sha256 = ?1",
            params![sha],
            |row| {
                Ok(Blob {
                    sha256: row.get(0)?,
                    mime: row.get(1)?,
                    encoding: row.get(2)?,
                    content: row.get(3)?,
                    size_bytes: row.get(4)?,
                    lang: row.get(5)?,
                    created_at: row.get(6)?,
                })
            },
        )
        .optional()?
        .ok_or_else(|| Error::BlobNotFound(sha.to_owned()))?;
    Ok(blob)
}

/// True if a blob with this hash already exists.
pub fn has_blob(conn: &Connection, sha: &str) -> Result<bool> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM blobs WHERE sha256 = ?1",
        params![sha],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use pretty_assertions::assert_eq;

    #[test]
    fn round_trip() {
        let conn = open_in_memory().unwrap();
        let sha = put_blob(&conn, b"hello world", "text/plain", Some("en")).unwrap();
        let blob = get_blob(&conn, &sha).unwrap();
        assert_eq!(blob.content, b"hello world");
        assert_eq!(blob.mime, "text/plain");
        assert_eq!(blob.encoding, "utf-8");
        assert_eq!(blob.lang.as_deref(), Some("en"));
        assert_eq!(blob.size_bytes, 11);
    }

    #[test]
    fn dedup_on_same_content() {
        let conn = open_in_memory().unwrap();
        let sha1 = put_blob(&conn, b"hello", "text/plain", None).unwrap();
        let sha2 = put_blob(&conn, b"hello", "text/plain", None).unwrap();
        assert_eq!(sha1, sha2);
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM blobs", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn binary_uses_base64_encoding_marker() {
        let conn = open_in_memory().unwrap();
        let sha = put_blob(&conn, &[0xff, 0xfe, 0xfd], "application/octet-stream", None).unwrap();
        let blob = get_blob(&conn, &sha).unwrap();
        assert_eq!(blob.encoding, "base64");
    }

    #[test]
    fn unknown_blob() {
        let conn = open_in_memory().unwrap();
        let err = get_blob(
            &conn,
            "0000000000000000000000000000000000000000000000000000000000000000",
        )
        .unwrap_err();
        assert!(matches!(err, Error::BlobNotFound(_)));
    }

    #[test]
    fn has_blob_predicate() {
        let conn = open_in_memory().unwrap();
        let sha = put_blob(&conn, b"abc", "text/plain", None).unwrap();
        assert!(has_blob(&conn, &sha).unwrap());
        assert!(!has_blob(&conn, "deadbeef").unwrap());
    }
}
