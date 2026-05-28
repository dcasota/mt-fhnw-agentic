//! Per-section audit profile (P-2 in the perception-improvement plan).
//!
//! Joins three sources the runtime already maintains:
//!   1. `audit_verdicts` (one row per gate per run; checkpoint + verdict).
//!   2. `agentic_core::profiles::GATE_CATALOG` (the canonical 26-gate list).
//!   3. A static path → section classifier (this module) that maps a
//!      deliverable path to a logical section (dimensions, campaigns,
//!      master_thesis, student_notes, agentic_handbook, audit, norms,
//!      frontmatter, projects, other).
//!   4. A static gate → ADRs map (this module).
//!
//! The profile gives the operator one-screen visibility of "which gates
//! touch this section and what their last verdict was". It does NOT re-run
//! the gates; it reads what the last cascade left behind.

use std::collections::BTreeMap;

use anyhow::Result;
use rusqlite::Connection;
use serde::Serialize;

/// Section taxonomy used by the per-section profile.
///
/// Mirrors the operator's `Governance-Perception.txt` section axis (A-J) but
/// with names that match the live manifest keys + content paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub enum Section {
    Dimensions,
    Campaigns,
    Projects,
    StudentNotes,
    MasterThesis,
    AgenticHandbook,
    Audit,
    Norms,
    Frontmatter,
    Other,
}

impl Section {
    /// Stable string slug; used in CLI args and JSON output.
    #[must_use]
    pub fn slug(self) -> &'static str {
        match self {
            Section::Dimensions => "dimensions",
            Section::Campaigns => "campaigns",
            Section::Projects => "projects",
            Section::StudentNotes => "student_notes",
            Section::MasterThesis => "master_thesis",
            Section::AgenticHandbook => "agentic_handbook",
            Section::Audit => "audit",
            Section::Norms => "norms",
            Section::Frontmatter => "frontmatter",
            Section::Other => "other",
        }
    }

    /// Parse a slug back to a Section. Returns None for unknown slugs.
    #[must_use]
    pub fn from_slug(s: &str) -> Option<Self> {
        match s {
            "dimensions" => Some(Section::Dimensions),
            "campaigns" => Some(Section::Campaigns),
            "projects" => Some(Section::Projects),
            "student_notes" => Some(Section::StudentNotes),
            "master_thesis" => Some(Section::MasterThesis),
            "agentic_handbook" => Some(Section::AgenticHandbook),
            "audit" => Some(Section::Audit),
            "norms" => Some(Section::Norms),
            "frontmatter" => Some(Section::Frontmatter),
            "other" => Some(Section::Other),
            _ => None,
        }
    }
}

/// Classify a content-store path into the section it belongs to. The mapping
/// mirrors the path-prefix conventions documented in ADR-0045 (three bookkit
/// profiles), ADR-0046 (ranking acceptance levels), and ADR-0048 (out/ as DB
/// source-of-truth). It is intentionally small and prefix-based — the same
/// scheme `crates/agentic/src/commands/review.rs::class_of()` already uses
/// for the model-review pipeline.
#[must_use]
pub fn classify_path(path: &str) -> Section {
    if path.starts_with("thesis/") {
        return Section::MasterThesis;
    }
    if path.starts_with("out/sources/agentic_handbook/") {
        return Section::AgenticHandbook;
    }
    if path.starts_with("out/sources/projects/") {
        return Section::Projects;
    }
    if path.starts_with("out/sources/norms/") {
        return Section::Norms;
    }
    if path.starts_with("out/sources/frontmatter/") {
        return Section::Frontmatter;
    }
    if path.starts_with("out/sources/cascade_audit/")
        || path.starts_with("out/sources/gov_perception_audit/")
        || path.contains("AI_Audit_BOM")
        || path.contains("audit_report")
    {
        return Section::Audit;
    }
    if path.starts_with("out/sources/StudentNotes")
        || path.starts_with("out/sources/Synthesis_campaigns")
    {
        return Section::StudentNotes;
    }
    // The merged dimensions book + per-dimension sources both belong here.
    if path.starts_with("out/sources/Dimension_") || path.starts_with("out/sources/Dimensions_") {
        return Section::Dimensions;
    }
    if path.starts_with("out/sources/Campaign_") {
        return Section::Campaigns;
    }
    Section::Other
}

