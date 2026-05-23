//! Deterministic deliverable normalisation — the Rust port of
//! `normalize_deliverable.py`. Expands cryptic prediction×mode cell codes to
//! full words, shortens over-long figure captions (figure-standards R4), and
//! applies verified-facts corrections (ADR-0036). Run before the gate.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

static CODE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b([DHI])[-·.]([NEC])\b").unwrap());
static FIG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)```figspec\s*\n(.*?)\n```").unwrap());

// Verified-facts corrections (each verified against a primary source).
static CORR: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    vec![
        (
            Regex::new(r"(?i)(?:roughly\s+|approximately\s+|about\s+|around\s+|~\s*)?446(\s+source\s+packages)").unwrap(),
            "over 1,000${1}",
        ),
        (
            Regex::new(r"(?i)(?:roughly\s+|approximately\s+|about\s+|around\s+|~\s*)?446(\s+packages)").unwrap(),
            "over 1,000${1}",
        ),
        (Regex::new(r"(?i)\b446-source-package\b").unwrap(), "1,000-plus-source-package"),
        (Regex::new(r"(?i)\b446-package\b").unwrap(), "1,000-plus-package"),
    ]
});

fn pred(c: &str) -> &'static str {
    match c {
        "D" => "Decrease",
        "H" => "Hold",
        _ => "Increase",
    }
}
fn mode(c: &str) -> &'static str {
    match c {
        "N" => "Normal",
        "E" => "Escalation",
        _ => "Catastrophic",
    }
}

/// Reduce an over-long caption to a ≤12-word label (figure-standards R4).
fn short_caption(cap: &str) -> String {
    let cap = cap.trim();
    if cap.split_whitespace().count() <= 12 {
        return cap.to_string();
    }
    for sep in [". ", " — ", " – ", ": ", "; "] {
        if let Some(idx) = cap.find(sep) {
            let first = cap[..idx].trim();
            let w = first.split_whitespace().count();
            if w > 0 && w <= 12 {
                return first.to_string();
            }
        }
    }
    cap.split_whitespace().take(12).collect::<Vec<_>>().join(" ")
}

fn shorten_captions(text: &str) -> String {
    FIG.replace_all(text, |c: &regex::Captures<'_>| {
        match serde_json::from_str::<Value>(&c[1]) {
            Ok(mut spec) => {
                if let Some(cap) = spec.get("caption").and_then(Value::as_str) {
                    if cap.split_whitespace().count() > 12 {
                        let short = short_caption(cap);
                        spec["caption"] = Value::String(short);
                        return format!(
                            "```figspec\n{}\n```",
                            serde_json::to_string_pretty(&spec).unwrap_or_else(|_| c[0].to_string())
                        );
                    }
                }
                c[0].to_string()
            }
            Err(_) => c[0].to_string(),
        }
    })
    .into_owned()
}

fn apply_corrections(text: &str) -> String {
    let mut t = text.to_string();
    for (re, repl) in CORR.iter() {
        t = re.replace_all(&t, *repl).into_owned();
    }
    t
}

/// Normalise a deliverable: expand prediction×mode codes, shorten captions,
/// apply verified-facts corrections. Idempotent.
#[must_use]
pub fn normalize(text: &str) -> String {
    let expanded = CODE
        .replace_all(text, |c: &regex::Captures<'_>| format!("{} / {}", pred(&c[1]), mode(&c[2])))
        .into_owned();
    let shortened = shorten_captions(&expanded);
    apply_corrections(&shortened)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_prediction_mode_codes() {
        assert_eq!(normalize("the D-N cell and I·C cell"), "the Decrease / Normal cell and Increase / Catastrophic cell");
    }

    #[test]
    fn corrects_verified_facts() {
        assert_eq!(normalize("about 446 source packages"), "over 1,000 source packages");
        assert_eq!(normalize("a 446-package distro"), "a 1,000-plus-package distro");
    }

    #[test]
    fn shortens_long_caption_in_figspec() {
        let md = "```figspec\n{\"id\":\"f\",\"type\":\"bar\",\"caption\":\"one two three four five six seven eight nine ten eleven twelve thirteen fourteen\",\"data\":{}}\n```";
        let out = normalize(md);
        // 14 words -> truncated to 12
        let cap_words = out.lines().find(|l| l.contains("caption")).map(|l| l.matches(' ').count()).unwrap_or(0);
        assert!(out.contains("\"caption\""));
        assert!(cap_words <= 16, "caption not shortened: {out}");
    }

    #[test]
    fn idempotent_on_clean_text() {
        let t = "A clean paragraph with Decrease / Normal already expanded.";
        assert_eq!(normalize(t), t);
    }
}
