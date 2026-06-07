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

fn row_to_entry(r: &rusqlite::Row) -> rusqlite::Result<Entry> {
    Ok(Entry {
        cache_key: r.get(0)?,
        source_lang: r.get(1)?,
        target_lang: r.get(2)?,
        source_text: r.get(3)?,
        target_text: r.get(4)?,
        provider: r.get(5)?,
        model: r.get(6)?,
        project_id: r.get(7)?,
        segment_kind: r.get(8)?,
        created_at: r.get(9)?,
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
    fn project_scope_preferred_then_global_fallback() {
        let conn = db::open_in_memory().unwrap();
        // Seed a project-scoped entry (project id not in projects table is
        // fine for the in-memory test — the FK is `REFERENCES projects(id)`
        // but FKs in SQLite require both PRAGMA and the table to exist; the
        // schema-creation runner sets the PRAGMA but tests can drop it).
        conn.execute("PRAGMA foreign_keys = OFF;", []).unwrap();
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
        // Seed a different global entry for the same key.
        put(
            &conn,
            "en",
            "de",
            "operating modes",
            "GLOBAL Betriebsmodi",
            "Grok",
            "grok-4.3",
            None,
            "paragraph",
        )
        .unwrap();
        // Lookup with the project hint must prefer the project entry.
        let got = get(&conn, "en", "de", "operating modes", Some("proj-A"))
            .unwrap()
            .unwrap();
        assert_eq!(got.target_text, "PROJECT-SCOPED Betriebsmodi");
        // Lookup with a DIFFERENT project hint falls back to global.
        let got = get(&conn, "en", "de", "operating modes", Some("proj-B"))
            .unwrap()
            .unwrap();
        assert_eq!(got.target_text, "GLOBAL Betriebsmodi");
        // Lookup with no project hint returns the global entry.
        let got = get(&conn, "en", "de", "operating modes", None)
            .unwrap()
            .unwrap();
        assert_eq!(got.target_text, "GLOBAL Betriebsmodi");
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
