//! Writing-quality checker: 46 AI-typical patterns + FHNW style rules.
//!
//! Port of `code/tools/writing_quality_check.py` from the FHNW template.
//! Two rule families:
//!
//! 1. **AI-typical patterns** (Lu et al. 2026 + Anthropic dataset): tokens that
//!    co-occur with LLM-generated text more often than chance.
//! 2. **FHNW style rules**: first-person pronouns banned, questions cannot be
//!    section headings, absolute statements flagged, gendered nouns must use
//!    Paarform or neutrale Form, em-dash overuse, uniform paragraph length.
//!
//! Each finding is severity-tagged so the consolidated checkpoint
//! (`compliance_consolidated`) can apply the standard verdict mapping.

use std::sync::OnceLock;

use agentic_core::worktree;
use anyhow::Result;
use regex::Regex;
use rusqlite::Connection;

use crate::{CheckReport, Finding, Severity};

const AI_TYPICAL_TOKENS: &[(&str, &str)] = &[
    ("delve", "LLM-typical opener. Prefer concrete verb."),
    ("tapestry", "Cliché. Use 'set' or 'collection'."),
    ("landscape", "Use concrete domain term."),
    ("pivotal", "Use 'important' or 'central'."),
    ("crucial", "Hedging-feel. Quantify or replace."),
    ("leverage", "Business jargon. Use 'use' or 'apply'."),
    ("robust", "Discipline-dependent; exempt in statistics."),
    ("navigate", "Metaphor. Use literal verb."),
    ("moreover", "Overused. Vary connectives."),
    ("furthermore", "Overused."),
    ("additionally", "Overused."),
    ("in conclusion", "Throat-clearing. Drop or rephrase."),
    ("it is important to note", "Throat-clearing. Drop."),
    ("it is crucial to note", "Throat-clearing."),
    ("in the realm of", "Throat-clearing."),
    ("in today's", "Generic opener."),
    ("in the era of", "Generic opener."),
    ("paradigm shift", "Discipline-dependent."),
    ("synergy", "Buzzword."),
    ("holistic", "Often used vaguely."),
    ("comprehensive", "Often used vaguely."),
    ("underscore", "Overused alternative to 'emphasise'."),
    ("underscores", "Overused."),
    ("spearhead", "Cliché."),
    ("myriad", "Cliché."),
    ("plethora", "Cliché."),
    ("facilitate", "Often replaceable by simpler verb."),
    ("intricate", "Often replaceable by 'complex'."),
    ("nuanced", "Often used vaguely."),
    ("elucidate", "Often replaceable by 'explain'."),
    ("showcasing", "Often replaceable by 'showing'."),
    ("showcases", "Often replaceable by 'shows'."),
    ("harness", "Business jargon."),
    ("embark", "Cliché."),
    ("transformative", "Often used vaguely."),
    ("groundbreaking", "Strong claim; substantiate or drop."),
    ("cutting-edge", "Cliché."),
    ("state-of-the-art", "Discipline-dependent; substantiate."),
    ("real-world", "Often vague."),
    ("seamlessly", "Marketing language."),
    ("intuitive", "Often vague."),
    ("vibrant", "Marketing language."),
    ("bustling", "Marketing language."),
    ("ushering in", "Throat-clearing."),
    ("at the forefront", "Cliché."),
    ("in conclusion,", "Throat-clearing."),
];

const GENDERED_NOUNS_DE: &[&str] = &[
    "Studenten",
    "Mitarbeiter",
    "Kunden",
    "Teilnehmer",
    "Dozenten",
    "Referenten",
    "Anwender",
    "Nutzer",
    "Lehrer",
    "Forscher",
    "Experten",
    "Manager",
    "Leser",
    "Autoren",
    "Bürger",
];

fn first_person_de() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?i)\b(ich|wir|unser|mein|uns)\b").unwrap())
}

fn absolute_de() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?i)\b(immer|nie|alle|niemand|jeder|kein)\b").unwrap())
}

