//! `agentic check page_boundary` — three-tier body-length gate (ADR-0035).
//!
//! ADR-0035 caps the EN-core thesis body. This gate estimates the page count
//! from the word count of the markdown under `--prefix` (default
//! `thesis-draft-v5/`) at ~500 words/page and compares it to `--max-pages`
//! (default 60). Over the limit → WARN `PAGE_OVER`; otherwise INFO `PAGE_OK`.
//! Advisory only — a soft signal, not a hard block.

use anyhow::Result;
use rusqlite::Connection;

use agentic_core::worktree;

use crate::{CheckReport, Finding, Severity};

/// Words per estimated page (rough manuscript convention).
const WORDS_PER_PAGE: usize = 500;

/// Whitespace-token word count (fence-agnostic — a coarse manuscript estimate).
#[must_use]
pub fn word_count(text: &str) -> usize {
    text.split_whitespace().count()
}

/// Estimated pages from a word count at [`WORDS_PER_PAGE`] words/page (ceil).
#[must_use]
pub const fn pages_from_words(words: usize) -> usize {
    words.div_ceil(WORDS_PER_PAGE)
}

pub fn run(
    conn: &Connection,
    project: &str,
    prefix: &str,
    max_pages: usize,
) -> Result<CheckReport> {
    let mut findings = Vec::new();
    let mut words = 0usize;

    for (path, _sha) in worktree::list(conn, project, prefix)? {
        if !std::path::Path::new(&path)
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("md"))
        {
            continue;
        }
        let blob = worktree::read_at(conn, project, &path)?;
        let text = String::from_utf8_lossy(&blob.content);
        words += word_count(&text);
    }

    let pages = pages_from_words(words);
    if pages > max_pages {
        findings.push(Finding {
            category: "PAGE_OVER".into(),
            severity: Severity::Warn,
            message: format!("≈{pages} pages > {max_pages} (ADR-0035 body limit) — {words} words"),
            location: Some(prefix.to_string()),
        });
    } else {
        findings.push(Finding {
            category: "PAGE_OK".into(),
            severity: Severity::Info,
            message: format!("≈{pages} pages <= {max_pages} ({words} words under '{prefix}')"),
            location: Some("page_boundary".into()),
        });
    }

    Ok(CheckReport::new("page_boundary", findings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentic_core::{
        db::open_in_memory,
        project::{ProjectKind, create as create_project},
    };

    #[test]
    fn page_estimate_ceils() {
        assert_eq!(pages_from_words(0), 0);
        assert_eq!(pages_from_words(1), 1);
        assert_eq!(pages_from_words(500), 1);
        assert_eq!(pages_from_words(501), 2);
    }

    #[test]
    fn over_limit_warns() {
        let conn = open_in_memory().unwrap();
        let pid = create_project(&conn, "T", ProjectKind::Thesis, "en", None).unwrap();
        // 600 words → ≈2 pages; max 1 → over.
        let body = "word ".repeat(600);
        worktree::put_at(
            &conn,
            &pid,
            "thesis-draft-v5/ch.md",
            body.as_bytes(),
            "text/markdown",
            Some("en"),
            "u",
            "init",
        )
        .unwrap();
        let report = run(&conn, &pid, "thesis-draft-v5/", 1).unwrap();
        assert!(report.findings.iter().any(|f| f.category == "PAGE_OVER"));
    }

    #[test]
    fn under_limit_ok() {
        let conn = open_in_memory().unwrap();
        let pid = create_project(&conn, "T", ProjectKind::Thesis, "en", None).unwrap();
        worktree::put_at(
            &conn,
            &pid,
            "thesis-draft-v5/ch.md",
            b"a short chapter body",
            "text/markdown",
            Some("en"),
            "u",
            "init",
        )
        .unwrap();
        let report = run(&conn, &pid, "thesis-draft-v5/", 60).unwrap();
        assert!(report.findings.iter().any(|f| f.category == "PAGE_OK"));
        assert_eq!(report.verdict, crate::Verdict::Pass);
    }
}
