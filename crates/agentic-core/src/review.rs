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

/// Paths whose CURRENT model_review assessment is "exclude". If a path has
/// multiple current entries (legacy data before supersede was enforced), only
/// the latest by entry id is consulted — latest-wins, so an old "exclude"
/// cannot persist after a newer "accept". A later "accept" review naturally
/// re-includes the document via passport supersede.
pub fn excluded_paths(conn: &Connection, project: &str) -> Result<HashSet<String>> {
    let entries = passport::current(conn, project, Section::ClaimAuditResults)?;
    // Per-path, keep the highest-id current entry's assessment.
    let mut latest: std::collections::HashMap<String, (i64, String)> =
        std::collections::HashMap::new();
    for e in entries {
        let Ok(v) = serde_json::from_str::<Value>(&e.payload_json) else {
            continue;
        };
        if v.get("kind").and_then(Value::as_str) != Some("model_review") {
            continue;
        }
        let Some(path) = v.get("path").and_then(Value::as_str) else {
            continue; // skip scope=rankings entries (no path)
        };
        let assessment = v
            .get("assessment")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let cur = latest.get(path).map(|(id, _)| *id).unwrap_or(0);
        if e.id > cur {
            latest.insert(path.to_string(), (e.id, assessment.to_string()));
        }
    }
    Ok(latest
        .into_iter()
        .filter_map(|(p, (_, a))| (a == "exclude").then_some(p))
        .collect())
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
    fn latest_review_wins_per_path() {
        // Two current entries for the same path (legacy before supersede was
        // enforced): the LATEST one decides. An older "exclude" must not
        // persist after a newer "accept".
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
            r#"{"kind":"model_review","path":"a.md","assessment":"accept"}"#,
        );
        let ex = excluded_paths(&c, &p).unwrap();
        assert!(
            !ex.contains("a.md"),
            "latest accept must win over older exclude"
        );
        // The reverse: latest exclude wins over older accept.
        let c2 = open_in_memory().unwrap();
        let p2 = create_project(&c2, "T", ProjectKind::Thesis, "en", None).unwrap();
        add(
            &c2,
            &p2,
            r#"{"kind":"model_review","path":"b.md","assessment":"accept"}"#,
        );
        add(
            &c2,
            &p2,
            r#"{"kind":"model_review","path":"b.md","assessment":"exclude"}"#,
        );
        assert!(excluded_paths(&c2, &p2).unwrap().contains("b.md"));
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
