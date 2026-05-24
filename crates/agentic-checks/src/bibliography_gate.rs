//! `agentic check bibliography` — per-dimension APA7 trace-coverage gate.
//!
//! Phase-1 non-repudiation: every dimension must carry at least one
//! `literature_corpus` reference, and every web URL cited in a dimension's
//! source must appear in the bibliography (no orphan web traces).

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use anyhow::Result;
use regex::Regex;
use rusqlite::Connection;
use serde_json::Value;

use agentic_core::passport::{self, Section};
use agentic_core::worktree;

use crate::{CheckReport, Finding, Severity};

static URL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"https?://[^\s)\]<>"']+"#).unwrap());
static DIMPATH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"Dimension_(\d{2})_").unwrap());

/// The dimensions that must each carry a bibliography (1..=11).
const DIMS: std::ops::RangeInclusive<i64> = 1..=11;

fn err(cat: &str, msg: String, loc: &str) -> Finding {
    Finding {
        category: cat.to_string(),
        severity: Severity::Error,
        message: msg,
        location: Some(loc.to_string()),
    }
}

/// Normalise a URL for membership comparison (trim trailing punctuation, lower).
fn norm(u: &str) -> String {
    u.trim_end_matches(['.', ',', ')', ']', '>', '"', '\'', ';'])
        .to_lowercase()
}

pub fn run(conn: &Connection, project: &str, prefix: &str) -> Result<CheckReport> {
    let mut findings = Vec::new();

    // 1. Index the bibliography: per-dimension entry counts + the set of URLs.
    let lit = passport::current(conn, project, Section::LiteratureCorpus)?;
    let mut per_dim: HashMap<i64, usize> = HashMap::new();
    let mut lit_urls: HashSet<String> = HashSet::new();
    for e in &lit {
        let Ok(v) = serde_json::from_str::<Value>(&e.payload_json) else {
            continue;
        };
        if let Some(d) = v.get("dimension").and_then(Value::as_i64) {
            *per_dim.entry(d).or_default() += 1;
        }
        if let Some(u) = v.get("url").and_then(Value::as_str) {
            if !u.is_empty() {
                lit_urls.insert(norm(u));
            }
        }
    }

    // 2. Every dimension must have at least one reference.
    for d in DIMS {
        if per_dim.get(&d).copied().unwrap_or(0) == 0 {
            findings.push(err(
                "NO_BIBLIOGRAPHY",
                format!("dimension {d} has no literature_corpus reference"),
                &format!("dimension {d}"),
            ));
        }
    }

    // 3. No orphan web traces: every URL in a dimension source must be cited in
    //    the bibliography (web/email/user traces fully captured as APA7).
    let entries = worktree::list(conn, project, prefix)?;
    for (path, _sha) in entries
        .iter()
        .filter(|(p, _)| DIMPATH.is_match(p) && p.ends_with("_EN.md") && !p.contains("_resolved"))
    {
        let blob = worktree::read_at(conn, project, path)?;
        let text = String::from_utf8_lossy(&blob.content);
        let mut seen: HashSet<String> = HashSet::new();
        for m in URL.find_iter(&text) {
            let u = norm(m.as_str());
            // skip local figure refs (none are http) and de-dup within the file
            if !seen.insert(u.clone()) {
                continue;
            }
            if !lit_urls.contains(&u) {
                findings.push(err(
                    "ORPHAN_URL",
                    format!("web source not in bibliography: {}", m.as_str()),
                    path,
                ));
            }
        }
    }

    Ok(CheckReport::new("bibliography", findings))
}
