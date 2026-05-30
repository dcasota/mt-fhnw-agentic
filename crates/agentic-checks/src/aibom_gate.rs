//! `agentic check aibom` — AIBOM chronological-ledger integrity gate.
//!
//! The AIBOM is the merged, time-ordered record of journal actions, content
//! commits, AI/LLM decisions and gate verdicts (rendered in the signed audit
//! report). For the chronology to be trustworthy and non-repudiable:
//!   * every commit must be signed (no ledger entry outside the seal),
//!   * the journal must cover the full commit span (no unlogged tail), and
//!   * AI decisions must be recorded (a non-empty decision index).

use anyhow::Result;
use rusqlite::Connection;

use crate::{CheckReport, Finding, Severity};

fn finding(cat: &str, sev: Severity, msg: String) -> Finding {
    Finding {
        category: cat.to_string(),
        severity: sev,
        message: msg,
        location: Some("aibom".to_string()),
    }
}

pub fn run(conn: &Connection, project: &str) -> Result<CheckReport> {
    let mut findings = Vec::new();

    // 1. Every commit must be signed — else its ledger entry is outside the seal.
    let unsigned: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM commits c \
             LEFT JOIN signatures s ON s.target_kind='commit' AND s.target_id=c.sha256 \
             WHERE s.id IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if unsigned > 0 {
        // ADR-0023 / cascade-phase-ordering remediation (2026-05-30):
        // when this gate runs INSIDE a cascade subprocess (env
        // `AGENTIC_CASCADE_IN_PROGRESS=1` set by `cascade.rs`), an
        // unsigned-commit count > 0 is the EXPECTED transient state
        // between phase-5 ingest and phase-7 `audit sign-commits`.
        // Downgrade Error → Info so the cascade verdict reflects the
        // post-seal state, not the mid-cascade snapshot. Standalone
        // `agentic check aibom` invocations (env unset) keep Error
        // severity — a real unsigned commit IS a real ledger gap.
        let in_cascade = std::env::var("AGENTIC_CASCADE_IN_PROGRESS")
            .map(|v| !v.is_empty())
            .unwrap_or(false);
        let sev = if in_cascade {
            Severity::Info
        } else {
            Severity::Error
        };
        let suffix = if in_cascade {
            " (cascade-transient: phase-7 `audit sign-commits` will sign them; standalone re-run would PASS)"
        } else {
            ""
        };
        findings.push(finding(
            "AIBOM_UNSIGNED",
            sev,
            format!(
                "{unsigned} commit(s) unsigned — run `audit sign-commits` so every ledger entry is sealed{suffix}"
            ),
        ));
    }

    // 2. The journal must not lag the commit DAG (no unlogged work at the tail).
    let max_commit: Option<String> = conn
        .query_row("SELECT MAX(timestamp) FROM commits", [], |r| r.get(0))
        .unwrap_or(None);
    let max_journal: Option<String> = conn
        .query_row(
            "SELECT MAX(ts) FROM journal_entries WHERE project_id = ?1",
            [project],
            |r| r.get(0),
        )
        .unwrap_or(None);
    if let (Some(mc), Some(mj)) = (max_commit.as_deref(), max_journal.as_deref()) {
        // Compare on the date (YYYY-MM-DD): a commit dated after the newest
        // journal entry means an action was committed without being logged.
        if mc.get(0..10).unwrap_or("") > mj.get(0..10).unwrap_or("") {
            findings.push(finding(
                "AIBOM_UNLOGGED",
                Severity::Error,
                format!(
                    "newest commit ({mc}) post-dates the newest journal entry ({mj}) — log this work before sealing"
                ),
            ));
        }
    }

    // 3. The AI-decision index must not be empty (decisions are part of the BOM).
    let decisions: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM audit_rows WHERE project_id = ?1",
            [project],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if decisions == 0 {
        findings.push(finding(
            "AIBOM_NO_DECISIONS",
            Severity::Warn,
            "no AI/LLM decisions recorded (`audit record`) — the decision index is empty"
                .to_string(),
        ));
    }

    Ok(CheckReport::new("aibom", findings))
}
