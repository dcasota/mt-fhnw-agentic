//! Deliverable acceptance gate — the Rust port of `verify_gate.py`
//! (ADR-0036/0037/0038 + figure-standards). Operates on markdown text and
//! returns [`Finding`]s: ERROR is blocking, WARN is advisory.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use crate::{CheckReport, Finding, Severity};

// Unambiguously-German marker terms (ADR-0037); gloss (`*…*`/`(…)`) is exempt.
static DE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(Ausgangslage|Zielsetzung|Forschungsfrage[n]?|Abgrenzung|Empfehlung(?:en)?|Methodik|Einf[üu]hrung|Schlussbetrachtung|L[öo]sung|Handlungsempfehlung(?:en)?|Bewertung|[ÜU]bersicht|Zusammenfassung|Verzeichnis|Abbildung|Tabelle|Begleitbrief|Fragebogen|Umfrage|Leitfaden|Grundlagen|werden m[üu]ssen)\b",
    )
    .unwrap()
});
static CROSSREF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"out/[\w./-]+\.docx").unwrap());
static MARKER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<!--.*?-->").unwrap());
static CELLCODE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b[DHI][-·.][NEC]\b").unwrap());
static NUM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b\d[\d,'.]*\s*(packages?|%|percent|million|billion|days?|GB|MB|engineers?|FLOPs?)\b",
    )
    .unwrap()
});
static NUMSRC: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\(|http|doi|github|source|CAR-|§|cite|measured").unwrap());
static FIGSPEC: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)```figspec\s*\n(.*?)\n```").unwrap());

const GRAPHICAL: &[&str] = &["bar", "hbar", "line", "matrix", "flow", "quadrant"];

/// All gate findings for one markdown document. `label` prefixes locations.
#[must_use]
pub fn findings_for(label: &str, text: &str) -> Vec<Finding> {
    let mut out = Vec::new();
    for (idx, ln) in text.lines().enumerate() {
        let i = idx + 1;
        let here = || Some(format!("{label}:{i}"));
        // ADR-0037 English-only (skip terms glossed in *…* or (…) just before).
        if let Some(m) = DE.find(ln) {
            let glossed = ln[..m.start()]
                .chars()
                .rev()
                .take(2)
                .any(|c| c == '*' || c == '(');
            if !glossed {
                out.push(err(
                    "NON_ENGLISH_TEXT",
                    format!("L{i}: German term '{}'", m.as_str()),
                    here(),
                ));
            }
        }
        if let Some(m) = CROSSREF.find(ln) {
            out.push(err(
                "CROSS_REFERENCE",
                format!("L{i}: build-path reference '{}'", m.as_str()),
                here(),
            ));
        }
        if MARKER.is_match(ln) {
            out.push(err(
                "INTERNAL_MARKER",
                format!("L{i}: HTML-comment marker"),
                here(),
            ));
        }
        if CELLCODE.is_match(ln) {
            out.push(err(
                "CRYPTIC_LABEL",
                format!("L{i}: forbidden prediction-mode code (use full words)"),
                here(),
            ));
        }
        if NUM.is_match(ln) && !NUMSRC.is_match(ln) {
            out.push(Finding {
                category: "NUMBER_UNSOURCED".into(),
                severity: Severity::Warn,
                message: format!("L{i}: numeric claim without an inline source -- verify"),
                location: here(),
            });
        }
    }
    // figure-standards: caption length + graphical-only + valid JSON.
    for cap in FIGSPEC.captures_iter(text) {
        let raw = &cap[1];
        match serde_json::from_str::<Value>(raw) {
            Ok(spec) => {
                let id = spec
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("?")
                    .to_string();
                let words = spec
                    .get("caption")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .split_whitespace()
                    .count();
                if words > 12 {
                    out.push(err(
                        "CAPTION_TOO_LONG",
                        format!("{id}: caption {words} words (max 12)"),
                        None,
                    ));
                }
                let typ = spec.get("type").and_then(Value::as_str).unwrap_or("");
                if !GRAPHICAL.contains(&typ) {
                    out.push(err(
                        "FIGURE_NOT_GRAPHICAL",
                        format!("{id}: type '{typ}' not a real graphical figure"),
                        None,
                    ));
                }
            }
            Err(_) => out.push(err(
                "FIGSPEC_INVALID",
                "figspec JSON parse error".into(),
                None,
            )),
        }
    }
    out
}

fn err(cat: &str, msg: String, loc: Option<String>) -> Finding {
    Finding {
        category: cat.into(),
        severity: Severity::Error,
        message: msg,
        location: loc,
    }
}

/// Gate one document into a [`CheckReport`] (verdict Fail if any ERROR).
#[must_use]
pub fn run_text(label: &str, text: &str) -> CheckReport {
    CheckReport::new("deliverable", findings_for(label, text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Verdict;

    #[test]
    fn clean_text_passes() {
        let r = run_text(
            "x.md",
            "# Title\n\nA clean English paragraph with a [source](http://x).\n",
        );
        assert_eq!(r.verdict, Verdict::Pass);
    }

    #[test]
    fn catches_german_crossref_marker_code() {
        let md = "## Schlussbetrachtung\nSee out/dimensions/D_07.docx for more.\n<!-- note -->\nThe D-N cell wins.\n";
        let fs = findings_for("x.md", md);
        let cats: Vec<&str> = fs.iter().map(|f| f.category.as_str()).collect();
        assert!(cats.contains(&"NON_ENGLISH_TEXT"));
        assert!(cats.contains(&"CROSS_REFERENCE"));
        assert!(cats.contains(&"INTERNAL_MARKER"));
        assert!(cats.contains(&"CRYPTIC_LABEL"));
    }

    #[test]
    fn gloss_exempts_german() {
        // German term in parentheses (a gloss) is exempt.
        let fs = findings_for("x.md", "The conclusion (Schlussbetrachtung) follows.\n");
        assert!(!fs.iter().any(|f| f.category == "NON_ENGLISH_TEXT"));
    }

    #[test]
    fn figspec_caption_and_type_checked() {
        let long = "```figspec\n{\"id\":\"f1\",\"type\":\"pie\",\"caption\":\"one two three four five six seven eight nine ten eleven twelve thirteen\",\"data\":{}}\n```\n";
        let fs = findings_for("x.md", long);
        let cats: Vec<&str> = fs.iter().map(|f| f.category.as_str()).collect();
        assert!(cats.contains(&"CAPTION_TOO_LONG"));
        assert!(cats.contains(&"FIGURE_NOT_GRAPHICAL"));
    }
}
