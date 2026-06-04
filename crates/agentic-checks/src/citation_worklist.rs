//! Rust port of `MT-Template/dist/_build_citation_worklist.py` — group raw
//! per-occurrence citation records by `(surname, year)`, prefer
//! DOI/arxiv/publisher URLs, and emit a worklist JSON usable by the
//! downstream URL-research workflow.
//!
//! Wave-2 Agent C (Python→Rust migration, 2026-06-04). The Python script's
//! input contract (the JSON array produced by `_extract_citations.py`) is
//! preserved so existing fixtures still flow through; this Rust port exposes
//! the same `build_worklist` transform as a library function, which the
//! agentic CLI's `check citations` flow can consume directly.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One raw citation occurrence — the per-paragraph record produced upstream.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CitationRecord {
    pub surname: String,
    pub year: String,
    #[serde(default)]
    pub bib_label: String,
    #[serde(default)]
    pub bib_urls: Vec<String>,
    pub paragraph_idx: u64,
    pub breadcrumb: String,
    pub raw_citation: String,
    pub sentence: String,
    pub recommendation: String,
    #[serde(default)]
    pub flags: BTreeMap<String, Value>,
}

/// One grouped worklist entry — emitted in the same JSON shape the Python
/// script wrote so downstream tooling keeps working.
#[derive(Debug, Clone, Serialize)]
pub struct WorklistEntry {
    pub key: String,
    pub surname: String,
    pub year: String,
    pub bib_label: String,
    /// Filled later from the docx; preserved as empty for round-trip parity.
    pub bib_text: String,
    pub urls: Vec<String>,
    pub occurrences: Vec<WorklistOccurrence>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorklistOccurrence {
    pub paragraph_idx: u64,
    pub breadcrumb: String,
    pub raw_citation: String,
    pub sentence: String,
    pub recommendation: String,
    /// Truthy flag names (the Python script's
    /// `[k for k,v in o['flags'].items() if v]`).
    pub flags: Vec<String>,
}

/// URL-preference score — lower is better. Mirrors the Python `score_url`
/// closure verbatim: DOI=0, arxiv=1, github=5, else=3.
#[must_use]
pub fn score_url(u: &str) -> u8 {
    if u.contains("doi.org") {
        0
    } else if u.contains("arxiv.org") {
        1
    } else if u.contains("github.com") {
        5
    } else {
        3
    }
}

/// Stable lower-case form of `surname` used both as the grouping key and as
/// part of the synthesised `worklist.key`.
fn surname_lower(s: &str) -> String {
    s.to_lowercase()
}

/// Build the deduplicated worklist from raw occurrences. The output is sorted
/// by `(surname_lower, year)` so two invocations on the same input produce
/// byte-identical JSON.
#[must_use]
pub fn build_worklist(records: &[CitationRecord]) -> Vec<WorklistEntry> {
    // Group by (surname_lower, year), preserving insertion order within a
    // group so the Python "first occurrence is representative" semantics
    // hold.
    let mut groups: BTreeMap<(String, String), Vec<&CitationRecord>> = BTreeMap::new();
    for c in records {
        groups
            .entry((surname_lower(&c.surname), c.year.clone()))
            .or_default()
            .push(c);
    }

    let mut out = Vec::with_capacity(groups.len());
    for ((surname_low, year), occurrences) in groups {
        let rep = occurrences[0];
        let mut urls = rep.bib_urls.clone();
        urls.sort_by_key(|u| score_url(u));
        let occs = occurrences
            .iter()
            .map(|o| WorklistOccurrence {
                paragraph_idx: o.paragraph_idx,
                breadcrumb: o.breadcrumb.clone(),
                raw_citation: o.raw_citation.clone(),
                sentence: o.sentence.clone(),
                recommendation: o.recommendation.clone(),
                flags: o
                    .flags
                    .iter()
                    .filter_map(|(k, v)| {
                        if value_is_truthy(v) {
                            Some(k.clone())
                        } else {
                            None
                        }
                    })
                    .collect(),
            })
            .collect();
        out.push(WorklistEntry {
            key: format!("{surname_low}_{year}"),
            surname: rep.surname.clone(),
            year: year.clone(),
            bib_label: rep.bib_label.clone(),
            bib_text: String::new(),
            urls,
            occurrences: occs,
        });
    }
    out
}

/// Mirror Python's truthiness (`if v:`): `false`, `null`, `0`, `""`, `[]`,
/// `{}` are falsy.
fn value_is_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|x| x != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// Summary stats matching the Python script's stdout: total worklist
/// entries, with-URL count, without-URL count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorklistStats {
    pub total: usize,
    pub with_url: usize,
    pub without_url: usize,
}

