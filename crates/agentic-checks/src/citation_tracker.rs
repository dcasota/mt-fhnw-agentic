//! Citation tracker: APA 7 in-text scanning + cross-check against the
//! material-passport's `literature_corpus` section + online-quota gate.
//!
//! Two findings categories:
//! * `CITATION_MISSING_REF`  — in-text citation not present in literature_corpus
//! * `CITATION_ONLINE_QUOTA` — > 10 % of literature_corpus entries are
//!    URL-only (no DOI, no print indicator) -- FHNW MAS hard rule.

use std::collections::HashSet;
use std::sync::OnceLock;

use agentic_core::{passport, worktree};
use anyhow::Result;
use regex::Regex;
use rusqlite::Connection;

use crate::{CheckReport, Finding, Severity};

/// Online-source quota threshold (FHNW MAS hard rule: max 10 %).
pub const ONLINE_QUOTA_PCT: f64 = 10.0;

fn apa_inline() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    // Matches "(Author, 2024)" / "(Author & Other, 2024)" / "(Author et al., 2024a)".
    // The connector + second-author tail is optional; "et al." can stand alone.
    R.get_or_init(|| {
        Regex::new(
            r"\(([A-Z][\p{L}\-']+)(?:\s+(?:&\s*[A-Z][\p{L}\-']+|et\s+al\.|und\s+[A-Z][\p{L}\-']+))?,\s*(\d{4})([a-z]?)(?:,\s*[^)]+)?\)",
        )
        .unwrap()
    })
}

/// Build the set of `<author><year><suffix>` slugs from in-text citations.
#[must_use]
pub fn extract_inline_keys(text: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    for caps in apa_inline().captures_iter(text) {
        let author = caps.get(1).map_or("", |m| m.as_str()).to_lowercase();
        let year = caps.get(2).map_or("", |m| m.as_str());
        let suffix = caps.get(3).map_or("", |m| m.as_str());
        // The portion before "&" or " et al."
        let primary_author = author.split_whitespace().next().unwrap_or(&author);
        out.insert(format!("{primary_author}{year}{suffix}"));
    }
    out
}