/// The static gate → ADR map. One row per gate the cascade composes
/// (universal_rules + per-profile additions per ADR-0047 R4). The ADR
/// numbers come from the doc-comments of each gate's source file in
/// `crates/agentic-checks/src/`; treating them as data here makes the
/// audit-profile output self-citing.
#[must_use]
pub fn gate_adrs(gate: &str) -> &'static [&'static str] {
    match gate {
        "self" => &["ADR-0001", "ADR-0023"],
        "tree" => &["ADR-0048"],
        "deliverable" => &["ADR-0036", "ADR-0037", "ADR-0038", "ADR-0044"],
        "writing-quality" | "writing_quality" => &["ADR-0030"],
        "citations" | "citation_tracker" => &["ADR-0007", "ADR-0020", "ADR-0026"],
        "contamination" => &["ADR-0014"],
        "bibliography" => &["ADR-0007", "ADR-0041"],
        "aibom" => &["ADR-0023", "ADR-0039"],
        "docs" => &["ADR-0047"],
        "facts-integrity" | "facts_integrity" => &["ADR-0036", "ADR-0042", "ADR-0044"],
        "i18n" => &["ADR-0043"],
        "bookkit" => &["ADR-0030", "ADR-0045"],
        "prisma" => &["ADR-0020", "ADR-0026"],
        "cross-model" | "cross_model" => &["ADR-0028"],
        "model-review" | "model_review" => &["ADR-0049"],
        "temporal" => &["ADR-0019"],
        "ground-truth" | "ground_truth" => &["ADR-0036"],
        "compliance" => &["ADR-0025"],
        "sprint" => &["ADR-0024"],
        "predatory" => &["ADR-0022"],
        "reproducibility" => &["ADR-0029", "ADR-0039"],
        "integrity" => &["ADR-0044"],
        "figure-quality" | "figure_quality" => &["ADR-0031", "ADR-0044"],
        "disclosure" => &["ADR-0044"],
        "freshness" => &["ADR-0047"],
        "page-boundary" | "page_boundary" => &["ADR-0035", "ADR-0045"],
        "rr-matrix" | "rr_matrix" => &["ADR-0044"],
        "calibration" => &["ADR-0044"],
        "undefined-terms" | "undefined_terms" => &["ADR-0044"],
        "model-review-display" => &["ADR-0049"],
        _ => &[],
    }
}

/// Gates that apply only when rendering the FHNW master thesis (bookkit C).
const THESIS_ONLY: &[&str] = &["page-boundary", "rr-matrix", "calibration"];

/// One row in the per-section profile: a gate's latest verdict + its ADR
/// citations + whether it is thesis-only.
#[derive(Debug, Clone, Serialize)]
pub struct GateStatus {
    pub gate: String,
    pub verdict: Option<String>,
    pub thesis_only: bool,
    pub adrs: Vec<String>,
}

/// One section's profile: name + applicable gate verdicts.
#[derive(Debug, Clone, Serialize)]
pub struct SectionProfile {
    pub section: Section,
    pub gates: Vec<GateStatus>,
}

