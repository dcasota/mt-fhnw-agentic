//! Translation segment cache (ADR-0062 Phase B/C savings).
//!
//! Content-addressed lookup for completed `(source_lang, target_lang,
//! source_text)` → `target_text` mappings. The cache is byte-literal:
//! whitespace differences in the source are treated as cache misses so
//! round-trips reproduce verbatim.
//!
//! See `migrations/0016_translation_cache.sql` for the schema rationale
//! and the side-effect note about pinning translations against the Grok
//! pattern-anchoring class.

use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};

use crate::Result;

/// One cache entry — the row that `get` returns and `put` writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub cache_key: String,
    pub source_lang: String,
    pub target_lang: String,
    pub source_text: String,
    pub target_text: String,
    pub provider: String,
    pub model: String,
    pub project_id: Option<String>,
    pub segment_kind: String,
    pub created_at: String,
}

/// Compute the cache key for a `(source_lang, target_lang, source_text)`
/// triple. The key is `sha256("<src>|<tgt>|<text>")` rendered as lower-hex.
///
/// `source_text` is consumed VERBATIM — no trim, no normalisation. The
/// caller is responsible for handing in the bytes it actually wants
/// translated; a trailing newline is a different cache entry than no
/// trailing newline.
#[must_use]
pub fn key_for(source_lang: &str, target_lang: &str, source_text: &str) -> String {
    let mut h = Sha256::new();
    h.update(source_lang.as_bytes());
    h.update(b"|");
    h.update(target_lang.as_bytes());
    h.update(b"|");
    h.update(source_text.as_bytes());
    hex::encode(h.finalize())
}

/// Look up a cached translation. Returns `Ok(None)` on miss.
///
/// `project_id`:
/// - `Some(p)`: prefer a per-project entry first; fall back to a global
///   (`project_id IS NULL`) entry if no per-project entry exists. This
///   lets a per-thesis seeded vocabulary win without losing globally
///   useful translations.
/// - `None`: only look at global entries.
pub fn get(
    conn: &Connection,
    source_lang: &str,
    target_lang: &str,
    source_text: &str,
    project_id: Option<&str>,
) -> Result<Option<Entry>> {
    let key = key_for(source_lang, target_lang, source_text);
    if let Some(p) = project_id {
        let row: Option<Entry> = conn
            .query_row(
                "SELECT cache_key, source_lang, target_lang, source_text, target_text,
                        provider, model, project_id, segment_kind, created_at
                   FROM translation_cache
                  WHERE cache_key = ?1 AND project_id = ?2",
                params![key, p],
                row_to_entry,
            )
            .optional()?;
        if row.is_some() {
            return Ok(row);
        }
    }
    let row: Option<Entry> = conn
        .query_row(
            "SELECT cache_key, source_lang, target_lang, source_text, target_text,
                    provider, model, project_id, segment_kind, created_at
               FROM translation_cache
              WHERE cache_key = ?1 AND project_id IS NULL",
            params![key],
            row_to_entry,
        )
        .optional()?;
    Ok(row)
}

/// Insert (or overwrite) a cached translation. `project_id = None` writes
/// a global entry; `Some(p)` scopes it to that project.
pub fn put(
    conn: &Connection,
    source_lang: &str,
    target_lang: &str,
    source_text: &str,
    target_text: &str,
    provider: &str,
    model: &str,
    project_id: Option<&str>,
    segment_kind: &str,
) -> Result<String> {
    let key = key_for(source_lang, target_lang, source_text);
    conn.execute(
        "INSERT INTO translation_cache
           (cache_key, source_lang, target_lang, source_text, target_text,
            provider, model, project_id, segment_kind)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(cache_key) DO UPDATE SET
           target_text = excluded.target_text,
           provider    = excluded.provider,
           model       = excluded.model,
           project_id  = excluded.project_id,
           segment_kind = excluded.segment_kind",
        params![
            key,
            source_lang,
            target_lang,
            source_text,
            target_text,
            provider,
            model,
            project_id,
            segment_kind,
        ],
    )?;
    Ok(key)
}

/// Count entries — used by the translate CLI to print
/// `cache hits=N misses=M` after a run.
pub fn count(conn: &Connection) -> Result<i64> {
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM translation_cache", [], |r| r.get(0))?;
    Ok(n)
}

/// Coerce a column to `String` even when the row stores it as `BLOB`
/// (which happens when an operator updates a cache row via SQLite's
/// `readfile()` and forgets the `CAST(... AS TEXT)` follow-up). Falls
/// back to `r.get(idx)` for strict TEXT rows.
fn col_text(r: &rusqlite::Row, idx: usize) -> rusqlite::Result<String> {
    use rusqlite::types::ValueRef;
    match r.get_ref(idx)? {
        ValueRef::Text(s) => Ok(String::from_utf8_lossy(s).into_owned()),
        ValueRef::Blob(b) => Ok(String::from_utf8_lossy(b).into_owned()),
        ValueRef::Null => Err(rusqlite::Error::InvalidColumnType(
            idx,
            "expected TEXT or BLOB, got NULL".into(),
            rusqlite::types::Type::Null,
        )),
        other => Err(rusqlite::Error::InvalidColumnType(
            idx,
            format!("expected TEXT or BLOB, got {other:?}"),
            other.data_type(),
        )),
    }
}

