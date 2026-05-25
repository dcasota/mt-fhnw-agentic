//! `agentic check prisma` — PRISMA claim→source coverage gate (ADR-0020/0026).
//!
//! Ports the claim→source map from `agentic prisma` into a gate: every in-text
//! citation key in the deliverable markdown must map to a `literature_corpus`
//! reference. An unmapped key surfaces a WARN `PRISMA_UNCOVERED` (advisory — the
//! `citations` gate is the blocking authority); an INFO summary reports the
//! `<mapped>/<total>` claims mapped. PASS when every claim maps.

use std::collections::HashSet;

use anyhow::Result;
use rusqlite::Connection;
use serde_json::Value;

use agentic_core::passport::{self, Section};
use agentic_core::worktree;

use crate::citation_tracker::extract_inline_keys;
use crate::{CheckReport, Finding, Severity};

// Same source-material / governance prefixes the citation gate skips.
const SKIP: &[&str] = &[
    "archive/",
    "proposal/",
    "emailresearch/",
    "inbox/",
    "refs/",
    "specs/",
];

/// PRISMA systematic-review protocol items (ADR-0044). `(label, &[keyword,…])`;
/// scored only when an SR context is detected. Operationalised coverage
/// heuristics, not the verbatim PRISMA-2020 wording.
const SR_PROTOCOL: &[(&str, &[&str])] = &[
    (
        "Eligibility criteria",
        &["eligibility", "inclusion criteria", "exclusion criteria"],
    ),
    (
        "Information sources",
        &["information sources", "databases searched", "data sources"],
    ),
    (
        "Search strategy",
        &[
            "search strategy",
            "search string",
            "query string",
            "boolean",
        ],
    ),
    (
        "Selection process",
        &["selection process", "screening", "screened"],
    ),
    (
        "Data collection",
        &["data collection", "data extraction", "charting"],
    ),
    (
        "Risk of bias",
        &["risk of bias", "quality assessment", "rob"],
    ),
    (
        "Synthesis methods",
        &["synthesis method", "meta-analysis", "narrative synthesis"],
    ),
    (
        "Flow of records",
        &[
            "records identified",
            "records screened",
            "flow diagram",
            "included studies",
        ],
    ),
    (
        "Certainty assessment",
        &["certainty", "grade", "confidence in"],
    ),
];

/// SR-context markers — only then is the protocol scored.
const SR_MARKERS: &[&str] = &[
    "systematic review",
    "prisma",
    "inclusion criteria",
    "risk of bias",
    "search strategy",
];

/// `(has_sr_context, missing_protocol_items)` for `text`.
#[must_use]
pub fn sr_protocol_coverage(text: &str) -> (bool, Vec<&'static str>) {
    let lower = text.to_lowercase();
    let has_sr = SR_MARKERS.iter().any(|m| lower.contains(m));
    let missing = SR_PROTOCOL
        .iter()
        .filter(|(_, kw)| !kw.iter().any(|k| lower.contains(k)))
        .map(|(label, _)| *label)
        .collect();
    (has_sr, missing)
}