/// Compute the per-section profile for a project.
///
/// Reads the latest `audit_verdicts` row per `checkpoint` (gate). For each
/// section, lists every gate in `GATE_CATALOG` with that gate's latest
/// verdict and its governing ADRs. Thesis-only gates are listed only for
/// the `MasterThesis` section. The function does NOT run gates; it reflects
/// the last cascade's state.
pub fn compute(conn: &Connection, project_id: &str) -> Result<Vec<SectionProfile>> {
    // 1. Latest verdict per checkpoint (one row per gate, most-recent wins).
    let mut stmt = conn.prepare(
        "SELECT checkpoint, verdict FROM audit_verdicts \
         WHERE project_id = ?1 \
         AND id IN (SELECT MAX(id) FROM audit_verdicts \
                    WHERE project_id = ?1 GROUP BY checkpoint)",
    )?;
    let mut latest: BTreeMap<String, String> = BTreeMap::new();
    let rows = stmt.query_map(rusqlite::params![project_id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (gate, verdict) = row?;
        latest.insert(gate, verdict);
    }

    // 2. The 28 gates we care about. Hard-coded so the profile is stable
    //    across cascade runs even if a gate did not record a verdict (e.g.
    //    the cascade aborted before reaching it).
    let universal: &[&str] = &[
        "self",
        "tree",
        "deliverable",
        "citation_tracker",
        "contamination",
        "bibliography",
        "aibom",
        "docs",
        "facts_integrity",
        "i18n",
        "bookkit",
        "prisma",
        "cross_model",
        "model_review",
        "temporal",
        "ground_truth",
        "compliance",
        "sprint",
        "predatory",
        "reproducibility",
        "integrity",
        "figure_quality",
        "disclosure",
        "freshness",
    ];

    // 3. For each known section, build its list of gate statuses.
    let sections = [
        Section::MasterThesis,
        Section::Dimensions,
        Section::Campaigns,
        Section::Projects,
        Section::StudentNotes,
        Section::AgenticHandbook,
        Section::Norms,
        Section::Frontmatter,
        Section::Audit,
        Section::Other,
    ];
    let mut out = Vec::new();
    for &section in &sections {
        let mut gates: Vec<GateStatus> = universal
            .iter()
            .map(|g| GateStatus {
                gate: (*g).to_string(),
                verdict: latest.get(*g).cloned(),
                thesis_only: false,
                adrs: gate_adrs(g).iter().map(|s| (*s).to_string()).collect(),
            })
            .collect();
        if section == Section::MasterThesis {
            for g in THESIS_ONLY {
                gates.push(GateStatus {
                    gate: (*g).to_string(),
                    verdict: latest
                        .get(&g.replace('-', "_"))
                        .or_else(|| latest.get(*g))
                        .cloned(),
                    thesis_only: true,
                    adrs: gate_adrs(g).iter().map(|s| (*s).to_string()).collect(),
                });
            }
        }
        out.push(SectionProfile { section, gates });
    }
    Ok(out)
}

/// Render the profile as a markdown document. Stable column order so diffs
/// are readable.
#[must_use]
pub fn render_markdown(profiles: &[SectionProfile]) -> String {
    let mut s = String::new();
    s.push_str("# Audit Profile\n\nPer-section view of the runtime's last cascade — one row per gate per section, with the gate's latest verdict and the ADRs it enforces. Thesis-only gates appear only under master_thesis.\n");
    for sp in profiles {
        s.push_str(&format!("\n## {}\n\n", sp.section.slug()));
        s.push_str("| Gate | Verdict | Thesis-only | ADRs |\n|---|---|---|---|\n");
        for g in &sp.gates {
            let v = g.verdict.as_deref().unwrap_or("(none)");
            let adrs = if g.adrs.is_empty() {
                "—".to_string()
            } else {
                g.adrs.join(", ")
            };
            let to = if g.thesis_only { "yes" } else { "no" };
            s.push_str(&format!("| {} | {} | {} | {} |\n", g.gate, v, to, adrs));
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_paths_to_canonical_sections() {
        assert_eq!(
            classify_path("thesis/fhnw_2_theory.md"),
            Section::MasterThesis
        );
        assert_eq!(
            classify_path("out/sources/Dimension_06_quantum_computing_EN.md"),
            Section::Dimensions
        );
        assert_eq!(
            classify_path("out/sources/Dimensions_merged_EN.md"),
            Section::Dimensions
        );
        assert_eq!(
            classify_path("out/sources/Campaign_07_iso42001_first_mover_EN.md"),
            Section::Campaigns
        );
        assert_eq!(
            classify_path("out/sources/projects/PT-C09-6_x.md"),
            Section::Projects
        );
        assert_eq!(
            classify_path("out/sources/StudentNotes_Synthesis_EN.md"),
            Section::StudentNotes
        );
        assert_eq!(
            classify_path("out/sources/agentic_handbook/04_quickstart.md"),
            Section::AgenticHandbook
        );
        assert_eq!(
            classify_path("out/sources/AI_Audit_BOM_EN.md"),
            Section::Audit
        );
        assert_eq!(
            classify_path("out/sources/cascade_audit/01_report.md"),
            Section::Audit
        );
        assert_eq!(
            classify_path("out/sources/norms/06_norms_EN.md"),
            Section::Norms
        );
        assert_eq!(
            classify_path("out/sources/frontmatter/acronyms.md"),
            Section::Frontmatter
        );
        assert_eq!(classify_path("some/other/path.md"), Section::Other);
    }

    #[test]
    fn slug_round_trip() {
        for s in [
            Section::Dimensions,
            Section::Campaigns,
            Section::Projects,
            Section::StudentNotes,
            Section::MasterThesis,
            Section::AgenticHandbook,
            Section::Audit,
            Section::Norms,
            Section::Frontmatter,
            Section::Other,
        ] {
            assert_eq!(Section::from_slug(s.slug()), Some(s));
        }
        assert_eq!(Section::from_slug("nope"), None);
    }

    #[test]
    fn gate_adrs_known_lookups() {
        assert_eq!(
            gate_adrs("deliverable"),
            &["ADR-0036", "ADR-0037", "ADR-0038", "ADR-0044"]
        );
        assert_eq!(gate_adrs("model_review"), &["ADR-0049"]);
        assert_eq!(gate_adrs("model-review"), &["ADR-0049"]);
        assert_eq!(gate_adrs("page_boundary"), &["ADR-0035", "ADR-0045"]);
        assert!(gate_adrs("does-not-exist").is_empty());
    }
}