fn row_to_entry(r: &rusqlite::Row) -> rusqlite::Result<Entry> {
    Ok(Entry {
        cache_key: col_text(r, 0)?,
        source_lang: col_text(r, 1)?,
        target_lang: col_text(r, 2)?,
        source_text: col_text(r, 3)?,
        target_text: col_text(r, 4)?,
        provider: col_text(r, 5)?,
        model: col_text(r, 6)?,
        project_id: r.get(7)?, // Option<String> — let rusqlite handle the NULL branch.
        segment_kind: col_text(r, 8)?,
        created_at: col_text(r, 9)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    #[test]
    fn key_is_byte_literal() {
        // Whitespace differences produce different keys — that's intentional;
        // the cache returns the EXACT translated bytes so a re-translation
        // round-trips verbatim.
        let a = key_for("en", "de", "operating modes");
        let b = key_for("en", "de", "operating modes ");
        assert_ne!(a, b);
        // Same inputs produce identical keys.
        let c = key_for("en", "de", "operating modes");
        assert_eq!(a, c);
        // Language pair changes the key.
        let d = key_for("en", "fr", "operating modes");
        assert_ne!(a, d);
    }

    #[test]
    fn put_get_round_trip_global() {
        let conn = db::open_in_memory().unwrap();
        let key = put(
            &conn,
            "en",
            "de",
            "operating modes",
            "Betriebsmodi",
            "Grok",
            "grok-4.3",
            None,
            "paragraph",
        )
        .unwrap();
        assert_eq!(key.len(), 64); // sha256 hex
        let got = get(&conn, "en", "de", "operating modes", None).unwrap();
        let entry = got.expect("entry should exist");
        assert_eq!(entry.target_text, "Betriebsmodi");
        assert_eq!(entry.provider, "Grok");
        assert_eq!(entry.project_id, None);
    }

    #[test]
    fn miss_returns_none() {
        let conn = db::open_in_memory().unwrap();
        let got = get(&conn, "en", "de", "operating modes", None).unwrap();
        assert!(got.is_none());
    }

    #[test]
    fn project_scope_lookup_hits_then_falls_back_to_global() {
        // v1 schema has `cache_key` as the sole PK, so only ONE row exists
        // per (source_lang, target_lang, source_text) — the `project_id`
        // column records which project last wrote the entry, but the
        // get() lookup logic still prefers a project-scoped row when one
        // exists and falls back to a global row otherwise. We exercise
        // both branches with distinct source texts so the schema's
        // single-row-per-key invariant is preserved.
        let conn = db::open_in_memory().unwrap();
        conn.execute("PRAGMA foreign_keys = OFF;", []).unwrap();
        // A project-scoped entry for one phrase.
        put(
            &conn,
            "en",
            "de",
            "operating modes",
            "PROJECT-SCOPED Betriebsmodi",
            "Grok",
            "grok-4.3",
            Some("proj-A"),
            "paragraph",
        )
        .unwrap();
        // A global entry for a different phrase.
        put(
            &conn,
            "en",
            "de",
            "the system",
            "GLOBAL das System",
            "Grok",
            "grok-4.3",
            None,
            "paragraph",
        )
        .unwrap();
        // Project-scoped lookup with the matching project id hits the
        // project entry.
        let got = get(&conn, "en", "de", "operating modes", Some("proj-A"))
            .unwrap()
            .unwrap();
        assert_eq!(got.target_text, "PROJECT-SCOPED Betriebsmodi");
        // Project-scoped lookup for the OTHER phrase falls back to global
        // (no project-scoped row exists for "the system").
        let got = get(&conn, "en", "de", "the system", Some("proj-A"))
            .unwrap()
            .unwrap();
        assert_eq!(got.target_text, "GLOBAL das System");
        // Lookup with no project hint returns the global entry.
        let got = get(&conn, "en", "de", "the system", None).unwrap().unwrap();
        assert_eq!(got.target_text, "GLOBAL das System");
    }

    #[test]
    fn get_tolerates_blob_typed_target_text() {
        // SQLite is dynamically typed. An operator who hand-edits a cache
        // row via `UPDATE … SET target_text = readfile(…)` ends up with a
        // BLOB-typed value in a TEXT column; the reader must still return
        // a `String`, not error with `InvalidColumnType(Blob)`.
        let conn = db::open_in_memory().unwrap();
        let key = key_for("en", "de", "operating modes");
        conn.execute(
            "INSERT INTO translation_cache
               (cache_key, source_lang, target_lang, source_text, target_text,
                provider, model, project_id, segment_kind, created_at)
             VALUES (?1, 'en', 'de', 'operating modes',
                     CAST(x'42657472696562736d6f6469' AS BLOB),
                     'human-correction', 'manual', NULL,
                     'paragraph', '2026-06-07T00:00:00Z')",
            params![key],
        )
        .unwrap();
        let got = get(&conn, "en", "de", "operating modes", None)
            .unwrap()
            .expect("blob-typed row should still be readable");
        assert_eq!(got.target_text, "Betriebsmodi");
        assert_eq!(got.provider, "human-correction");
    }

    #[test]
    fn put_upserts_on_existing_key() {
        let conn = db::open_in_memory().unwrap();
        put(
            &conn,
            "en",
            "de",
            "operating modes",
            "first take",
            "Grok",
            "grok-4.3",
            None,
            "paragraph",
        )
        .unwrap();
        put(
            &conn,
            "en",
            "de",
            "operating modes",
            "revised take",
            "Anthropic",
            "claude-opus-4-7",
            None,
            "paragraph",
        )
        .unwrap();
        let got = get(&conn, "en", "de", "operating modes", None)
            .unwrap()
            .unwrap();
        assert_eq!(got.target_text, "revised take");
        assert_eq!(got.provider, "Anthropic");
        assert_eq!(count(&conn).unwrap(), 1);
    }
}