/// Run the PRISMA coverage gate over the deliverable markdown (`prefix`).
pub fn run(conn: &Connection, project: &str, prefix: &str) -> Result<CheckReport> {
    let mut findings = Vec::new();

    // Reference universe: literature_corpus citation keys (lower-cased).
    let corpus: HashSet<String> = passport::current(conn, project, Section::LiteratureCorpus)?
        .iter()
        .filter_map(|e| serde_json::from_str::<Value>(&e.payload_json).ok())
        .filter_map(|v| {
            v.get("citation_key")
                .and_then(Value::as_str)
                .map(str::to_lowercase)
        })
        .collect();

    let mut total = 0usize;
    let mut mapped = 0usize;
    let mut sr_corpus = String::new();

    for (path, _sha) in worktree::list(conn, project, prefix)? {
        let is_md = std::path::Path::new(&path)
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("md"));
        if !is_md || path.contains("_resolved") || SKIP.iter().any(|p| path.starts_with(p)) {
            continue;
        }
        let blob = worktree::read_at(conn, project, &path)?;
        let text = String::from_utf8_lossy(&blob.content);
        sr_corpus.push_str(&text);
        sr_corpus.push('\n');
        // `extract_inline_keys` returns a per-document set; report the first line
        // where each unmapped key appears for a useful `file:line` location.
        let keys = extract_inline_keys(&text);
        for key in keys {
            total += 1;
            if corpus.contains(&key) {
                mapped += 1;
            } else {
                let location = first_line_with(&text, &key)
                    .map_or_else(|| path.clone(), |l| format!("{path}:{l}"));
                findings.push(Finding {
                    category: "PRISMA_UNCOVERED".into(),
                    severity: Severity::Warn,
                    message: format!("in-text citation '{key}' has no reference-list/corpus entry"),
                    location: Some(location),
                });
            }
        }
    }

    findings.push(Finding {
        category: "PRISMA_SUMMARY".into(),
        severity: Severity::Info,
        message: format!("{mapped}/{total} claims mapped to a reference"),
        location: Some("prisma".into()),
    });

    // SR-protocol coverage (ADR-0044) — scored only when an SR context exists.
    let (has_sr, missing) = sr_protocol_coverage(&sr_corpus);
    if has_sr && !missing.is_empty() {
        findings.push(Finding {
            category: "PRISMA_SR_PROTOCOL".into(),
            severity: Severity::Warn,
            message: format!(
                "systematic-review context detected but {} protocol item(s) not evidenced: {}",
                missing.len(),
                missing.join(", ")
            ),
            location: Some(prefix.to_owned()),
        });
    } else if has_sr {
        findings.push(Finding {
            category: "PRISMA_SR_SUMMARY".into(),
            severity: Severity::Info,
            message: "all SR-protocol items evidenced".into(),
            location: Some("prisma".into()),
        });
    }

    Ok(CheckReport::new("prisma", findings))
}

/// First 1-based line whose lower-cased text contains the citation `key`'s
/// author token (the alphabetic prefix). Conservative best-effort locator.
fn first_line_with(text: &str, key: &str) -> Option<usize> {
    let author: String = key.chars().take_while(|c| c.is_alphabetic()).collect();
    if author.is_empty() {
        return None;
    }
    text.lines()
        .enumerate()
        .find(|(_, ln)| ln.to_lowercase().contains(&author))
        .map(|(i, _)| i + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentic_core::{
        db::open_in_memory,
        project::{ProjectKind, create as create_project},
    };

    #[test]
    fn unmapped_key_warns() {
        let conn = open_in_memory().unwrap();
        let pid = create_project(&conn, "T", ProjectKind::Thesis, "en", None).unwrap();
        worktree::put_at(
            &conn,
            &pid,
            "out/sources/ch.md",
            b"Per (Mayer, 2022), no corpus entry exists.",
            "text/markdown",
            Some("en"),
            "u",
            "init",
        )
        .unwrap();
        let report = run(&conn, &pid, "out/sources/").unwrap();
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.category == "PRISMA_UNCOVERED")
        );
    }

    #[test]
    fn sr_protocol_scored_only_with_context() {
        let (ctx, _) = sr_protocol_coverage("A normal chapter about cryptography.");
        assert!(!ctx);
        let (ctx2, missing) =
            sr_protocol_coverage("This systematic review used a search strategy across databases.");
        assert!(ctx2);
        assert!(!missing.is_empty());
        assert!(missing.contains(&"Risk of bias"));
    }

    #[test]
    fn mapped_key_passes() {
        let conn = open_in_memory().unwrap();
        let pid = create_project(&conn, "T", ProjectKind::Thesis, "en", None).unwrap();
        passport::append(
            &conn,
            &pid,
            Section::LiteratureCorpus,
            r#"{"citation_key":"mayer2022"}"#,
            None,
            None,
        )
        .unwrap();
        worktree::put_at(
            &conn,
            &pid,
            "out/sources/ch.md",
            b"Per (Mayer, 2022), all good.",
            "text/markdown",
            Some("en"),
            "u",
            "init",
        )
        .unwrap();
        let report = run(&conn, &pid, "out/sources/").unwrap();
        assert!(
            !report
                .findings
                .iter()
                .any(|f| f.category == "PRISMA_UNCOVERED")
        );
    }
}
