//! `agentic check` — integrity checker dispatch.

use std::path::Path;

use anyhow::Result;
use serde_json::json;

use agentic_checks::{CheckReport, Verdict};

use crate::cli::CheckAction;

pub async fn run(db_path: &Path, action: CheckAction, lang: &str, json_out: bool) -> Result<()> {
    let (report, _project) = run_report(db_path, action, lang).await?;
    print_report(&report, json_out);
    if matches!(report.verdict, Verdict::Fail) {
        std::process::exit(1);
    }
    Ok(())
}

/// Run a single gate and return its `(report, project)` WITHOUT printing or
/// calling `process::exit` — so the cascade / SDD Cycle can compose gates
/// in-process and harvest verdicts directly (ADR-0047, item 1). The verdict is
/// still recorded in `audit_verdicts` here; the CLI wrapper `run` adds printing
/// and the exit-on-FAIL. In-process callers must never exit.
pub async fn run_report(
    db_path: &Path,
    action: CheckAction,
    lang: &str,
) -> Result<(CheckReport, Option<String>)> {
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
            let anchored =
                crate::commands::facts::anchored_claims(&conn, &project).unwrap_or_default();
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
        CheckAction::Aibom { project } => (
            agentic_checks::aibom_gate::run(&conn, &project)?,
            Some(project),
        ),
        CheckAction::AdrEnforcement { project } => (
            agentic_checks::adr_enforcement_gate::run(&conn, &project)?,
            Some(project),
        ),
        CheckAction::TermRename {
            project,
            token,
            replacement,
            since,
        } => {
            let report = if let Some(tok) = token {
                let term = agentic_checks::term_rename_gate::DeprecatedTerm {
                    token: tok,
                    replacement,
                    since,
                    severity: "warn".into(),
                    note: None,
                };
                agentic_checks::term_rename_gate::run_with_terms(&conn, &project, &[term])?
            } else {
                agentic_checks::term_rename_gate::run(&conn, &project)?
            };
            (report, Some(project))
        }
        CheckAction::FactsIntegrity { project } => (
            agentic_checks::facts_gate::run(&conn, &project)?,
            Some(project),
        ),
        CheckAction::I18n { project } => {
            // The chrome table is compile-time, so the gate needs no DB; the
            // project is taken only to record the audit_verdict (checkpoint
            // "i18n") consistently with the other gates.
            //
            // The target language is the global `--lang` (default "en"). The
            // English baseline can never leak into itself, so the default "en"
            // means "no specific target" → check every non-English language;
            // any explicit language (incl. an unsupported one) is checked
            // directly.
            let target = if lang.eq_ignore_ascii_case("en") {
                None
            } else {
                Some(lang)
            };
            (agentic_checks::i18n_gate::run(target), Some(project))
        }
        CheckAction::Bookkit {
            project,
            prefix,
            paths_from_manifest,
            book_key,
        } => {
            let report = match (paths_from_manifest, book_key) {
                (Some(manifest_path), Some(key)) => {
                    let paths = load_manifest_paths(&conn, &project, &manifest_path, &key)?;
                    let paths_ref: Vec<&str> = paths.iter().map(String::as_str).collect();
                    agentic_checks::bookkit_gate::run_scoped(
                        &conn,
                        &project,
                        agentic_checks::bookkit_gate::Scope::Paths {
                            book_key: &key,
                            paths: &paths_ref,
                        },
                    )?
                }
                _ => agentic_checks::bookkit_gate::run(&conn, &project, &prefix)?,
            };
            (report, Some(project))
        }
        CheckAction::Prisma { project, prefix } => (
            agentic_checks::prisma_gate::run(&conn, &project, &prefix)?,
            Some(project),
        ),
        CheckAction::CrossModel { project } => (
            agentic_checks::cross_model_gate::run(&conn, &project)?,
            Some(project),
        ),
        CheckAction::ModelReview { project } => (
            agentic_checks::model_review_gate::run(&conn, &project)?,
            Some(project),
        ),
        CheckAction::UndefinedTerms { project, prefix } => (
            agentic_checks::undefined_terms::run(&conn, &project, &prefix)?,
            Some(project),
        ),
        CheckAction::Temporal { project, max_year } => (
            agentic_checks::temporal_gate::run(&conn, &project, max_year)?,
            Some(project),
        ),
        CheckAction::GroundTruth { project } => (
            agentic_checks::ground_truth_gate::run(&conn, &project)?,
            Some(project),
        ),
        CheckAction::Compliance { project } => (
            agentic_checks::compliance_gate::run(&conn, &project)?,
            Some(project),
        ),
        CheckAction::Sprint { project } => (
            agentic_checks::sprint_gate::run(&conn, &project)?,
            Some(project),
        ),
        CheckAction::Predatory { project } => (
            agentic_checks::predatory_gate::run(&conn, &project)?,
            Some(project),
        ),
        CheckAction::Reproducibility { project } => (
            agentic_checks::reproducibility_gate::run(&conn, &project)?,
            Some(project),
        ),
        CheckAction::Integrity { project } => (
            agentic_checks::integrity_gate::run(&conn, &project)?,
            Some(project),
        ),
        CheckAction::FigureQuality { project } => (
            agentic_checks::figure_quality_gate::run(&conn, &project)?,
            Some(project),
        ),
        CheckAction::FigureSizing { project, root } => (
            agentic_checks::figure_sizing_gate::run(&conn, &project, &root)?,
            Some(project),
        ),
        CheckAction::Disclosure { project, venue } => (
            agentic_checks::disclosure_gate::run(&conn, &project, venue.as_deref())?,
            Some(project),
        ),
        CheckAction::RrMatrix { project } => (
            agentic_checks::rr_matrix_gate::run(&conn, &project)?,
            Some(project),
        ),
        CheckAction::Calibration { project } => (
            agentic_checks::calibration_gate::run(&conn, &project)?,
            Some(project),
        ),
        CheckAction::Freshness {
            project,
            max_age_months,
        } => {
            use chrono::Datelike;
            let now = chrono::Local::now();
            (
                agentic_checks::freshness_gate::run(
                    &conn,
                    &project,
                    u32::try_from(now.year()).unwrap_or(2026),
                    now.month(),
                    max_age_months,
                )?,
                Some(project),
            )
        }
        CheckAction::PageBoundary {
            project,
            prefix,
            max_pages,
            words_per_page,
            paths_from_manifest,
            book_key,
            body_from,
            body_to,
        } => {
            let report = match (paths_from_manifest, book_key) {
                (Some(manifest_path), Some(key)) => {
                    let all_paths = load_manifest_paths(&conn, &project, &manifest_path, &key)?;
                    // ADR-0050 §3 (G3): the FHNW 60-page cap applies to the
                    // *body* only. Filter the manifest paths down to numbered
                    // body chapters when measuring the thesis profile; for
                    // every other manifest key, keep the full path list (the
                    // bookkit-C body-cap rule does not apply to campaigns or
                    // dimensions).
                    let filtered: Vec<String> = if key == "master_thesis" {
                        all_paths
                            .iter()
                            .filter(|p| is_thesis_body_chapter(p))
                            .cloned()
                            .collect()
                    } else {
                        all_paths.clone()
                    };
                    let paths_ref: Vec<&str> = filtered.iter().map(String::as_str).collect();
                    agentic_checks::page_boundary_gate::run_scoped(
                        &conn,
                        &project,
                        agentic_checks::page_boundary_gate::Scope::Paths {
                            book_key: &key,
                            paths: &paths_ref,
                            body_from: body_from.as_deref(),
                            body_to: body_to.as_deref(),
                        },
                        max_pages,
                        words_per_page,
                    )?
                }
                _ => agentic_checks::page_boundary_gate::run_scoped(
                    &conn,
                    &project,
                    agentic_checks::page_boundary_gate::Scope::Prefix(&prefix),
                    max_pages,
                    words_per_page,
                )?,
            };
            (report, Some(project))
        }
        CheckAction::Tree {
            project,
            root,
            prefix,
        } => (
            agentic_checks::tree_integrity::run(&conn, &project, &root, &prefix)?,
            Some(project),
        ),
        CheckAction::RenderFidelity {
            project,
            rendered_docx,
            proposal_docx,
        } => (
            agentic_checks::render_fidelity_gate::run(
                rendered_docx.as_deref(),
                proposal_docx.as_deref(),
            )?,
            Some(project),
        ),
        CheckAction::TocCoverage {
            project,
            snapshot_dir,
        } => (
            agentic_checks::toc_coverage_gate::run(&conn, &project, &snapshot_dir)?,
            Some(project),
        ),
        CheckAction::Parity {
            project,
            book,
            reference,
            current,
            html_report,
        } => {
            let cur_path = current.unwrap_or_else(|| {
                std::path::PathBuf::from(format!("snapshots/latest/{book}.docx"))
            });
            // Route by book key: master_thesis_bookkit uses the thesis reference
            // targets + ByteParityDiff (per W1-B); other books use the original
            // AI-Norms targets via run_parity. Default lang to "en" — non-EN
            // routing can come via a future --lang flag (ADR-0061 §3.2).
            let book_kind = agentic_checks::parity::BookKind::from_book_key(&book);
            let parity_report = agentic_checks::parity::run_parity_for_book(
                book_kind, "en", &reference, &cur_path,
            )?;
            if let Some(path) = html_report.as_ref() {
                agentic_checks::parity_report::write_html(&parity_report, path)?;
            }
            (
                agentic_checks::parity::into_check_report(&parity_report),
                Some(project),
            )
        }
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

    Ok((report, project))
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

/// Is `path` a numbered FHNW master-thesis BODY chapter
/// (`thesis/fhnw_<N>_*.md` with N ∈ 1..=9)?
///
/// ADR-0050 §3 (G3): the FHNW 60-page cap covers body chapters only.
/// Front-matter (title page, declarations, management summary, acronyms)
/// and back-matter (appendix, list of figures/tables, bibliography,
/// AI-tools disclosure) do NOT count toward the cap. The numbered-body
/// pattern is the conventional FHNW-thesis chapter prefix (1 Introduction,
/// 2 Theory, …, 7 Personal Reflection); chapters 0 / 00 are reserved for
/// FHNW front-matter (title / decls / mgmt-summary) and chapter 99 for
/// back-matter (AI-tools disclosure, etc.).
///
/// Used by `check page-boundary` when invoked with
/// `--paths-from-manifest --book-key master_thesis`. Other books keep
/// the full path list (the FHNW body-cap rule is thesis-specific).
fn is_thesis_body_chapter(path: &str) -> bool {
    // Match `thesis/fhnw_<N>_…` where N is a SINGLE non-zero digit.
    // Multi-digit (e.g. `fhnw_00_`, `fhnw_99_`) and zero (`fhnw_0_`) are
    // not body chapters.
    let Some(stem) = path.strip_prefix("thesis/fhnw_") else {
        return false;
    };
    let mut chars = stem.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    let Some(second) = chars.next() else {
        return false;
    };
    matches!(first, '1'..='9') && second == '_'
}

#[cfg(test)]
mod tests {
    use super::is_thesis_body_chapter;

    #[test]
    fn body_chapter_recognition_covers_FHNW_pattern() {
        // Numbered body 1..=7 (current thesis) — all true.
        for n in 1..=9 {
            assert!(is_thesis_body_chapter(&format!(
                "thesis/fhnw_{n}_anything.md"
            )));
        }
        // Front-matter (00 / 0) — all false.
        assert!(!is_thesis_body_chapter("thesis/fhnw_00_title_page.md"));
        assert!(!is_thesis_body_chapter(
            "thesis/fhnw_00_declaration_of_originality.md"
        ));
        assert!(!is_thesis_body_chapter(
            "thesis/fhnw_00_compliance_declaration_photon_os.md"
        ));
        assert!(!is_thesis_body_chapter(
            "thesis/fhnw_0_management_summary.md"
        ));
        // Back-matter (99) — false.
        assert!(!is_thesis_body_chapter(
            "thesis/fhnw_99_ai_tools_disclosure.md"
        ));
        // Other prefixes — false.
        assert!(!is_thesis_body_chapter(
            "out/sources/frontmatter/acronyms.md"
        ));
        assert!(!is_thesis_body_chapter(
            "out/sources/Dimensions_bibliography_EN.md"
        ));
        assert!(!is_thesis_body_chapter(
            "out/sources/frontmatter/appendix_research_prompts.md"
        ));
        // Edge cases.
        assert!(!is_thesis_body_chapter("thesis/fhnw_"));
        assert!(!is_thesis_body_chapter(""));
        assert!(!is_thesis_body_chapter("random.md"));
    }
}

/// Read a bookkit manifest from the project's content store and return the
/// ordered chapter path list for `book_key`. Used by the scoped variants of
/// `check page-boundary` and `check bookkit` so the gate measures exactly
/// what the rendered book composes.
///
/// The manifest path is the DB path (the same one the book engine reads),
/// resolved by the project worktree — NOT a filesystem path.
fn load_manifest_paths(
    conn: &rusqlite::Connection,
    project: &str,
    manifest_path: &Path,
    book_key: &str,
) -> Result<Vec<String>> {
    let manifest_db_path = manifest_path.to_string_lossy().replace('\\', "/");
    let blob = agentic_core::worktree::read_at(conn, project, &manifest_db_path)?;
    let text = String::from_utf8(blob.content)?;
    let json: serde_json::Value = serde_json::from_str(&text)?;
    let books = json
        .get("books")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow::anyhow!("manifest has no `books` array"))?;
    for b in books {
        if b.get("key").and_then(|v| v.as_str()) == Some(book_key) {
            let chapters = b
                .get("chapters")
                .and_then(|v| v.as_array())
                .ok_or_else(|| anyhow::anyhow!("book '{book_key}' has no `chapters` array"))?;
            return chapters
                .iter()
                .map(|c| {
                    c.as_str()
                        .map(str::to_string)
                        .ok_or_else(|| anyhow::anyhow!("chapter is not a string"))
                })
                .collect();
        }
    }
    anyhow::bail!("book key '{book_key}' not found in manifest {manifest_db_path}")
}