fn heading_as_question() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?m)^#{1,6}\s+.*\?\s*$").unwrap())
}

fn heading_as_sentence() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?m)^#{1,6}\s+\w.+[.!]\s*$").unwrap())
}

#[must_use]
pub fn em_dash_count(text: &str) -> usize {
    text.matches('—').count() + text.matches(" -- ").count()
}

#[must_use]
pub fn paragraph_lengths_uniform(text: &str) -> bool {
    let paragraphs: Vec<&str> = text.split("\n\n").filter(|p| !p.trim().is_empty()).collect();
    if paragraphs.len() < 5 {
        return false;
    }
    let lengths: Vec<usize> = paragraphs.iter().map(|p| p.split_whitespace().count()).collect();
    let avg = lengths.iter().sum::<usize>() as f64 / lengths.len() as f64;
    if avg == 0.0 {
        return false;
    }
    let variance =
        lengths.iter().map(|&l| (l as f64 - avg).powi(2)).sum::<f64>() / lengths.len() as f64;
    let cv = variance.sqrt() / avg;
    cv < 0.20
}

/// Check a single piece of text and return its findings (path used only for
/// the `location` field).
pub fn check_text(text: &str, path: &str) -> Vec<Finding> {
    let mut findings = Vec::new();
    let lower = text.to_lowercase();

    // 1. AI-typical tokens.
    for (token, hint) in AI_TYPICAL_TOKENS {
        let pat = format!(r"\b{}\b", regex::escape(token));
        let re = Regex::new(&pat).unwrap();
        let count = re.find_iter(&lower).count();
        if count >= 3 {
            findings.push(Finding {
                category: "STYLE".into(),
                severity: Severity::Warn,
                message: format!("AI-typical token '{token}' appears {count} times -- {hint}"),
                location: Some(path.to_owned()),
            });
        }
    }

    // 2. Em-dash overuse.
    let em = em_dash_count(text);
    if em > 3 {
        findings.push(Finding {
            category: "STYLE".into(),
            severity: Severity::Warn,
            message: format!("Em-dash overuse: {em} occurrences (max ~3 per paper)."),
            location: Some(path.to_owned()),
        });
    }

    // 3. Uniform paragraph length.
    if paragraph_lengths_uniform(text) {
        findings.push(Finding {
            category: "STYLE".into(),
            severity: Severity::Warn,
            message: "Paragraph lengths are uniform (CV < 0.20). Vary sentence rhythm.".into(),
            location: Some(path.to_owned()),
        });
    }

    // 4. First-person pronouns (FHNW academic style).
    let fp_count = first_person_de().find_iter(text).count();
    if fp_count > 0 {
        findings.push(Finding {
            category: "STYLE".into(),
            severity: Severity::Error,
            message: format!("First-person pronouns found {fp_count}× -- forbidden in FHNW academic style."),
            location: Some(path.to_owned()),
        });
    }

    // 5. Absolute statements.
    let abs_count = absolute_de().find_iter(text).count();
    if abs_count > 5 {
        findings.push(Finding {
            category: "STYLE".into(),
            severity: Severity::Warn,
            message: format!("Absolute statements ('immer', 'nie', 'alle', …) {abs_count}× -- replace with hedged form."),
            location: Some(path.to_owned()),
        });
    }

    // 6. Headings as questions (BLOCKING per FHNW).
    let q: Vec<&str> = heading_as_question().find_iter(text).map(|m| m.as_str()).collect();
    if !q.is_empty() {
        findings.push(Finding {
            category: "STRUCTURE".into(),
            severity: Severity::Error,
            message: format!(
                "Headings as questions: {:?}{}",
                &q[..q.len().min(3)],
                if q.len() > 3 { "…" } else { "" }
            ),
            location: Some(path.to_owned()),
        });
    }

    // 7. Headings as sentences.
    let s: Vec<&str> = heading_as_sentence().find_iter(text).map(|m| m.as_str()).collect();
    if !s.is_empty() {
        findings.push(Finding {
            category: "STRUCTURE".into(),
            severity: Severity::Warn,
            message: format!(
                "Headings as sentences: {:?}{}",
                &s[..s.len().min(3)],
                if s.len() > 3 { "…" } else { "" }
            ),
            location: Some(path.to_owned()),
        });
    }

    // 8. Gendered nouns (DE).
    for word in GENDERED_NOUNS_DE {
        let needle = format!("{word} ");
        let count = text.matches(&needle).count();
        if count > 0 {
            findings.push(Finding {
                category: "GENDER_LANGUAGE".into(),
                severity: Severity::Warn,
                message: format!(
                    "Gendered noun '{word}' {count}× -- use Paarform or neutrale Form."
                ),
                location: Some(path.to_owned()),
            });
        }
    }

    findings
}

