//! Bookkit-profile registry + rule-matrix (ADR-0047 R4).
//!
//! The rule *logic* (which gates exist, and the checkpoint each records under)
//! lives in code as [`GATE_CATALOG`]. The rule *mapping* — which gates apply to
//! which bookkit profile — is governed data: a [`RuleMatrix`] (a `universal`
//! set plus per-profile `additions`) loaded from the SDD-Chain
//! (`specs/rule-matrix.json`) and synced into the project. The cascade composes
//! its gate suite from the matrix, so adding a profile or moving a gate is a
//! data edit, not a recompile. [`RuleMatrix::default_matrix`] reproduces the
//! hard-coded behaviour, so the system works with or without the data file.

use std::collections::BTreeMap;

use anyhow::Result;
use serde::Deserialize;

/// Every gate the tool knows, as `(check subcommand, audit_verdicts checkpoint)`.
/// This is the catalog the rule-matrix references by subcommand name.
pub const GATE_CATALOG: &[(&str, &str)] = &[
    ("self", "self"),
    ("tree", "tree"),
    ("deliverable", "deliverable"),
    ("citations", "citation_tracker"),
    ("contamination", "contamination"),
    ("bibliography", "bibliography"),
    ("aibom", "aibom"),
    ("docs", "docs"),
    ("facts-integrity", "facts_integrity"),
    ("i18n", "i18n"),
    ("bookkit", "bookkit"),
    ("prisma", "prisma"),
    ("cross-model", "cross_model"),
    ("model-review", "model_review"),
    ("temporal", "temporal"),
    ("ground-truth", "ground_truth"),
    ("compliance", "compliance"),
    ("sprint", "sprint"),
    ("predatory", "predatory"),
    ("reproducibility", "reproducibility"),
    ("integrity", "integrity"),
    ("figure-quality", "figure_quality"),
    ("disclosure", "disclosure"),
    ("freshness", "freshness"),
    ("page-boundary", "page_boundary"),
    ("rr-matrix", "rr_matrix"),
    ("calibration", "calibration"),
    ("adr-enforcement", "adr_enforcement"), // ADR-0052 §4.3
    ("term-rename", "term_rename"),         // deprecated-token surveillance
    ("toc-coverage", "toc_coverage"),       // ADR-0056 §3.2 (renderer book TOC ⇄ source headings)
    ("artefact-cap", "artefact_cap"), // ADR-0055 — future gate (catalog registration only; subcommand not yet implemented)
    ("parity", "parity"),             // ADR-0057 §3 — visual / structural parity gate
];

/// Resolve a gate subcommand to its catalog `(subcommand, checkpoint)` entry.
#[must_use]
pub fn catalog_entry(sub: &str) -> Option<(&'static str, &'static str)> {
    GATE_CATALOG.iter().copied().find(|(s, _)| *s == sub)
}

/// One bookkit profile's extra gates beyond the universal set.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Profile {
    #[serde(default)]
    pub additions: Vec<String>,
}

/// The governed rule-matrix: a universal gate set + per-profile additions.
#[derive(Debug, Clone, Deserialize)]
pub struct RuleMatrix {
    pub universal: Vec<String>,
    #[serde(default)]
    pub profiles: BTreeMap<String, Profile>,
}

impl RuleMatrix {
    /// Parse a rule-matrix JSON document.
    pub fn parse(json: &str) -> Result<Self> {
        Ok(serde_json::from_str(json)?)
    }

