//! `agentic check reproducibility` — content-pinning gate (ADR-0039).
//!
//! Reproducibility requires that every authored artefact be bound to a SIGNED
//! commit, so the exact bytes can be re-derived and non-repudiably attested.
//! This gate counts passport `literature_corpus` + `verified_facts` entries
//! whose backing commit is NULL or unsigned (reusing the same
//! `commits LEFT JOIN signatures` idea as the AIBOM gate). Any unpinned content
//! surfaces a WARN `REPRO_UNPINNED`; PASS when every entry is bound to a signed
//! commit. Advisory only.

use anyhow::Result;
use rusqlite::Connection;

use agentic_core::passport::{self, Section};

use crate::{CheckReport, Finding, Severity};

/// Is this commit SHA present and signed?
fn is_signed(conn: &Connection, sha: &str) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM signatures \
         WHERE target_kind = 'commit' AND target_id = ?1",
        [sha],
        |r| r.get::<_, i64>(0),
    )
    .unwrap_or(0)
        > 0
}

pub fn run(conn: &Connection, project: &str) -> Result<CheckReport> {
    let mut findings = Vec::new();
    let mut total = 0usize;
    let mut pinned = 0usize;

    for section in [Section::LiteratureCorpus, Section::VerifiedFacts] {
        for e in passport::current(conn, project, section)? {
            total += 1;
            let signed = e.commit_sha.as_deref().is_some_and(|s| is_signed(conn, s));
            if signed {
                pinned += 1;
            }
        }
    }

    let unpinned = total - pinned;
    if unpinned > 0 {
        findings.push(Finding {
            category: "REPRO_UNPINNED".into(),
            severity: Severity::Warn,
            message: format!(
                "{unpinned}/{total} passport entry/entries are not bound to a signed commit — \
                 commit + `audit sign-commits` to pin them"
            ),
            location: Some("passport".into()),
        });
    }

    findings.push(Finding {
        category: "REPRO_SUMMARY".into(),
        severity: Severity::Info,
        message: format!("{pinned}/{total} passport entry/entries pinned to a signed commit"),
        location: Some("reproducibility".into()),
    });

    Ok(CheckReport::new("reproducibility", findings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentic_core::{
        db::open_in_memory,
        project::{ProjectKind, create as create_project},
    };

    #[test]
    fn unsigned_entry_warns() {
        let conn = open_in_memory().unwrap();
        let pid = create_project(&conn, "T", ProjectKind::Thesis, "en", None).unwrap();
        // commit_sha = None → unpinned.
        passport::append(
            &conn,
            &pid,
            Section::VerifiedFacts,
            r#"{"kind":"measured","claim":"42","source":"out/x.md"}"#,
            None,
            None,
        )
        .unwrap();
        let report = run(&conn, &pid).unwrap();
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.category == "REPRO_UNPINNED")
        );
    }

    #[test]
    fn empty_corpus_passes() {
        let conn = open_in_memory().unwrap();
        let pid = create_project(&conn, "T", ProjectKind::Thesis, "en", None).unwrap();
        let report = run(&conn, &pid).unwrap();
        assert_eq!(report.verdict, crate::Verdict::Pass);
    }
}
