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
static NUMSRC: LazyLock<Regex> = LazyLock::new(|| {
    // An inline source/justification marker near a number rescues it from
    // NUMBER_UNSOURCED. Includes the project's own model-estimate context
    // (RAMP effort/cost per ADR-0040: engineer-weeks/-months, person-hours,
    // "loaded" cost, "estimate") — those are sourced to the prediction model,
    // not external facts.
    Regex::new(
        r"(?i)\(|http|doi|github|source|CAR-|§|cite|measured|estimat|engineer-(?:week|month|day)|person-(?:month|hour)|loaded|RAMP|ADR-|REQ-|SLO|\bpackages?\b|roughly|approximately|~",
    )
    .unwrap()
});
static FIGSPEC: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)```figspec\s*\n(.*?)\n```").unwrap());

const GRAPHICAL: &[&str] = &[
    "bar", "hbar", "line", "matrix", "flow", "quadrant", // base chart types
    "icon", "heatmap", "regstack", "govmap", // book-figure types (AI-Norms generator ports)
    "treemap", "procmap", // population treemap + ISO-5338-style process map
];

/// All gate findings for one markdown document. `label` prefixes locations.
#[must_use]
pub fn findings_for(label: &str, text: &str) -> Vec<Finding> {
    findings_for_facts(label, text, &[])
}

/// True if byte offset `pos` in `ln` sits inside an open quotation span — i.e.
/// an odd number of quote toggles (straight `"` or curly `“`/`”`) precede it.
/// Numbers inside a quoted title/quote (e.g. the *"$200 million … playbook"*)
/// are part of a name, not the author's own unsourced quantitative claim.
fn inside_quote(ln: &str, pos: usize) -> bool {
    ln[..pos]
        .chars()
        .filter(|&c| c == '"' || c == '“' || c == '”')
        .count()
        % 2
        == 1
}

/// As [`findings_for`], but a number on a line matching an anchored verified-fact
/// claim (ADR-0016/0042) is treated as already-sourced (no NUMBER_UNSOURCED).
#[must_use]
pub fn findings_for_facts(label: &str, text: &str, anchored: &[String]) -> Vec<Finding> {
    let mut out = Vec::new();
    let mut in_fence = false;
    // Parenthesis nesting carried across prose lines: a number inside a
    // multi-line `(…)` aside is rescued exactly as the existing single-line
    // `(` rule (NUMSRC) already rescues one. Reset at paragraph breaks so a
    // stray unclosed `(` can't bleed past a blank line.
    let mut paren_depth: i32 = 0;
    for (idx, ln) in text.lines().enumerate() {
        let i = idx + 1;
        // Fenced code/figspec blocks are literal, not prose: a ``` toggles the
        // fence and the per-line prose checks (incl. NUMBER_UNSOURCED on figure
        // data) are skipped inside it. Figspec captions are still validated by
        // the separate FIGSPEC pass below.
        if ln.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        if ln.trim().is_empty() {
            paren_depth = 0; // paragraph break closes any dangling parenthetical
        }
        let paren_before = paren_depth;
        paren_depth += i32::try_from(ln.matches('(').count()).unwrap_or(0)
            - i32::try_from(ln.matches(')').count()).unwrap_or(0);
        paren_depth = paren_depth.max(0);
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
        // A number is unsourced only if its line carries no source marker, it
        // is not inside a multi-line parenthetical aside, it doesn't match an
        // anchored verified fact, AND at least one of its occurrences sits
        // outside a quoted title/quote.
        let line_sourced = NUMSRC.is_match(ln)
            || paren_before > 0
            || anchored.iter().any(|a| ln.contains(a.as_str()));
        if !line_sourced && NUM.find_iter(ln).any(|m| !inside_quote(ln, m.start())) {
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
    fn bare_unsourced_number_still_flags() {
        // Control: an unquoted number with no source and no parenthetical
        // must still be flagged — the precision fixes must not over-suppress.
        let fs = findings_for("x.md", "The final disk image is 47 GB total.\n");
        assert!(fs.iter().any(|f| f.category == "NUMBER_UNSOURCED"));
    }

    #[test]
    fn number_in_multiline_parenthetical_is_sourced() {
        // The citation opens a parenthetical on an earlier line; the numbers
        // land on later lines of the same aside and must inherit the source.
        let md = "Certificate lifetimes shorten (per CA/Browser Forum\nBallot SC-081v3: 200 days from 2026, 100 days from\n2027, and 47 days from 2029) so automation is required.\n";
        let fs = findings_for("x.md", md);
        assert!(
            !fs.iter().any(|f| f.category == "NUMBER_UNSOURCED"),
            "numbers inside a multi-line parenthetical aside should be rescued"
        );
    }

    #[test]
    fn number_inside_quoted_title_is_exempt() {
        // A number that is part of a quoted proper-noun title is a name, not
        // an unsourced quantitative claim.
        let fs = findings_for(
            "x.md",
            "The \"$200 million hybrid AI-media buying playbook\" names the trend.\n",
        );
        assert!(!fs.iter().any(|f| f.category == "NUMBER_UNSOURCED"));
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