/// Walk every Markdown blob in the project's working tree and check it.
pub fn run(conn: &Connection, project_id: &str) -> Result<CheckReport> {
    let mut all_findings = Vec::new();
    let entries = worktree::list(conn, project_id, "")?;
    for (path, blob_sha) in entries {
        if !path.ends_with(".md") {
            continue;
        }
        let blob = agentic_core::content::blob::get_blob(conn, &blob_sha)?;
        let text = std::str::from_utf8(&blob.content).unwrap_or("");
        all_findings.extend(check_text(text, &path));
    }
    Ok(CheckReport::new("writing_quality", all_findings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentic_core::{
        db::open_in_memory,
        project::{ProjectKind, create as create_project},
    };
    use pretty_assertions::assert_eq;

    #[test]
    fn detects_first_person_german() {
        let findings = check_text("Ich habe das untersucht. Wir glauben, dass …", "ch1.md");
        assert!(findings.iter().any(|f| f.category == "STYLE"
            && f.message.contains("First-person")));
    }

    #[test]
    fn detects_heading_as_question() {
        let findings = check_text("## Was ist das Problem?\n\nIntro.", "ch1.md");
        assert!(findings.iter().any(|f| f.message.contains("Headings as questions")));
    }

    #[test]
    fn detects_ai_typical_token_with_threshold() {
        let text = "We will leverage X. Leverage Y. We leverage everything.";
        let findings = check_text(text, "ch.md");
        assert!(findings.iter().any(|f| f.message.contains("'leverage'")));
    }

    #[test]
    fn detects_em_dash_overuse() {
        let text = "A — b — c — d — e.";
        assert_eq!(em_dash_count(text), 4);
    }

    #[test]
    fn detects_gendered_nouns_de() {
        let findings = check_text("Die Studenten lesen. Mitarbeiter helfen.", "ch.md");
        assert!(findings.iter().any(|f| f.category == "GENDER_LANGUAGE"));
    }

    #[test]
    fn run_walks_all_md_blobs() {
        let conn = open_in_memory().unwrap();
        let pid = create_project(&conn, "T", ProjectKind::Thesis, "de", None).unwrap();
        agentic_core::worktree::put_at(
            &conn, &pid, "thesis-draft/ch-01.md",
            b"## Wieso?\n\nIch fand das.\n",
            "text/markdown", Some("de"), "u", "init",
        ).unwrap();
        let report = run(&conn, &pid).unwrap();
        assert!(!report.findings.is_empty());
        // Has both: heading-as-question (Error) and first-person (Error) → FAIL.
        assert_eq!(report.verdict, crate::Verdict::Fail);
    }

    #[test]
    fn run_skips_non_markdown() {
        let conn = open_in_memory().unwrap();
        let pid = create_project(&conn, "T", ProjectKind::Thesis, "de", None).unwrap();
        agentic_core::worktree::put_at(
            &conn, &pid, "data.json",
            b"{\"ich\": true}",
            "application/json", None, "u", "init",
        ).unwrap();
        let report = run(&conn, &pid).unwrap();
        assert_eq!(report.findings.len(), 0);
    }
}
