//! `agentic check compliance` — contamination-report consolidation gate.
//!
//! The `contamination` gate writes a `contamination_status` compliance report
//! (with a PRISMA disposition) to the passport. This gate reads the NEWEST such
//! report and consolidates its verdict:
//!   * no report at all → WARN `COMPLIANCE_NO_REPORT` (run `check contamination`),
//!   * `fabricated > 0`  → ERROR `COMPLIANCE_FABRICATED` (blocks),
//!   * `suspect > 0`     → WARN `COMPLIANCE_SUSPECT`,
//!   * otherwise (matched / not-indexed only) → PASS.
//!
//! An INFO summary always echoes the PRISMA buckets.

use anyhow::Result;
use rusqlite::Connection;
use serde_json::Value;

use agentic_core::passport::{self, Section};
use agentic_core::worktree;

use crate::{CheckReport, Finding, Severity};

/// Operationalised PRISMA-trAIce AI-disclosure checklist (ADR-0044). Each item
/// is `(label, &[keyword, …])`; an item counts as covered when any keyword
/// appears in the disclosure text. These are coverage heuristics, not a verbatim
/// reproduction of the official wording.
const PRISMA_TRAICE: &[(&str, &[&str])] = &[
    (
        "AI tool identity/version",
        &["model version", "gpt-", "claude", "llm used", "tool:"],
    ),
    (
        "AI role/purpose",
        &["role of", "used to", "purpose of the ai", "ai was used for"],
    ),
    ("Prompts/queries disclosed", &["prompt", "query", "queries"]),
    (
        "Human oversight",
        &[
            "human oversight",
            "human review",
            "author reviewed",
            "verified by the author",
        ],
    ),
    (
        "Date of AI use",
        &["date of use", "accessed", "as of 20", "between 20"],
    ),
    (
        "Output verification",
        &["verified", "fact-check", "cross-check", "validated"],
    ),
    (
        "Data provided to AI",
        &[
            "data provided",
            "input data",
            "uploaded",
            "supplied to the model",
        ],
    ),
    (
        "Search assistance",
        &["search", "literature discovery", "screening"],
    ),
    (
        "Extraction assistance",
        &["extraction", "extracted", "data charting"],
    ),
    (
        "Synthesis assistance",
        &["synthesis", "summari", "drafting"],
    ),
    (
        "Bias/limitations of AI",
        &["bias", "limitation", "hallucinat"],
    ),
    (
        "Reproducibility settings",
        &[
            "temperature",
            "seed",
            "settings",
            "parameters",
            "deterministic",
        ],
    ),
    ("Error handling", &["error", "correction", "mistake"]),
    ("Ethics/consent", &["ethic", "consent", "irb", "privacy"]),
    (
        "Funding/conflict re AI",
        &["funding", "conflict of interest", "no competing"],
    ),
    (
        "Accountability statement",
        &["accountab", "responsib", "the authors take"],
    ),
    (
        "Reproducibility of AI use",
        &["reproduc", "rerun", "re-run", "replicat"],
    ),
];

/// RAISE four-principle coverage (operationalised, ADR-0044).
const RAISE: &[(&str, &[&str])] = &[
    ("Transparency", &["transparen", "disclos"]),
    ("Accountability", &["accountab", "responsib"]),
    ("Fairness/bias", &["fair", "bias", "equit"]),
    (
        "Human oversight",
        &[
            "human oversight",
            "human-in-the-loop",
            "hitl",
            "human review",
        ],
    ),
];

/// Disclosure context markers — only then do we score the checklist.
const DISCLOSURE_MARKERS: &[&str] = &[
    "ai disclosure",
    "use of generative ai",
    "use of ai",
    "prisma-traice",
    "prisma-trace",
    "raise",
    "ai-assistance statement",
    "artificial intelligence was used",
];

/// `(has_context, missing_prisma_labels, missing_raise_labels)` for `text`.
#[must_use]
pub fn disclosure_coverage(text: &str) -> (bool, Vec<&'static str>, Vec<&'static str>) {
    let lower = text.to_lowercase();
    let has_context = DISCLOSURE_MARKERS.iter().any(|m| lower.contains(m));
    let missing_prisma = PRISMA_TRAICE
        .iter()
        .filter(|(_, kw)| !kw.iter().any(|k| lower.contains(k)))
        .map(|(label, _)| *label)
        .collect();
    let missing_raise = RAISE
        .iter()
        .filter(|(_, kw)| !kw.iter().any(|k| lower.contains(k)))
        .map(|(label, _)| *label)
        .collect();
    (has_context, missing_prisma, missing_raise)
}