#[must_use]
pub fn stats(worklist: &[WorklistEntry]) -> WorklistStats {
    let with_url = worklist.iter().filter(|w| !w.urls.is_empty()).count();
    WorklistStats {
        total: worklist.len(),
        with_url,
        without_url: worklist.len() - with_url,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(
        surname: &str,
        year: &str,
        para: u64,
        urls: Vec<&str>,
        flags: Vec<(&str, Value)>,
    ) -> CitationRecord {
        CitationRecord {
            surname: surname.into(),
            year: year.into(),
            bib_label: format!("[{para}]"),
            bib_urls: urls.into_iter().map(String::from).collect(),
            paragraph_idx: para,
            breadcrumb: "Ch1 > Intro".into(),
            raw_citation: format!("({surname}, {year})"),
            sentence: "A sentence.".into(),
            recommendation: "ok".into(),
            flags: flags.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
        }
    }

    #[test]
    fn score_url_prefers_doi_then_arxiv_then_generic_then_github() {
        assert_eq!(score_url("https://doi.org/10.1/x"), 0);
        assert_eq!(score_url("https://arxiv.org/abs/2401.x"), 1);
        assert_eq!(score_url("https://example.com/paper.pdf"), 3);
        assert_eq!(score_url("https://github.com/foo/bar"), 5);
    }

    #[test]
    fn build_worklist_groups_by_surname_year_case_insensitive() {
        let recs = vec![
            rec(
                "Karpathy",
                "2024",
                10,
                vec!["https://github.com/k/x"],
                vec![],
            ),
            rec("karpathy", "2024", 50, vec![], vec![]),
            rec("Garlan", "2004", 12, vec!["https://doi.org/10.1/g"], vec![]),
        ];
        let wl = build_worklist(&recs);
        assert_eq!(wl.len(), 2);
        let karp = wl.iter().find(|w| w.key == "karpathy_2024").unwrap();
        assert_eq!(karp.occurrences.len(), 2);
        // Representative entry comes from the FIRST occurrence (Karpathy).
        assert_eq!(karp.surname, "Karpathy");
    }

    #[test]
    fn build_worklist_sorts_urls_by_preference() {
        let recs = vec![rec(
            "Doe",
            "2024",
            1,
            vec![
                "https://github.com/foo",
                "https://example.com",
                "https://doi.org/10/x",
                "https://arxiv.org/abs/1",
            ],
            vec![],
        )];
        let wl = build_worklist(&recs);
        assert_eq!(
            wl[0].urls,
            vec![
                "https://doi.org/10/x".to_string(),
                "https://arxiv.org/abs/1".to_string(),
                "https://example.com".to_string(),
                "https://github.com/foo".to_string(),
            ]
        );
    }

    #[test]
    fn build_worklist_keeps_only_truthy_flags() {
        let recs = vec![rec(
            "Doe",
            "2024",
            1,
            vec![],
            vec![
                ("direct_quote", Value::Bool(true)),
                ("regulatory_clause", Value::Bool(false)),
                ("specific_finding", Value::String("yes".into())),
                (
                    "statistical_claim",
                    Value::Number(serde_json::Number::from(0)),
                ),
                ("general_reference", Value::Null),
                ("custom_flag", Value::Array(vec![Value::Bool(true)])),
            ],
        )];
        let wl = build_worklist(&recs);
        let mut got = wl[0].occurrences[0].flags.clone();
        got.sort();
        assert_eq!(got, vec!["custom_flag", "direct_quote", "specific_finding"]);
    }

    #[test]
    fn stats_split_with_and_without_url() {
        let wl = build_worklist(&[
            rec("A", "2020", 1, vec!["https://doi.org/x"], vec![]),
            rec("B", "2021", 2, vec![], vec![]),
            rec("C", "2022", 3, vec![], vec![]),
        ]);
        let s = stats(&wl);
        assert_eq!(s.total, 3);
        assert_eq!(s.with_url, 1);
        assert_eq!(s.without_url, 2);
    }

    #[test]
    fn build_worklist_is_deterministic() {
        let recs = vec![
            rec("Zeta", "2024", 1, vec![], vec![]),
            rec("Alpha", "2020", 2, vec![], vec![]),
            rec("Mu", "2022", 3, vec![], vec![]),
        ];
        let a = build_worklist(&recs);
        let b = build_worklist(&recs);
        let a_keys: Vec<_> = a.iter().map(|w| &w.key).collect();
        let b_keys: Vec<_> = b.iter().map(|w| &w.key).collect();
        assert_eq!(a_keys, b_keys);
        // BTreeMap sorts → alphabetical surname order.
        assert_eq!(a_keys, vec!["alpha_2020", "mu_2022", "zeta_2024"]);
    }
}
