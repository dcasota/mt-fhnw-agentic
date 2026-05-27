//! `agentic check figure-quality` — deterministic figure-hygiene gate (ADR-0044).
//!
//! The ARS "visualization quality" skill scores figures with a VLM 10-point
//! checklist. VLM scoring needs a provider call; this gate enforces the
//! deterministic subset of that checklist over deliverable markdown and leaves
//! the perceptual points to an optional provider-backed extension:
//!
//!   * `FIGURE_NO_ALT`        — an image embed with empty alt text (WARN).
//!   * `FIGURE_NO_CAPTION`    — an image with no "Figure N:" caption nearby (WARN).
//!   * `FIGURE_ORPHAN_REF`    — an in-text "Figure N" with no Figure-N caption
//!                              anywhere (ERROR — a dangling cross-reference).
//!   * `FIGURE_UNREFERENCED`  — a captioned Figure N never referenced in prose
//!                              (WARN).
//!
//! An INFO summary reports the figure/reference counts.

use std::collections::HashSet;
use std::sync::LazyLock;

use anyhow::Result;
use regex::Regex;
use rusqlite::Connection;

use agentic_core::worktree;

use crate::{CheckReport, Finding, Severity};

/// Markdown image: `![alt](src)`.
static IMG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"!\[([^\]]*)\]\(([^)]+)\)").unwrap());
/// A figure caption defining a number: `Figure 3:` / `Figure 3.` / `**Figure 3**`.
static CAPTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?im)figure\s+(\d+)\s*[:.\*]").unwrap());
/// Any "Figure N" mention (caption or in-text reference).
static FIG_REF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)figure\s+(\d+)").unwrap());

/// Figure-hygiene findings for one document.
#[must_use]
pub fn figure_findings(text: &str, path: &str) -> Vec<Finding> {
    let mut out = Vec::new();

    // Image embeds with empty alt + caption proximity.
    for (idx, ln) in text.lines().enumerate() {
        for cap in IMG.captures_iter(ln) {
            let alt = cap.get(1).map_or("", |m| m.as_str()).trim();
            if alt.is_empty() {
                out.push(Finding {
                    category: "FIGURE_NO_ALT".into(),
                    severity: Severity::Warn,
                    message: "image embed has empty alt text".into(),
                    location: Some(format!("{path}:{}", idx + 1)),
                });
            }
        }
    }

    // Caption-defined figure numbers vs. in-text references.
    let defined: HashSet<u32> = CAPTION
        .captures_iter(text)
        .filter_map(|c| c[1].parse::<u32>().ok())
        .collect();
    let referenced: HashSet<u32> = FIG_REF
        .captures_iter(text)
        .filter_map(|c| c[1].parse::<u32>().ok())
        .collect();

    // Orphan: referenced but never defined by a caption.
    for n in referenced.difference(&defined) {
        out.push(Finding {
            category: "FIGURE_ORPHAN_REF".into(),
            severity: Severity::Error,
            message: format!("'Figure {n}' is referenced but no Figure {n} caption exists"),
            location: Some(path.to_owned()),
        });
    }
    // Unreferenced: a captioned figure that prose never points at. A figure is
    // "referenced" only if its number appears more than once (caption + ≥1 ref).
    for n in &defined {
        let mentions = FIG_REF
            .captures_iter(text)
            .filter(|c| c[1].parse::<u32>().ok() == Some(*n))
            .count();
        if mentions <= 1 {
            out.push(Finding {
                category: "FIGURE_UNREFERENCED".into(),
                severity: Severity::Warn,
                message: format!("Figure {n} is captioned but never referenced in the prose"),
                location: Some(path.to_owned()),
            });
        }
    }

    // Caption hygiene: flag only if the document has image(s), defines no
    // "Figure N:" caption, AND at least one image is genuinely *unlabeled*. A
    // descriptive alt text (a phrase: whitespace + ≥ 8 chars) labels an inline
    // illustration, so richly-captioned-by-alt diagrams in reference chapters
    // are not false-flagged; empty/trivial alt is still caught by FIGURE_NO_ALT.
    if defined.is_empty()
        && IMG.is_match(text)
        && IMG
            .captures_iter(text)
            .any(|c| !is_descriptive_alt(c.get(1).map_or("", |m| m.as_str())))
    {
        out.push(Finding {
            category: "FIGURE_NO_CAPTION".into(),
            severity: Severity::Warn,
            message: "document embeds unlabeled image(s) with no 'Figure N:' caption".into(),
            location: Some(path.to_owned()),
        });
    }

    out
}

