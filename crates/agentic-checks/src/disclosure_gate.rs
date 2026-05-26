//! `agentic check disclosure` — AI-disclosure presence + venue match (ADR-0044).
//!
//! The ARS "AI Disclosure Mode" renders venue-specific statements. This gate
//! enforces the auditable property behind it: an AI-use disclosure statement
//! must exist, and when a publication venue/track is named in the deliverables
//! (or passed via `--venue`), the disclosure must be present:
//!
//!   * venue/track named but no disclosure statement → ERROR `DISCLOSURE_MISSING`.
//!   * disclosure present → INFO with the detected venue/track.
//!   * neither venue nor disclosure (e.g. a plain thesis chapter) → WARN
//!     `DISCLOSURE_ABSENT` (advisory: recommend an AI-use statement).

use anyhow::Result;
use rusqlite::Connection;

use agentic_core::worktree;

use crate::{CheckReport, Finding, Severity};

/// Disclosure-statement markers.
const DISCLOSURE_MARKERS: &[&str] = &[
    "ai disclosure",
    "use of generative ai",
    "use of ai",
    "ai-assistance statement",
    "artificial intelligence was used",
    "generative ai was used",
    "declaration of ai",
    "ai usage statement",
];

/// Recognised venues / disclosure tracks.
const VENUES: &[&str] = &[
    "iclr",
    "neurips",
    "nature",
    "science",
    "acl",
    "emnlp",
    "ieee",
    "icmje",
    "prisma-traice",
    "prisma-trace",
    "elsevier",
    "springer",
    "acm",
];

/// `(has_disclosure, detected_venues)` for `text` (lower-cased scan).
#[must_use]
pub fn scan(text: &str) -> (bool, Vec<&'static str>) {
    let lower = text.to_lowercase();
    let has_disclosure = DISCLOSURE_MARKERS.iter().any(|m| lower.contains(m));
    let venues = VENUES
        .iter()
        .filter(|v| lower.contains(**v))
        .copied()
        .collect();
    (has_disclosure, venues)
}

pub fn run(conn: &Connection, project: &str, venue: Option<&str>) -> Result<CheckReport> {
    let mut findings = Vec::new();
    let mut corpus = String::new();
    for (path, sha) in worktree::list(conn, project, agentic_core::paths::SOURCES_PREFIX)? {
        if !path.ends_with(".md") {
            continue;
        }
        if let Ok(blob) = agentic_core::content::blob::get_blob(conn, &sha) {
            corpus.push_str(&String::from_utf8_lossy(&blob.content));
            corpus.push('\n');
        }
    }
    let (has_disclosure, mut venues) = scan(&corpus);
    if let Some(v) = venue {
        let v = v.to_lowercase();
        if let Some(known) = VENUES.iter().find(|k| **k == v) {
            if !venues.contains(known) {
                venues.push(known);
            }
        }
    }

    if !venues.is_empty() && !has_disclosure {
        findings.push(Finding {
            category: "DISCLOSURE_MISSING".into(),
            severity: Severity::Error,
            message: format!(
                "venue/track named ({}) but no AI-disclosure statement found",
                venues.join(", ")
            ),
            location: Some("out/sources".into()),
        });
    } else if has_disclosure {
        findings.push(Finding {
            category: "DISCLOSURE_PRESENT".into(),
            severity: Severity::Info,
            message: if venues.is_empty() {
                "AI-disclosure statement present (no specific venue detected)".into()
            } else {
                format!(
                    "AI-disclosure statement present; venue/track: {}",
                    venues.join(", ")
                )
            },
            location: Some("out/sources".into()),
        });
    } else {
        findings.push(Finding {
            category: "DISCLOSURE_ABSENT".into(),
            severity: Severity::Warn,
            message: "no AI-disclosure statement found — add an AI-use declaration".into(),
            location: Some("out/sources".into()),
        });
    }

    Ok(CheckReport::new("disclosure", findings))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn venue_without_disclosure_errors() {
        let (d, v) = scan("Submitted to NeurIPS 2026. No statement here.");
        assert!(!d);
        assert!(v.contains(&"neurips"));
    }

    #[test]
    fn disclosure_detected() {
        let (d, _) = scan("AI disclosure: generative AI was used to draft sections.");
        assert!(d);
    }
}