pub fn run(conn: &Connection, project: &str) -> Result<CheckReport> {
    let mut findings = Vec::new();
    let reports = passport::current(conn, project, Section::ComplianceReports)?;

    // Newest contamination_status report (entries are id-ordered ascending).
    let latest = reports
        .iter()
        .rev()
        .filter_map(|e| serde_json::from_str::<Value>(&e.payload_json).ok())
        .find(|v| v.get("report").and_then(Value::as_str) == Some("contamination_status"));

    let Some(v) = latest else {
        findings.push(Finding {
            category: "COMPLIANCE_NO_REPORT".into(),
            severity: Severity::Warn,
            message: "no contamination_status report — run `agentic check contamination`".into(),
            location: Some("compliance_reports".into()),
        });
        return Ok(CheckReport::new("compliance", findings));
    };

    let prisma = v.get("prisma").cloned().unwrap_or(Value::Null);
    let bucket = |k: &str| prisma.get(k).and_then(Value::as_u64).unwrap_or(0);
    let (matched, not_indexed, suspect, fabricated) = (
        bucket("matched"),
        bucket("not_indexed"),
        bucket("suspect"),
        bucket("fabricated"),
    );

    if fabricated > 0 {
        findings.push(Finding {
            category: "COMPLIANCE_FABRICATED".into(),
            severity: Severity::Error,
            message: format!(
                "{fabricated} fabricated reference(s) in the latest contamination report"
            ),
            location: Some("compliance_reports".into()),
        });
    }
    if suspect > 0 {
        findings.push(Finding {
            category: "COMPLIANCE_SUSPECT".into(),
            severity: Severity::Warn,
            message: format!("{suspect} suspect reference(s) — route to cross-model / HITL"),
            location: Some("compliance_reports".into()),
        });
    }

    findings.push(Finding {
        category: "COMPLIANCE_SUMMARY".into(),
        severity: Severity::Info,
        message: format!(
            "PRISMA buckets: {matched} matched, {not_indexed} not-indexed, {suspect} suspect, {fabricated} fabricated"
        ),
        location: Some("compliance".into()),
    });

    // PRISMA-trAIce-17 + RAISE coverage over the AI-disclosure material (ADR-0044).
    // Only scored when a disclosure context exists; otherwise advisory INFO.
    let mut disclosure = String::new();
    for (path, sha) in worktree::list(conn, project, agentic_core::paths::SOURCES_PREFIX)? {
        if !path.ends_with(".md") {
            continue;
        }
        if let Ok(blob) = agentic_core::content::blob::get_blob(conn, &sha) {
            disclosure.push_str(&String::from_utf8_lossy(&blob.content));
            disclosure.push('\n');
        }
    }
    let (has_context, missing_prisma, missing_raise) = disclosure_coverage(&disclosure);
    if has_context {
        if !missing_prisma.is_empty() {
            findings.push(Finding {
                category: "COMPLIANCE_PRISMA_TRAICE".into(),
                severity: Severity::Warn,
                message: format!(
                    "AI-disclosure present but {} of 17 PRISMA-trAIce items uncovered: {}",
                    missing_prisma.len(),
                    missing_prisma.join(", ")
                ),
                location: Some("out/sources".into()),
            });
        }
        if !missing_raise.is_empty() {
            findings.push(Finding {
                category: "COMPLIANCE_RAISE".into(),
                severity: Severity::Warn,
                message: format!(
                    "RAISE principle(s) not evidenced: {}",
                    missing_raise.join(", ")
                ),
                location: Some("out/sources".into()),
            });
        }
        findings.push(Finding {
            category: "COMPLIANCE_DISCLOSURE_SUMMARY".into(),
            severity: Severity::Info,
            message: format!(
                "AI-disclosure coverage: {}/17 PRISMA-trAIce, {}/4 RAISE",
                17 - missing_prisma.len(),
                4 - missing_raise.len()
            ),
            location: Some("compliance".into()),
        });
    } else {
        findings.push(Finding {
            category: "COMPLIANCE_DISCLOSURE_SUMMARY".into(),
            severity: Severity::Info,
            message: "no AI-disclosure context found — PRISMA-trAIce/RAISE checklist not scored"
                .into(),
            location: Some("compliance".into()),
        });
    }

    Ok(CheckReport::new("compliance", findings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentic_core::{
        db::open_in_memory,
        project::{ProjectKind, create as create_project},
    };

    #[test]
    fn no_report_warns() {
        let conn = open_in_memory().unwrap();
        let pid = create_project(&conn, "T", ProjectKind::Thesis, "en", None).unwrap();
        let report = run(&conn, &pid).unwrap();
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.category == "COMPLIANCE_NO_REPORT")
        );
    }

    #[test]
    fn fabricated_blocks() {
        let conn = open_in_memory().unwrap();
        let pid = create_project(&conn, "T", ProjectKind::Thesis, "en", None).unwrap();
        passport::append(
            &conn,
            &pid,
            Section::ComplianceReports,
            r#"{"report":"contamination_status","prisma":{"matched":3,"not_indexed":0,"suspect":1,"fabricated":2}}"#,
            None,
            None,
        )
        .unwrap();
        let report = run(&conn, &pid).unwrap();
        assert_eq!(report.verdict, crate::Verdict::Fail);
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.category == "COMPLIANCE_FABRICATED")
        );
    }

    #[test]
    fn disclosure_coverage_scores_only_with_context() {
        // No context → nothing flagged.
        let (ctx, _, _) = disclosure_coverage("A thesis chapter with no disclosure.");
        assert!(!ctx);
        // Context present but sparse → many missing.
        let (ctx2, miss_p, miss_r) =
            disclosure_coverage("AI disclosure: generative AI was used to draft text.");
        assert!(ctx2);
        assert!(!miss_p.is_empty());
        assert!(!miss_r.is_empty());
    }

    #[test]
    fn clean_report_passes() {
        let conn = open_in_memory().unwrap();
        let pid = create_project(&conn, "T", ProjectKind::Thesis, "en", None).unwrap();
        passport::append(
            &conn,
            &pid,
            Section::ComplianceReports,
            r#"{"report":"contamination_status","prisma":{"matched":5,"not_indexed":1,"suspect":0,"fabricated":0}}"#,
            None,
            None,
        )
        .unwrap();
        let report = run(&conn, &pid).unwrap();
        assert_eq!(report.verdict, crate::Verdict::Pass);
    }
}
