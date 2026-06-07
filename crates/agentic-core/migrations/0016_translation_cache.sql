-- =============================================================================
--  Migration 0016: translation segment cache (ADR-0062 Phase B/C savings)
-- =============================================================================
--  Content-addressed cache of completed `(source_lang, target_lang, source_text)`
--  → `target_text` mappings, so a re-translation of the same paragraph or the
--  same figspec body never re-bills the provider. Key = SHA256 of
--  `<src>|<tgt>|<text>` (the `<text>` is the LITERAL source bytes — no
--  normalisation; whitespace differences are treated as cache misses so the
--  output round-trips byte-for-byte).
--
--  Granularity:
--    - paragraph for documents (split on a blank line in `translate.rs`)
--    - whole-figspec JSON body for figure-scope translations
--
--  Side effect that matters more than $ savings: PINS the translation. The
--  same EN paragraph always resolves to the same DE/FR/IT/RM/HI paragraph
--  on subsequent runs, even when the underlying provider (Grok) is
--  non-deterministic at temp=0. Mitigates the 2026-06-07 Grok
--  pattern-anchoring class — once a clean translation is cached, the
--  cache prevents Grok from re-hallucinating a fresh elaboration the next
--  time the same source phrase is translated.

CREATE TABLE IF NOT EXISTS translation_cache (
    -- SHA256 of "<source_lang>|<target_lang>|<source_text>" (lowercase hex).
    -- PK lookups are the hot path; SQLite's PK index serves them in O(log n).
    cache_key       TEXT PRIMARY KEY,
    -- ISO-639-1 two-letter source/target language tags
    -- (en / de / fr / it / rm / hi).
    source_lang     TEXT NOT NULL,
    target_lang     TEXT NOT NULL,
    -- The literal source bytes that produced the key. Stored verbatim so
    -- collision diagnosis is trivial (compare source_text against the
    -- segment the caller looked up).
    source_text     TEXT NOT NULL,
    -- The accepted target-language translation.
    target_text     TEXT NOT NULL,
    -- Which provider/model produced the entry — auditable, so a future
    -- "evict everything Grok wrote" pass can target it precisely.
    provider        TEXT NOT NULL,
    model           TEXT NOT NULL,
    -- Optional scope for per-project caching when the same segment should
    -- not cross project boundaries (e.g. thesis-A's "operating modes"
    -- shouldn't seed thesis-B's translation). NULL = global hit.
    project_id      TEXT REFERENCES projects(id),
    -- Free-form granularity tag so the caller can record what kind of
    -- segment produced this entry (`paragraph` / `figspec` / `front-matter`).
    -- Lets analytics distinguish paragraph hits from figure hits.
    segment_kind    TEXT NOT NULL,
    -- ISO-8601 UTC timestamp of when the row was created.
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now'))
);
CREATE INDEX IF NOT EXISTS idx_translation_cache_langs
    ON translation_cache(source_lang, target_lang);
CREATE INDEX IF NOT EXISTS idx_translation_cache_project
    ON translation_cache(project_id) WHERE project_id IS NOT NULL;

INSERT OR IGNORE INTO schema_version (version) VALUES (16);
