//! Model-review adoption helpers (ADR-0049 ph3).
//!
//! `agentic review run` writes per-document verdicts as `claim_audit_results`
//! entries (`kind=model_review`). Merge/build consult these to honor adoption
//! live in the cascade — a path with the current verdict `assessment="exclude"`
//! is held out of the mainline build (LowRankings-with-justification: the
//! content remains in the store, the passport entry IS the justification, and a
//! later `accept` review supersedes the exclusion).

use anyhow::Result;
use rusqlite::Connection;
use serde_json::Value;
use std::collections::HashSet;

use crate::passport::{self, Section};

/// Paths whose CURRENT model_review assessment is "exclude". A path is included
/// once and only once per call; superseded entries are ignored by
/// `passport::current`, so a later "accept" review naturally re-includes the
/// document.
pub fn excluded_paths(conn: &Connection, project: &str) -> Result<HashSet<String>> {
    let mut out = HashSet::new();
    for e in passport::current(conn, project, Section::ClaimAuditResults)? {
        let Ok(v) = serde_json::from_str::<Value>(&e.payload_json) else {
            continue;
        };
        if v.get("kind").and_then(Value::as_str) != Some("model_review") {
            continue;
        }
        if v.get("assessment").and_then(Value::as_str) != Some("exclude") {
            continue;
        }
        if let Some(p) = v.get("path").and_then(Value::as_str) {
            out.insert(p.to_string());
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use crate::project::{ProjectKind, create as create_project};

    fn add(c: &Connection, p: &str, payload: &str) {
        passport::append(c, p, Section::ClaimAuditResults, payload, None, None).unwrap();
    }

    #[test]
    fn only_current_exclude_reviews_are_returned() {
        let c = open_in_memory().unwrap();
        let p = create_project(&c, "T", ProjectKind::Thesis, "en", None).unwrap();
        add(
            &c,
            &p,
            r#"{"kind":"model_review","path":"a.md","assessment":"exclude"}"#,
        );
        add(
            &c,
            &p,
            r#"{"kind":"model_review","path":"b.md","assessment":"accept"}"#,
        );
        // A non-review claim_audit_results entry must be ignored.
        add(
            &c,
            &p,
            r#"{"kind":"ranking","path":"c.md","tier":"Critical"}"#,
        );
        // The rankings-scope review has no path; must not crash.
        add(
            &c,
            &p,
            r#"{"kind":"model_review","scope":"rankings","assessment":"revise"}"#,
        );
        let ex = excluded_paths(&c, &p).unwrap();
        assert_eq!(ex.len(), 1);
        assert!(ex.contains("a.md"));
    }
}