    /// The default matrix — equivalent to the previously hard-coded suite:
    /// 28 universal gates + the bookkit-C-only page-boundary/rr-matrix/calibration.
    ///
    /// NOTE: `artefact-cap` (ADR-0055) is intentionally NOT in `universal` —
    /// it's registered in `GATE_CATALOG` so ADR-0052 frontmatter enforcement
    /// can reference it, but has no CLI subcommand yet. Including it here
    /// would break the cascade with a clap parse error at
    /// `agentic check artefact-cap`. Once the subcommand lands, add it to
    /// `universal` AND drop the `- catalog_only.len()` correction in the
    /// `default_matrix_suite_is_full_catalog` test.
    #[must_use]
    pub fn default_matrix() -> Self {
        let universal = [
            "self",
            "tree",
            "deliverable",
            "citations",
            "contamination",
            "bibliography",
            "aibom",
            "docs",
            "facts-integrity",
            "i18n",
            "bookkit",
            "prisma",
            "cross-model",
            "model-review",
            "temporal",
            "ground-truth",
            "compliance",
            "sprint",
            "predatory",
            "reproducibility",
            "integrity",
            "figure-quality",
            "disclosure",
            "freshness",
            "adr-enforcement", // ADR-0052 §4.3
            "term-rename",     // v0.1.18 — deprecated-token surveillance
            "toc-coverage",    // v0.1.18 — ADR-0056 §3.2
            "parity",          // v0.1.18 — ADR-0057 §3
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect();
        let mut profiles = BTreeMap::new();
        profiles.insert("A-books".to_string(), Profile::default());
        profiles.insert("B-companion".to_string(), Profile::default());
        profiles.insert(
            "C-thesis".to_string(),
            Profile {
                additions: ["page-boundary", "rr-matrix", "calibration"]
                    .iter()
                    .map(|s| (*s).to_string())
                    .collect(),
            },
        );
        Self {
            universal,
            profiles,
        }
    }

    /// The composed gate suite: the universal gates plus every profile's
    /// additions, de-duplicated and ordered by the catalog. Unknown gate names
    /// are skipped (the catalog is authoritative for what actually runs).
    #[must_use]
    pub fn gate_suite(&self) -> Vec<(&'static str, &'static str)> {
        let mut wanted: std::collections::BTreeSet<&str> =
            self.universal.iter().map(String::as_str).collect();
        for p in self.profiles.values() {
            wanted.extend(p.additions.iter().map(String::as_str));
        }
        GATE_CATALOG
            .iter()
            .copied()
            .filter(|(sub, _)| wanted.contains(sub))
            .collect()
    }

    /// Gates contributed by a profile's additions only (for reporting/tagging).
    #[must_use]
    pub fn is_universal(&self, sub: &str) -> bool {
        self.universal.iter().any(|u| u == sub)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_matrix_suite_is_full_catalog() {
        let m = RuleMatrix::default_matrix();
        // universal(28) + C additions(3) = 31 invocable gates.
        // GATE_CATALOG holds 32 entries because `artefact-cap` is registered
        // for ADR-frontmatter cross-checking (ADR-0055) but has no CLI
        // subcommand yet — so it must be excluded from default_matrix to keep
        // the cascade runnable. Once `agentic check artefact-cap` lands, drop
        // the `- 1` and add `"artefact-cap"` to default_matrix.universal.
        let catalog_only = ["artefact-cap"];
        assert_eq!(
            m.gate_suite().len(),
            GATE_CATALOG.len() - catalog_only.len()
        );
        for sub in catalog_only {
            assert!(
                GATE_CATALOG.iter().any(|(s, _)| *s == sub),
                "{sub} must remain in GATE_CATALOG (it's referenced by ADR enforcement)"
            );
            assert!(
                m.gate_suite().iter().all(|(s, _)| *s != sub),
                "{sub} must NOT appear in default_matrix gate_suite (no CLI subcommand yet)"
            );
        }
        assert!(!m.is_universal("page-boundary"));
        assert!(m.is_universal("freshness"));
    }

    #[test]
    fn parse_and_compose() {
        let json = r#"{"universal":["self","bookkit"],
            "profiles":{"C":{"additions":["page-boundary","unknown-gate"]}}}"#;
        let m = RuleMatrix::parse(json).unwrap();
        let suite = m.gate_suite();
        // self, bookkit, page-boundary resolve; unknown-gate is skipped.
        assert_eq!(suite.len(), 3);
        assert!(suite.iter().any(|(s, _)| *s == "page-boundary"));
        assert!(!suite.iter().any(|(s, _)| *s == "unknown-gate"));
        // checkpoint mapping comes from the catalog.
        assert!(suite.iter().any(|(s, cp)| *s == "self" && *cp == "self"));
    }
}
