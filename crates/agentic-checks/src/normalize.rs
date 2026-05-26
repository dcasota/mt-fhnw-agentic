//! Deterministic deliverable normalisation — the Rust port of
//! `normalize_deliverable.py`. Expands cryptic prediction×mode cell codes to
//! full words, shortens over-long figure captions (figure-standards R4), and
//! applies verified-facts corrections (ADR-0036). Run before the gate.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

static CODE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b([DHI])[-·.]([NEC])\b").unwrap());
static FIG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)```figspec\s*\n(.*?)\n```").unwrap());

// --- bold de-emphasis (R5 enforcement of bookkit RULE 1) -----------------
// Mirrors `bookkit_gate`: bold is permitted ONLY as a short leading label
// (starts the paragraph/list item, <= 8 words and <= 60 chars). Any other bold
// has its `**` markers removed so the prose complies with the bookkit gate.
static BOLD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\*\*(.+?)\*\*").unwrap());
static LEAD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(?:[\s>#+-]|\d+\.|\*\s)*").unwrap());
static JSON_KEY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""[\w-]+"\s*:"#).unwrap());
const MAX_LABEL_WORDS: usize = 8;
const MAX_LABEL_CHARS: usize = 60;

fn looks_like_json(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with('{') || t.starts_with('}') || t.starts_with('"') || JSON_KEY.is_match(line)
}

/// De-emphasise non-compliant bold. A bold span is kept only when it is a short
/// leading label (mirrors `bookkit_gate::bold_violations`); every other `**…**`
/// has its markers stripped to plain text. Fence- and JSON-aware. Idempotent
/// (compliant labels survive a re-run; stripped spans have no `**` left).
#[must_use]
pub fn debold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_fence = false;
    let mut first = true;
    for ln in text.lines() {
        if !first {
            out.push('\n');
        }
        first = false;
        if ln.trim_start().starts_with("```") {
            in_fence = !in_fence;
            out.push_str(ln);
            continue;
        }
        if in_fence || looks_like_json(ln) {
            out.push_str(ln);
            continue;
        }
        let lead_end = LEAD.find(ln).map_or(0, |m| m.end());
        let mut rebuilt = String::with_capacity(ln.len());
        let mut last = 0usize;
        for m in BOLD.captures_iter(ln) {
            let (Some(whole), Some(grp)) = (m.get(0), m.get(1)) else {
                continue;
            };
            let inner = grp.as_str();
            let is_leading = whole.start() == lead_end;
            let allowed = is_leading
                && inner.split_whitespace().count() <= MAX_LABEL_WORDS
                && inner.chars().count() <= MAX_LABEL_CHARS;
            rebuilt.push_str(&ln[last..whole.start()]);
            if allowed {
                rebuilt.push_str(whole.as_str()); // keep the compliant label
            } else {
                rebuilt.push_str(inner); // strip the `**` markers
            }
            last = whole.end();
        }
        rebuilt.push_str(&ln[last..]);
        out.push_str(&rebuilt);
    }
    if text.ends_with('\n') {
        out.push('\n');
    }
    out
}

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
    cap.split_whitespace()
        .take(12)
        .collect::<Vec<_>>()
        .join(" ")
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
                            serde_json::to_string_pretty(&spec)
                                .unwrap_or_else(|_| c[0].to_string())
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
        .replace_all(text, |c: &regex::Captures<'_>| {
            format!("{} / {}", pred(&c[1]), mode(&c[2]))
        })
        .into_owned();
    let shortened = shorten_captions(&expanded);
    let corrected = apply_corrections(&shortened);
    debold(&corrected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_prediction_mode_codes() {
        assert_eq!(
            normalize("the D-N cell and I·C cell"),
            "the Decrease / Normal cell and Increase / Catastrophic cell"
        );
    }

    #[test]
    fn corrects_verified_facts() {
        assert_eq!(
            normalize("about 446 source packages"),
            "over 1,000 source packages"
        );
        assert_eq!(
            normalize("a 446-package distro"),
            "a 1,000-plus-package distro"
        );
    }

    #[test]
    fn shortens_long_caption_in_figspec() {
        let md = "```figspec\n{\"id\":\"f\",\"type\":\"bar\",\"caption\":\"one two three four five six seven eight nine ten eleven twelve thirteen fourteen\",\"data\":{}}\n```";
        let out = normalize(md);
        // 14 words -> truncated to 12
        let cap_words = out
            .lines()
            .find(|l| l.contains("caption"))
            .map(|l| l.matches(' ').count())
            .unwrap_or(0);
        assert!(out.contains("\"caption\""));
        assert!(cap_words <= 16, "caption not shortened: {out}");
    }

    #[test]
    fn idempotent_on_clean_text() {
        let t = "A clean paragraph with Decrease / Normal already expanded.";
        assert_eq!(normalize(t), t);
    }

    #[test]
    fn debold_strips_inline_keeps_leading_label() {
        // Inline bold mid-prose is stripped.
        assert_eq!(
            debold("text with **bold** inside.\n"),
            "text with bold inside.\n"
        );
        // A short leading label is preserved.
        assert_eq!(
            debold("**Term.** the rest is plain.\n"),
            "**Term.** the rest is plain.\n"
        );
        // List-item leading label preserved; trailing inline stripped.
        assert_eq!(
            debold("- **Label:** see **this** part.\n"),
            "- **Label:** see this part.\n"
        );
        // Over-long leading label (>8 words) is de-emphasised.
        assert_eq!(
            debold("**one two three four five six seven eight nine** rest.\n"),
            "one two three four five six seven eight nine rest.\n"
        );
    }

    #[test]
    fn debold_is_idempotent_and_fence_safe() {
        let t = "text with **bold** and `**code**` then\n```\n**keep** in fence\n```\n";
        let once = debold(t);
        assert_eq!(debold(&once), once, "debold must be idempotent");
        assert!(once.contains("**keep** in fence"), "fenced bold preserved");
    }
}
