//! `agentic prisma` — systematic claim→source map (ADR-0020/0026).
//!
//! Emits a PRISMA-style coverage report over the authored deliverables: each
//! in-text citation is mapped to a `literature_corpus` entry, and overall
//! number/citation coverage is summarised. Complements the `citations` gate
//! (which fails on a *missing* ref) by producing the auditable map artefact.

use std::collections::HashSet;

use anyhow::Result;
use serde_json::{Value, json};

use agentic_checks::citation_tracker::extract_inline_keys;
use agentic_core::passport::{self, Section};
use agentic_core::worktree;

use crate::cli::PrismaAction;

// Same source-material / governance prefixes the citation gate skips.
const SKIP: &[&str] = &[
    "archive/",
    "proposal/",
    "emailresearch/",
    "inbox/",
    "refs/",
    "specs/",
];

pub fn run(db_path: &std::path::Path, action: PrismaAction, json_out: bool) -> Result<()> {
    let PrismaAction::Build { project, prefix } = action;
    let conn = agentic_core::db::open(db_path)?;

    // Reference universe: literature_corpus citation keys.
    let corpus: HashSet<String> = passport::current(&conn, &project, Section::LiteratureCorpus)?
        .iter()
        .filter_map(|e| serde_json::from_str::<Value>(&e.payload_json).ok())
        .filter_map(|v| {
            v.get("citation_key")
                .and_then(Value::as_str)
                .map(str::to_lowercase)
        })
        .collect();

    let mut total_keys = 0usize;
    let mut mapped = 0usize;
    let mut unmapped: Vec<(String, String)> = Vec::new(); // (key, file)
    let mut docs = 0usize;

    for (path, _sha) in worktree::list(&conn, &project, &prefix)? {
        if !path.ends_with(".md") || path.contains("_resolved") || SKIP.iter().any(|p| path.starts_with(p)) {
            continue;
        }
        docs += 1;
        let blob = worktree::read_at(&conn, &project, &path)?;
        let text = String::from_utf8_lossy(&blob.content);
        for key in extract_inline_keys(&text) {
            total_keys += 1;
            if corpus.contains(&key) {
                mapped += 1;
            } else {
                unmapped.push((key, path.clone()));
            }
        }
    }
    unmapped.sort();
    unmapped.dedup();
    let coverage = if total_keys == 0 {
        100.0
    } else {
        (mapped as f64 / total_keys as f64) * 100.0
    };

    if json_out {
        println!(
            "{}",
            json!({
                "docs": docs, "references": corpus.len(),
                "in_text_citations": total_keys, "mapped": mapped,
                "unmapped": unmapped.len(), "coverage_pct": coverage
            })
        );
        return Ok(());
    }
    println!("## PRISMA claim→source map (ADR-0026)\n");
    println!("- Deliverables scanned: {docs}");
    println!("- Reference universe (literature_corpus): {}", corpus.len());
    println!("- In-text citation occurrences: {total_keys}");
    println!("- Mapped to a reference: {mapped} ({coverage:.1}%)");
    println!("- Unmapped (would fail `check citations`): {}", unmapped.len());
    if !unmapped.is_empty() {
        println!("\n| in-text key | first seen in |\n|---|---|");
        for (k, f) in unmapped.iter().take(40) {
            println!("| {k} | {} |", f.rsplit('/').next().unwrap_or(f));
        }
    }
    Ok(())
}
