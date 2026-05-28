//! Per-section ranking summary (P-4 in the perception-improvement plan).
//!
//! Walks `claim_audit_results` and, for each section identified by the
//! `audit_profile::classify_path` map, counts:
//!   * placements: `thesis_main`, `thesis_appendix`, `lowrankings`, `other`,
//!     `(none)` (where the CAR carries no `placement` key);
//!   * tiers: Critical / High / Medium (ADR-0046 §2) where the CAR carries
//!     a `tier` key;
//!   * model_review accept / revise / exclude (latest-wins per path).
//!
//! Read-only against the passport. Reflects what the cascade left behind.

use std::collections::BTreeMap;

use anyhow::Result;
use rusqlite::Connection;
use serde::Serialize;
use serde_json::Value;

use crate::audit_profile::{Section, classify_path};
use crate::passport::{self, Section as PassportSection};

#[derive(Debug, Clone, Default, Serialize)]
pub struct PlacementCounts {
    pub thesis_main: usize,
    pub thesis_appendix: usize,
    pub lowrankings: usize,
    pub other: usize,
    pub none: usize,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct TierCounts {
    pub critical: usize,
    pub high: usize,
    pub medium: usize,
    pub none: usize,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ModelReviewCounts {
    pub accept: usize,
    pub revise: usize,
    pub exclude: usize,
    pub other: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct SectionRanking {
    pub section: Section,
    pub total_cars: usize,
    pub placements: PlacementCounts,
    pub tiers: TierCounts,
    pub model_review: ModelReviewCounts,
}

/// Latest-wins-per-path model_review verdict. Mirrors
/// `agentic_core::review::excluded_paths()` semantics so the count here
/// matches the count surfaced by `check model-review`.
fn latest_review_verdicts(entries: &[passport::Entry]) -> BTreeMap<String, String> {
    let mut latest: BTreeMap<String, (i64, String)> = BTreeMap::new();
    for e in entries {
        let Ok(v) = serde_json::from_str::<Value>(&e.payload_json) else {
            continue;
        };
        if v.get("kind").and_then(Value::as_str) != Some("model_review") {
            continue;
        }
        let Some(path) = v.get("path").and_then(Value::as_str) else {
            continue; // skip rankings-scope reviews
        };
        let assessment = v
            .get("assessment")
            .and_then(Value::as_str)
            .unwrap_or("other")
            .to_string();
        let cur = latest.get(path).map(|(id, _)| *id).unwrap_or(0);
        if e.id > cur {
            latest.insert(path.to_string(), (e.id, assessment));
        }
    }
    latest.into_iter().map(|(p, (_, a))| (p, a)).collect()
}

/// Compute the per-section ranking summary for a project.
pub fn compute(conn: &Connection, project_id: &str) -> Result<Vec<SectionRanking>> {
    let entries = passport::current(conn, project_id, PassportSection::ClaimAuditResults)?;
    let mut by_section: BTreeMap<Section, SectionRanking> = BTreeMap::new();
    for sec in [
        Section::MasterThesis,
        Section::Dimensions,
        Section::Campaigns,
        Section::Projects,
        Section::StudentNotes,
        Section::AgenticHandbook,
        Section::Audit,
        Section::Norms,
        Section::Frontmatter,
        Section::Other,
    ] {
        by_section.insert(
            sec,
            SectionRanking {
                section: sec,
                total_cars: 0,
                placements: PlacementCounts::default(),
                tiers: TierCounts::default(),
                model_review: ModelReviewCounts::default(),
            },
        );
    }

    // Latest review verdicts (per-path, latest-wins).
    let reviews = latest_review_verdicts(&entries);
    for (path, assessment) in &reviews {
        let sec = classify_path(path);
        let entry = by_section.get_mut(&sec).expect("section seeded");
        match assessment.as_str() {
            "accept" => entry.model_review.accept += 1,
            "revise" => entry.model_review.revise += 1,
            "exclude" => entry.model_review.exclude += 1,
            _ => entry.model_review.other += 1,
        }
    }

    // Placement / tier counts walk every CAR (not just model_review).
    for e in &entries {
        let Ok(v) = serde_json::from_str::<Value>(&e.payload_json) else {
            continue;
        };
        // Find a path to classify: model_review carries `path`; older CARs
        // carry `provenance.sources` (list of source paths) — take the first
        // that looks like an out/sources path. Fall back to Other.
        let mut path: Option<String> = v.get("path").and_then(Value::as_str).map(str::to_string);
        if path.is_none() {
            if let Some(arr) = v.pointer("/provenance/sources").and_then(Value::as_array) {
                for s in arr {
                    if let Some(s) = s.as_str() {
                        if s.contains("/sources/")
                            || s.starts_with("thesis/")
                            || s.starts_with("out/sources/")
                        {
                            path = Some(s.to_string());
                            break;
                        }
                    }
                }
            }
        }
        let sec = path.as_deref().map(classify_path).unwrap_or(Section::Other);
        let row = by_section.get_mut(&sec).expect("section seeded");
        row.total_cars += 1;
        let placement = v
            .get("placement")
            .and_then(Value::as_str)
            .unwrap_or("(none)");
        match placement {
            "thesis_main" => row.placements.thesis_main += 1,
            "thesis_appendix" => row.placements.thesis_appendix += 1,
            "lowrankings" => row.placements.lowrankings += 1,
            "(none)" => row.placements.none += 1,
            _ => row.placements.other += 1,
        }
        let tier = v.get("tier").and_then(Value::as_str).map(str::to_lowercase);
        match tier.as_deref() {
            Some("critical") => row.tiers.critical += 1,
            Some("high") => row.tiers.high += 1,
            Some("medium") => row.tiers.medium += 1,
            _ => row.tiers.none += 1,
        }
    }
    Ok(by_section.into_values().collect())
}

#[must_use]
pub fn render_markdown(rs: &[SectionRanking]) -> String {
    let mut s = String::new();
    s.push_str("# Ranking Summary\n\nPer-section ADR-0046 acceptance — one row per section with placement, tier, and model-review aggregates from `claim_audit_results`.\n");
    s.push_str("\n| Section | CARs | thesis_main | thesis_appendix | lowrankings | other | (none) | Critical | High | Medium | tier-(none) | accept | revise | exclude |\n");
    s.push_str("|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|\n");
    for r in rs {
        s.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            r.section.slug(),
            r.total_cars,
            r.placements.thesis_main,
            r.placements.thesis_appendix,
            r.placements.lowrankings,
            r.placements.other,
            r.placements.none,
            r.tiers.critical,
            r.tiers.high,
            r.tiers.medium,
            r.tiers.none,
            r.model_review.accept,
            r.model_review.revise,
            r.model_review.exclude,
        ));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use crate::passport::{Section as PassportSection, append};
    use crate::project::{ProjectKind, create as create_project};

    fn add(c: &Connection, p: &str, payload: &str) {
        append(
            c,
            p,
            PassportSection::ClaimAuditResults,
            payload,
            None,
            None,
        )
        .unwrap();
    }

    #[test]
    fn counts_per_section_split() {
        let c = open_in_memory().unwrap();
        let p = create_project(&c, "T", ProjectKind::Thesis, "en", None).unwrap();
        // Two model_review entries on one thesis path: accept + revise; latest wins.
        add(
            &c,
            &p,
            r#"{"kind":"model_review","path":"thesis/fhnw_2_theory.md","assessment":"accept"}"#,
        );
        add(
            &c,
            &p,
            r#"{"kind":"model_review","path":"thesis/fhnw_2_theory.md","assessment":"revise"}"#,
        );
        // One CAR on a campaign path with placement + tier.
        add(
            &c,
            &p,
            r#"{"kind":"ranking","path":"out/sources/Campaign_07_iso42001_first_mover_EN.md","placement":"thesis_main","tier":"Critical"}"#,
        );
        // One CAR with provenance.sources only (no path).
        add(
            &c,
            &p,
            r#"{"kind":"ranking","placement":"lowrankings","provenance":{"sources":["out/sources/projects/PT-C09-6_x.md"]}}"#,
        );

        let summary = compute(&c, &p).unwrap();
        let thesis = summary
            .iter()
            .find(|r| r.section == Section::MasterThesis)
            .unwrap();
        // Latest-wins-per-path: 2 model_review entries on the same path -> 1 revise.
        assert_eq!(thesis.model_review.accept, 0);
        assert_eq!(thesis.model_review.revise, 1);
        // Both entries count as CARs (total_cars walks ALL entries).
        assert_eq!(thesis.total_cars, 2);

        let camp = summary
            .iter()
            .find(|r| r.section == Section::Campaigns)
            .unwrap();
        assert_eq!(camp.total_cars, 1);
        assert_eq!(camp.placements.thesis_main, 1);
        assert_eq!(camp.tiers.critical, 1);

        let proj = summary
            .iter()
            .find(|r| r.section == Section::Projects)
            .unwrap();
        assert_eq!(proj.total_cars, 1);
        assert_eq!(proj.placements.lowrankings, 1);
    }
}
