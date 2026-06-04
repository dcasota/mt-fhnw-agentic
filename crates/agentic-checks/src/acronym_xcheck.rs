//! Rust port of `MT-Template/dist/_acronym_full_xcheck.py` — cross-check
//! "missing acronyms" candidates against the actual acronym table so false
//! positives (variants with a trailing colon, case-differences) don't surface
//! as omissions.
//!
//! Wave-2 Agent C (Python→Rust migration, 2026-06-04). The Python script
//! consumed the legacy DOCX table directly; this Rust port takes the listed
//! tokens as a pre-extracted set — the existing `undefined_terms` gate is
//! responsible for extracting acronyms from markdown, so this module is the
//! pure-set-diff primitive other gates can call.
//!
//! Behaviour preserved verbatim:
//!   * trailing-colon variants (`"PKI:"` vs `"PKI"`) are normalised before
//!     comparison;
//!   * a `(token, count)` pair is "already present" iff its normalised token
//!     appears in the table;
//!   * everything else is "really missing".

use std::collections::BTreeSet;

use serde::Serialize;

/// One candidate row from the suggested-missing list: `(token, occurrences)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AcronymCandidate {
    pub token: String,
    pub count: u32,
}

/// Result of the cross-check — two disjoint partitions of the input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct AcronymXCheck {
    /// Really missing — the token does NOT appear in the table.
    pub really_missing: Vec<AcronymCandidate>,
    /// False positive — the token DOES appear in the table after
    /// normalisation.
    pub already_present: Vec<AcronymCandidate>,
}

/// Normalise a single token for comparison: strip a trailing colon. (Case is
/// preserved — acronyms are intentionally upper-case in FHNW thesis prose.)
#[must_use]
pub fn normalise_token(t: &str) -> &str {
    t.trim_end_matches(':')
}

/// Normalise an entire collection of listed-table acronyms.
#[must_use]
pub fn normalise_listed<'a, I>(listed: I) -> BTreeSet<String>
where
    I: IntoIterator<Item = &'a str>,
{
    listed
        .into_iter()
        .map(|t| normalise_token(t).to_string())
        .collect()
}

/// Partition `suggested` candidates against the `listed` acronym set.
#[must_use]
pub fn xcheck(suggested: &[AcronymCandidate], listed: &BTreeSet<String>) -> AcronymXCheck {
    let mut out = AcronymXCheck::default();
    for c in suggested {
        let norm = normalise_token(&c.token);
        if listed.contains(norm) {
            out.already_present.push(c.clone());
        } else {
            out.really_missing.push(c.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalise_strips_trailing_colon_only() {
        assert_eq!(normalise_token("PKI"), "PKI");
        assert_eq!(normalise_token("PKI:"), "PKI");
        assert_eq!(normalise_token(":PKI"), ":PKI");
        assert_eq!(normalise_token("PKI::"), "PKI");
    }

    #[test]
    fn xcheck_splits_into_already_and_really_missing() {
        let listed = normalise_listed(["PKI", "DORA:", "NIS2"]);
        let suggested = vec![
            AcronymCandidate {
                token: "PKI".into(),
                count: 12,
            },
            AcronymCandidate {
                token: "DORA".into(),
                count: 3,
            },
            AcronymCandidate {
                token: "QRNG".into(),
                count: 1,
            },
        ];
        let r = xcheck(&suggested, &listed);
        assert_eq!(r.already_present.len(), 2);
        assert_eq!(r.really_missing.len(), 1);
        assert_eq!(r.really_missing[0].token, "QRNG");
    }

    #[test]
    fn xcheck_preserves_count_through_normalisation() {
        let listed = normalise_listed(["AIBOM"]);
        let suggested = vec![AcronymCandidate {
            token: "AIBOM:".into(),
            count: 7,
        }];
        let r = xcheck(&suggested, &listed);
        // Trailing colon in the SUGGESTED token: still a false positive.
        assert_eq!(r.already_present.len(), 1);
        assert_eq!(r.already_present[0].count, 7);
        assert!(r.really_missing.is_empty());
    }

    #[test]
    fn empty_inputs_yield_empty_output() {
        let r = xcheck(&[], &BTreeSet::new());
        assert!(r.really_missing.is_empty());
        assert!(r.already_present.is_empty());
    }

    #[test]
    fn case_difference_does_not_collapse() {
        // Intentional: 'pki' != 'PKI' in body prose.
        let listed = normalise_listed(["PKI"]);
        let suggested = vec![AcronymCandidate {
            token: "pki".into(),
            count: 1,
        }];
        let r = xcheck(&suggested, &listed);
        assert_eq!(r.really_missing.len(), 1);
    }
}
