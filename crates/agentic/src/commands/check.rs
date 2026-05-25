//! `agentic check` — integrity checker dispatch.

use std::path::Path;

use anyhow::Result;
use serde_json::json;

use agentic_checks::{CheckReport, Verdict};

use crate::cli::CheckAction;

pub async fn run(db_path: &Path, action: CheckAction, json_out: bool) -> Result<()> {
    let conn = agentic_core::db::open(db_path)?;
    // Each arm yields (report, project?) so EVERY check can record an
    // audit_verdict (ADR-0041: enhanced checks after the research-skill must
    // themselves be auditable — their verdicts land in the AIBOM/report).
    let (report, project): (CheckReport, Option<String>) = match action {
        CheckAction::Self_ { project } => (agentic_checks::self_check::run(&conn)?, project),
        CheckAction::WritingQuality { project } => (
            agentic_checks::writing_quality::run(&conn, &project)?,
            Some(project),
        ),
        CheckAction::Citations { project } => (
            agentic_checks::citation_tracker::run(&conn, &project)?,
            Some(project),
        ),
        CheckAction::Contamination { project, offline } => (
            agentic_checks::contamination::run(&conn, &project, !offline).await?,
            Some(project),
        ),
        CheckAction::Deliverable { project, prefix } => {
            // Anchored verified facts (ADR-0016/0042): a number on a matching
            // line is already sourced and is not flagged NUMBER_UNSOURCED.
            let anchored = crate::commands::facts::anchored_claims(&conn, &project)
                .unwrap_or_default();
            let entries = agentic_core::worktree::list(&conn, &project, &prefix)?;
            let mut findings = Vec::new();
            for (path, _sha) in entries.iter().filter(|(p, _)| p.ends_with(".md")) {
                let blob = agentic_core::worktree::read_at(&conn, &project, path)?;
                let text = String::from_utf8_lossy(&blob.content);
                findings.extend(agentic_checks::deliverable_gate::findings_for_facts(
                    path, &text, &anchored,
                ));
            }
            (
                agentic_checks::CheckReport::new("deliverable", findings),
                Some(project),
            )
        }
        CheckAction::Docs { project, root } => (
            agentic_checks::docs_gate::run(&conn, &project, &root)?,
            Some(project),
        ),
        CheckAction::Bibliography { project, prefix } => (
            agentic_checks::bibliography_gate::run(&conn, &project, &prefix)?,
            Some(project),
        ),
        CheckAction::Aibom { project } => {
            (agentic_checks::aibom_gate::run(&conn, &project)?, Some(project))
        }
        CheckAction::FactsIntegrity { project } => {
            (agentic_checks::facts_gate::run(&conn, &project)?, Some(project))
        }
        CheckAction::Tree {
            project,
            root,
            prefix,
        } => (
            agentic_checks::tree_integrity::run(&conn, &project, &root, &prefix)?,
            Some(project),
        ),
    };

    // Record the verdict so the gate run is itself auditable (ADR-0041).
    if let Some(p) = &project {
        let verdict = match report.verdict {
            Verdict::Pass => "PASS",
            Verdict::Warn => "WARN",
            Verdict::Fail => "FAIL",
        };
        let _ = conn.execute(
            "INSERT INTO audit_verdicts (project_id, checkpoint, verdict, findings_json) \
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![
                p,
                report.checker,
                verdict,
                serde_json::to_string(&report.findings).ok()
            ],
        );
    }

    print_report(&report, json_out);
    if matches!(report.verdict, Verdict::Fail) {
        std::process::exit(1);
    }
    Ok(())
}

fn print_report(report: &CheckReport, json_out: bool) {
    if json_out {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "checker": report.checker,
                "verdict": report.verdict,
                "findings": report.findings,
            }))
            .unwrap_or_else(|_| "{}".into())
        );
        return;
    }
    println!(
        "=== {} -- {} -- {} finding{} ===",
        report.checker,
        verdict_label(report.verdict),
        report.findings.len(),
        if report.findings.len() == 1 { "" } else { "s" }
    );
    for f in &report.findings {
        let where_ = f.location.as_deref().unwrap_or("-");
        println!(
            "  [{:<8}] [{:>17}] {} ({})",
            severity_label(&f.severity),
            f.category,
            f.message,
            where_
        );
    }
}

fn verdict_label(v: Verdict) -> &'static str {
    match v {
        Verdict::Pass => "PASS",
        Verdict::Warn => "WARN",
        Verdict::Fail => "FAIL",
    }
}

fn severity_label(s: &agentic_checks::Severity) -> &'static str {
    match s {
        agentic_checks::Severity::Info => "INFO",
        agentic_checks::Severity::Warn => "WARN",
        agentic_checks::Severity::Error => "ERROR",
        agentic_checks::Severity::Blocking => "BLOCKING",
    }
}