/// A descriptive alt text labels its image: a phrase (contains whitespace) of
/// at least 8 characters, e.g. "ISO/IEC 38500 governance-of-IT model".
fn is_descriptive_alt(alt: &str) -> bool {
    let a = alt.trim();
    a.chars().count() >= 8 && a.contains(char::is_whitespace)
}

pub fn run(conn: &Connection, project: &str) -> Result<CheckReport> {
    let mut findings = Vec::new();
    let mut imgs = 0usize;
    let mut caps = 0usize;
    for (path, sha) in worktree::list(conn, project, agentic_core::paths::SOURCES_PREFIX)? {
        if !path.ends_with(".md") {
            continue;
        }
        let Ok(blob) = agentic_core::content::blob::get_blob(conn, &sha) else {
            continue;
        };
        let text = String::from_utf8_lossy(&blob.content);
        imgs += IMG.find_iter(&text).count();
        caps += CAPTION.captures_iter(&text).count();
        findings.extend(figure_findings(&text, &path));
    }
    findings.push(Finding {
        category: "FIGURE_SUMMARY".into(),
        severity: Severity::Info,
        message: format!("{imgs} image embed(s), {caps} figure caption(s) scanned (deterministic 4-of-10 checklist; VLM points optional)"),
        location: Some("figure_quality".into()),
    });
    Ok(CheckReport::new("figure_quality", findings))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_alt_flagged() {
        let f = figure_findings("![](fig1.png)\n\nFigure 1: A plot. See Figure 1.", "c.md");
        assert!(f.iter().any(|x| x.category == "FIGURE_NO_ALT"));
    }

    #[test]
    fn orphan_reference_is_error() {
        let f = figure_findings("As shown in Figure 7, the trend holds.", "c.md");
        assert!(f
            .iter()
            .any(|x| x.category == "FIGURE_ORPHAN_REF" && matches!(x.severity, Severity::Error)));
    }

    #[test]
    fn descriptive_alt_labels_inline_illustration() {
        // A reference chapter whose images all carry descriptive alt text and
        // no "Figure N" cross-reference must not be flagged NO_CAPTION.
        let md = "Text.\n\n![ISO/IEC 38500 governance-of-IT model](iso.png)\n\nMore text.\n";
        let f = figure_findings(md, "norms/06.md");
        assert!(!f.iter().any(|x| x.category == "FIGURE_NO_CAPTION"));
        assert!(!f.iter().any(|x| x.category == "FIGURE_NO_ALT"));
    }

    #[test]
    fn unlabeled_image_without_caption_flagged() {
        // A short/trivial alt and no caption is a genuinely unlabeled figure.
        let f = figure_findings("![plot](f.png)\n\nSome prose.\n", "c.md");
        assert!(f.iter().any(|x| x.category == "FIGURE_NO_CAPTION"));
    }

    #[test]
    fn referenced_caption_ok() {
        let f = figure_findings(
            "![plot](f.png)\n\nFigure 1: A. Discussed in Figure 1 below.",
            "c.md",
        );
        assert!(!f.iter().any(|x| x.category == "FIGURE_ORPHAN_REF"));
        assert!(!f.iter().any(|x| x.category == "FIGURE_UNREFERENCED"));
    }
}