/// Run the checker against a project's working tree + material passport.
pub fn run(conn: &Connection, project_id: &str) -> Result<CheckReport> {
    let mut findings = Vec::new();

    // 1. Gather literature_corpus citation_keys from the passport.
    let corpus = passport::current(conn, project_id, passport::Section::LiteratureCorpus)?;
    let mut corpus_keys: HashSet<String> = HashSet::new();
    let mut online_only_count = 0usize;
    for entry in &corpus {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&entry.payload_json) {
            if let Some(key) = value.get("citation_key").and_then(|v| v.as_str()) {
                corpus_keys.insert(key.to_lowercase());
            }
            // "Online-only" = no DOI, no ISBN, no publisher.
            let has_doi = value.get("doi").and_then(serde_json::Value::as_str).map_or(false, |s| !s.is_empty());
            let has_isbn = value.get("isbn").and_then(serde_json::Value::as_str).map_or(false, |s| !s.is_empty());
            let has_publisher = value.get("publisher").and_then(serde_json::Value::as_str).map_or(false, |s| !s.is_empty());
            if !has_doi && !has_isbn && !has_publisher {
                online_only_count += 1;
            }
        }
    }

    // 2. Online quota.
    let total_refs = corpus.len();
    if total_refs > 0 {
        let pct = (online_only_count as f64 / total_refs as f64) * 100.0;
        if pct > ONLINE_QUOTA_PCT {
            findings.push(Finding {
                category: "CITATION_ONLINE_QUOTA".into(),
                severity: Severity::Error,
                message: format!(
                    "{online_only_count}/{total_refs} references are URL-only ({pct:.1} % > {ONLINE_QUOTA_PCT:.0} % FHNW MAS limit)."
                ),
                location: None,
            });
        }
    }

    // 3. Walk markdown blobs; collect inline citations; cross-check against corpus.
    let mut missing: Vec<(String, String)> = Vec::new();
    for (path, blob_sha) in worktree::list(conn, project_id, "")? {
        if !path.ends_with(".md") {
            continue;
        }
        let blob = agentic_core::content::blob::get_blob(conn, &blob_sha)?;
        let text = std::str::from_utf8(&blob.content).unwrap_or("");
        for key in extract_inline_keys(text) {
            if !corpus_keys.contains(&key) {
                missing.push((path.clone(), key));
            }
        }
    }
    missing.sort();
    missing.dedup();
    for (path, key) in missing {
        findings.push(Finding {
            category: "CITATION_MISSING_REF".into(),
            severity: Severity::Error,
            message: format!("In-text citation '{key}' has no matching literature_corpus entry."),
            location: Some(path),
        });
    }

    Ok(CheckReport::new("citation_tracker", findings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentic_core::{
        db::open_in_memory,
        project::{ProjectKind, create as create_project},
    };
    use pretty_assertions::assert_eq;

    #[test]
    fn extracts_simple_apa_citation() {
        let keys = extract_inline_keys("As Probst (2015) and (Mayer, 2022) showed, …");
        // narrative "Probst (2015)" — our regex requires the opening paren, so only "(Mayer, 2022)" matches.
        assert!(keys.contains("mayer2022"));
    }

    #[test]
    fn extracts_et_al_and_suffix() {
        let keys = extract_inline_keys("(Weyns et al., 2024a) and (Smith & Jones, 2023).");
        assert!(keys.contains("weyns2024a"));
        assert!(keys.contains("smith2023"));
    }

    #[test]
    fn flags_missing_reference() {
        let conn = open_in_memory().unwrap();
        let pid = create_project(&conn, "T", ProjectKind::Thesis, "de", None).unwrap();
        agentic_core::worktree::put_at(
            &conn, &pid, "thesis-draft/ch.md",
            b"Per (Mayer, 2022), no.",
            "text/markdown", Some("de"), "u", "init",
        ).unwrap();
        // No corpus entry → missing.
        let report = run(&conn, &pid).unwrap();
        assert!(report.findings.iter().any(|f| f.category == "CITATION_MISSING_REF"));
    }

    #[test]
    fn accepts_present_reference() {
        let conn = open_in_memory().unwrap();
        let pid = create_project(&conn, "T", ProjectKind::Thesis, "de", None).unwrap();
        agentic_core::worktree::put_at(
            &conn, &pid, "thesis-draft/ch.md",
            b"Per (Mayer, 2022), no.",
            "text/markdown", Some("de"), "u", "init",
        ).unwrap();
        passport::append(&conn, &pid, passport::Section::LiteratureCorpus,
            r#"{"citation_key":"mayer2022","doi":"10.1000/1","title":"X"}"#, None, None).unwrap();
        let report = run(&conn, &pid).unwrap();
        assert!(!report.findings.iter().any(|f| f.category == "CITATION_MISSING_REF"));
    }

    #[test]
    fn flags_online_quota() {
        let conn = open_in_memory().unwrap();
        let pid = create_project(&conn, "T", ProjectKind::Thesis, "de", None).unwrap();
        // Two corpus entries, both URL-only → 100 % over the 10 % limit.
        passport::append(&conn, &pid, passport::Section::LiteratureCorpus,
            r#"{"citation_key":"a2024","url":"https://x"}"#, None, None).unwrap();
        passport::append(&conn, &pid, passport::Section::LiteratureCorpus,
            r#"{"citation_key":"b2024","url":"https://y"}"#, None, None).unwrap();
        let report = run(&conn, &pid).unwrap();
        assert!(report.findings.iter().any(|f| f.category == "CITATION_ONLINE_QUOTA"));
    }

    #[test]
    fn passes_when_online_quota_within_limit() {
        let conn = open_in_memory().unwrap();
        let pid = create_project(&conn, "T", ProjectKind::Thesis, "de", None).unwrap();
        // 1 URL-only out of 11 = 9 % → OK.
        passport::append(&conn, &pid, passport::Section::LiteratureCorpus,
            r#"{"citation_key":"online1","url":"https://x"}"#, None, None).unwrap();
        for i in 0..10 {
            let payload = format!(r#"{{"citation_key":"book{i}","doi":"10.1000/{i}"}}"#);
            passport::append(&conn, &pid, passport::Section::LiteratureCorpus, &payload, None, None).unwrap();
        }
        let report = run(&conn, &pid).unwrap();
        assert!(!report.findings.iter().any(|f| f.category == "CITATION_ONLINE_QUOTA"));
    }
}
