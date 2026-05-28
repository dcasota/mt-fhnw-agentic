//! `agentic check bookkit` — deliverable bookkit-polishing audit.
//!
//! A REPORTING gate (WARN-level, never blocks): it surfaces two polishing
//! rules so a corpus that currently violates them can still be measured.
//!
//! * RULE 1 — bold-emphasis limit (`BOLD_OVERUSE`): markdown bold (`**…**`) is
//!   permitted ONLY as a short *leading label* — the bold span starts at the
//!   very beginning of a paragraph/list item (after optional list/quote/heading
//!   markers and whitespace) AND the bolded text is <= 8 words and <= 60 chars.
//!   Any other bold (inline mid-prose, or an over-long leading label) is flagged.
//! * RULE 2 — content English-only (`NON_ENGLISH`): clearly non-English tokens
//!   in body prose are flagged via a conservative deny-list of common
//!   German/French/Italian function words and a few telltales.
//!
//! Both rules are fence-aware (fenced code/figspec is skipped) and ignore
//! figspec/JSON, URLs and APA reference lines to keep the heuristics quiet.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use anyhow::Result;
use regex::Regex;
use rusqlite::Connection;

use agentic_core::worktree;

use crate::{CheckReport, Finding, Severity};

/// Cap on per-file `BOLD_OVERUSE` findings emitted (all are still counted).
const MAX_BOLD_FINDINGS_PER_FILE: usize = 50;
/// Leading-label limits.
const MAX_LABEL_WORDS: usize = 8;
const MAX_LABEL_CHARS: usize = 60;

/// A markdown bold span `**…**` (non-greedy, no nested `**`).
static BOLD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\*\*(.+?)\*\*").unwrap());
/// Optional leading markers permitted before a leading-label bold: list
/// bullets (`-`/`+`, or a `*` bullet which is always followed by whitespace),
/// block-quote (`>`), heading hashes (`#`), ordered-list numbers (`1.`), and
/// surrounding whitespace. The `regex` crate has no look-around, so a `*`
/// bullet is matched as `*` + whitespace — this never swallows a `**` opener
/// (which is `**`, not `* `).
static LEAD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(?:[\s>#+-]|\d+\.|\*\s)*").unwrap());

/// Conservative non-English deny-list: common DE/FR/IT function words and a few
/// telltales. Whole-word, case-insensitive.
static NON_EN: LazyLock<Regex> = LazyLock::new(|| {
    // `die` is deliberately omitted: it collides with the common English word
    // ("live or die", a semiconductor "die"). `der`/`das` are kept as safer
    // German markers. All-caps matches (e.g. the MIT licence) and tokens inside
    // a parenthesised gloss (e.g. a German law title) are filtered in
    // `non_english_tokens`.
    Regex::new(
        r"(?i)\b(?:\
und|oder|nicht|mit|f[üu]r|[üu]ber|L[öo]sung|L[öo]sungen|Abbildung|Tabelle|Verzeichnis|\
der|das|eine|sind|werden|sich|auch|sowie|zwischen|\
et|ou|pour|avec|dans|les|des|une|[êe]tre|\
della|degli|sono|anche|perch[ée])\b",
    )
    .unwrap()
});

/// Is byte offset `pos` inside a *gloss* on `line` — parentheses, double quotes
/// (straight or curly), or single-asterisk italics? Glossed foreign
/// terms-of-art (a German law/book title, an italicised loan-phrase such as
/// `*raison d'être*`) are allowed by ADR-0037 and must not be flagged.
fn inside_gloss(line: &str, pos: usize) -> bool {
    let mut depth = 0i32;
    for (i, c) in line.char_indices() {
        if i >= pos {
            break;
        }
        match c {
            '(' => depth += 1,
            ')' => depth = (depth - 1).max(0),
            _ => {}
        }
    }
    if depth > 0 {
        return true;
    }
    let pre = &line[..pos];
    // Curly double-quote span: an opener `“` not yet closed by `”`.
    if pre.matches('\u{201c}').count() > pre.matches('\u{201d}').count() {
        return true;
    }
    // Straight double-quote parity (odd ⇒ inside an open `"…`).
    if pre.matches('"').count() % 2 == 1 {
        return true;
    }
    // Single-asterisk italics: total `*` parity is odd inside `*…*`. Bold
    // `**…**` contributes an even count, so it does not affect the parity.
    if pre.matches('*').count() % 2 == 1 {
        return true;
    }
    false
}
/// A markdown heading deeper than H4 (`#####` or more, then whitespace).
static DEEP_HEADING: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^#####+\s").unwrap());
/// A URL anywhere on the line → skip RULE 2 (URLs carry foreign-looking tokens).
static URL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"https?://[^\s)\]<>"']+"#).unwrap());
/// An APA-style in-text/reference citation `(1999)` / `(2020a)` → skip RULE 2.
static APA_YEAR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\(\d{4}[a-z]?\)").unwrap());

/// Does this line *look like* a figspec/JSON body? Bold/foreign tokens inside
/// machine data are not prose. A line whose trimmed form starts with `{`/`}`/`"`
/// or contains a `"key":` pair is treated as data.
fn looks_like_json(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with('{') || t.starts_with('}') || t.starts_with('"') || JSON_KEY.is_match(line)
}
static JSON_KEY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""[\w-]+"\s*:"#).unwrap());

/// Is a reference / bibliography line (hanging APA entry or `(YEAR)` citation)?
fn looks_like_reference(line: &str) -> bool {
    APA_YEAR.is_match(line)
}

/// First `n` chars of `s` (char-safe), with an ellipsis if truncated.
fn snippet(s: &str, n: usize) -> String {
    let mut out: String = s.chars().take(n).collect();
    if s.chars().count() > n {
        out.push('…');
    }
    out
}

/// RULE 1 helper: bold-emphasis violations in `text`.
///
/// Returns `(line_number, snippet)` pairs — one per *violating* bold span.
/// A bold span is allowed only when it is a leading label (starts the
/// paragraph/list item after optional markers) and is <= 8 words / <= 60 chars.
/// Fence-aware (fenced code/figspec is skipped) and JSON-ish lines are ignored.
#[must_use]
pub fn bold_violations(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut in_fence = false;
    for (idx, ln) in text.lines().enumerate() {
        let i = idx + 1;
        if ln.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence || looks_like_json(ln) {
            continue;
        }
        // The end of the optional leading-marker run: a bold opening at exactly
        // this offset is a candidate leading label.
        let lead_end = LEAD.find(ln).map_or(0, |m| m.end());
        for m in BOLD.captures_iter(ln) {
            // Both groups are guaranteed by the `\*\*(.+?)\*\*` pattern.
            let (Some(whole), Some(grp)) = (m.get(0), m.get(1)) else {
                continue;
            };
            let inner = grp.as_str();
            let is_leading = whole.start() == lead_end;
            let words = inner.split_whitespace().count();
            let chars = inner.chars().count();
            let allowed = is_leading && words <= MAX_LABEL_WORDS && chars <= MAX_LABEL_CHARS;
            if !allowed {
                out.push((i, snippet(inner, 40)));
            }
        }
    }
    out
}

/// RULE 2 helper: clearly non-English tokens in body prose.
///
/// Returns `(line_number, token)` pairs. Fence-aware; JSON-ish, URL-bearing and
/// APA-reference lines are skipped to keep the heuristic conservative.
#[must_use]
pub fn non_english_tokens(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut in_fence = false;
    for (idx, ln) in text.lines().enumerate() {
        let i = idx + 1;
        if ln.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence || looks_like_json(ln) || URL.is_match(ln) || looks_like_reference(ln) {
            continue;
        }
        for m in NON_EN.find_iter(ln) {
            let tok = m.as_str();
            // Skip all-caps acronyms (MIT/BSD licence, org names) and tokens
            // inside a parenthesised gloss (e.g. a German law/standard title).
            if tok.chars().all(|c| c.is_ascii_uppercase()) || inside_gloss(ln, m.start()) {
                continue;
            }
            out.push((i, tok.to_string()));
        }
    }
    out
}

/// RULE 3 helper: heading-depth violations in `text`.
///
/// Returns `(line_number, snippet)` pairs — one per heading deeper than `####`
/// (H4). A book chapter is one H1 plus at most three sub-levels (H2/H3/H4), so
/// any `#####`-or-deeper line violates the structure. Fence-aware (headings
/// inside fenced code/figspec are ignored).
#[must_use]
pub fn heading_depth_violations(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut in_fence = false;
    for (idx, ln) in text.lines().enumerate() {
        let i = idx + 1;
        if ln.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        if DEEP_HEADING.is_match(ln) {
            out.push((i, snippet(ln.trim_start(), 40)));
        }
    }
    out
}

/// Where to source the audit files from.
#[derive(Debug, Clone, Copy)]
pub enum Scope<'a> {
    /// Scan every `*.md` whose path starts with `prefix` (the legacy default).
    Prefix(&'a str),
    /// Scan only the chapter list of one bookkit manifest entry.
    Paths {
        /// Manifest key, surfaced in the audit message for traceability.
        book_key: &'a str,
        /// The chapter path list, in manifest order.
        paths: &'a [&'a str],
    },
}

/// Run the bookkit gate over a project's deliverable markdown (`prefix`).
pub fn run(conn: &Connection, project: &str, prefix: &str) -> Result<CheckReport> {
    run_scoped(conn, project, Scope::Prefix(prefix))
}

/// Scoped variant used by the cascade thesis-profile invocation.
///
/// `Scope::Prefix` matches the legacy behaviour. `Scope::Paths` audits
/// exactly the chapter list a bookkit manifest entry composes — fixing the
/// mixed-prefix-scope blind spot where the master-thesis book draws from
/// both `thesis/` and `out/sources/` but the gate only sees one.
pub fn run_scoped(conn: &Connection, project: &str, scope: Scope<'_>) -> Result<CheckReport> {
    let mut findings = Vec::new();
    let mut total_bold = 0usize;
    let mut total_non_en = 0usize;
    let mut total_heading_depth = 0usize;
    let mut distinct_tokens: BTreeSet<String> = BTreeSet::new();

    let entries: Vec<(String, String)> = match scope {
        Scope::Prefix(prefix) => worktree::list(conn, project, prefix)?,
        Scope::Paths { book_key: _, paths } => paths
            .iter()
            .map(|p| ((*p).to_string(), String::new()))
            .collect(),
    };
    for (path, _sha) in entries.iter().filter(|(p, _)| {
        std::path::Path::new(p)
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("md"))
    }) {
        let blob = worktree::read_at(conn, project, path)?;
        let text = String::from_utf8_lossy(&blob.content);

        // RULE 1 — bold-emphasis limit (count all, emit up to the per-file cap).
        let bolds = bold_violations(&text);
        total_bold += bolds.len();
        for (line, snip) in bolds.iter().take(MAX_BOLD_FINDINGS_PER_FILE) {
            findings.push(Finding {
                category: "BOLD_OVERUSE".into(),
                severity: Severity::Warn,
                message: format!("bold not a short leading label: '{snip}'"),
                location: Some(format!("{path}:{line}")),
            });
        }

        // RULE 2 — content English-only.
        for (line, tok) in non_english_tokens(&text) {
            total_non_en += 1;
            distinct_tokens.insert(tok.to_lowercase());
            findings.push(Finding {
                category: "NON_ENGLISH".into(),
                severity: Severity::Warn,
                message: format!("non-English token '{tok}'"),
                location: Some(format!("{path}:{line}")),
            });
        }

        // RULE 3 — heading depth (one H1 chapter + at most 3 sub-levels).
        for (line, snip) in heading_depth_violations(&text) {
            total_heading_depth += 1;
            findings.push(Finding {
                category: "HEADING_DEPTH".into(),
                severity: Severity::Warn,
                message: format!("heading deeper than H4 (max H2/H3/H4): '{snip}'"),
                location: Some(format!("{path}:{line}")),
            });
        }
    }

    // INFO summary findings (one per rule) carry the totals.
    findings.push(Finding {
        category: "BOLD_SUMMARY".into(),
        severity: Severity::Info,
        message: format!("bookkit RULE 1 (bold-emphasis): {total_bold} violation(s) total"),
        location: Some("bookkit".into()),
    });
    findings.push(Finding {
        category: "NON_ENGLISH_SUMMARY".into(),
        severity: Severity::Info,
        message: format!(
            "bookkit RULE 2 (English-only): {total_non_en} token(s) total, {} distinct ({})",
            distinct_tokens.len(),
            distinct_tokens
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        ),
        location: Some("bookkit".into()),
    });
    findings.push(Finding {
        category: "HEADING_DEPTH_SUMMARY".into(),
        severity: Severity::Info,
        message: format!(
            "bookkit RULE 3 (heading-depth): {total_heading_depth} heading(s) deeper than H4"
        ),
        location: Some("bookkit".into()),
    });

    Ok(CheckReport::new("bookkit", findings))
}

#[cfg(test)]
mod tests {
    use super::{bold_violations, heading_depth_violations, non_english_tokens};

    #[test]
    fn leading_bold_label_ok() {
        // A short leading `**Term.**` label (<= 8 words) is allowed.
        assert_eq!(
            bold_violations("**Term.** the rest is plain prose.\n").len(),
            0
        );
    }

    #[test]
    fn leading_bold_label_list_item_ok() {
        // Allowed at the start of a list item too (after the bullet marker).
        assert_eq!(
            bold_violations("- **Label:** explanation follows.\n").len(),
            0
        );
    }

    #[test]
    fn inline_bold_flagged() {
        // Bold in the middle of running prose → 1 violation.
        let v = bold_violations("text with **bold** inside.\n");
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].0, 1);
    }

    #[test]
    fn long_leading_label_flagged() {
        // A leading bold label of 12 words exceeds the 8-word limit → 1.
        let v = bold_violations(
            "**one two three four five six seven eight nine ten eleven twelve** rest.\n",
        );
        assert_eq!(v.len(), 1);
    }

    #[test]
    fn bold_in_fence_ignored() {
        // Bold inside a fenced code block → 0.
        let md = "```\ntext with **bold** inside\n```\n";
        assert_eq!(bold_violations(md).len(), 0);
    }

    #[test]
    fn non_english_flagged() {
        // "the Lösung is" flags the German token.
        let v = non_english_tokens("the Lösung is good.\n");
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].1, "Lösung");
    }

    #[test]
    fn english_clean() {
        // Pure English prose → 0.
        assert_eq!(non_english_tokens("the solution is good.\n").len(), 0);
    }

    #[test]
    fn english_homographs_not_flagged() {
        // "die" (English verb), "MIT" (licence) and a parenthesised German gloss
        // are all legitimate English-deliverable content → 0 flags.
        assert_eq!(
            non_english_tokens("properties live or die here.\n").len(),
            0
        );
        assert_eq!(
            non_english_tokens("mixes Apache-2.0, BSD, MIT, GPL terms.\n").len(),
            0
        );
        assert_eq!(
            non_english_tokens(
                "the Cybersecurity Ordinance (Verordnung über die Cybersicherheit).\n"
            )
            .len(),
            0
        );
        // A genuine non-glossed German function word in prose is still caught.
        assert_eq!(non_english_tokens("the Lösung is good.\n").len(), 1);
        // Quoted / italicised foreign titles & loan-phrases are glosses → 0.
        assert_eq!(
            non_english_tokens("read \u{201c}Hacking und Cybersecurity mit KI\u{201d} today.\n")
                .len(),
            0
        );
        assert_eq!(
            non_english_tokens("the *raison d'être* of the rule.\n").len(),
            0
        );
        assert_eq!(
            non_english_tokens("the *Verordnung über die Cyber* law.\n").len(),
            0
        );
    }

    #[test]
    fn deep_heading_flagged() {
        // An H5 line is deeper than H4 → 1 violation; H2/H3/H4 are clean.
        let md = "# H1\n## H2\n### H3\n#### H4\n##### H5 too deep\n";
        let v = heading_depth_violations(md);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].0, 5);
    }

    #[test]
    fn deep_heading_in_fence_ignored() {
        // A `#####` line inside a fenced block is code, not a heading → 0.
        let md = "```\n##### not a heading\n```\n#### H4 ok\n";
        assert_eq!(heading_depth_violations(md).len(), 0);
    }

    #[test]
    fn fenced_and_url_lines_ignored() {
        // Fenced and URL-bearing lines are skipped for RULE 2.
        let fenced = "```\nund oder nicht\n```\n";
        assert_eq!(non_english_tokens(fenced).len(), 0);
        let url = "see https://example.com/und/oder for details\n";
        assert_eq!(non_english_tokens(url).len(), 0);
    }

    #[test]
    fn scoped_paths_only_audits_listed_files() {
        // Regression for the 2026-05-28 cascade: the bookkit gate was scanning
        // only `out/sources/` by default, so a master-thesis book composed
        // from `thesis/...` chapters showed 0 bold violations even when its
        // title page had 5. The scoped variant audits exactly the manifest's
        // chapter list, so the title-page bolds surface.
        use crate::bookkit_gate::{Scope, run as run_legacy, run_scoped as run_scoped_fn};
        use agentic_core::{
            db::open_in_memory,
            project::{ProjectKind, create as create_project},
            worktree,
        };
        let conn = open_in_memory().unwrap();
        let pid = create_project(&conn, "T", ProjectKind::Thesis, "en", None).unwrap();
        // Thesis chapter with one mid-prose bold violation.
        worktree::put_at(
            &conn,
            &pid,
            "thesis/fhnw_00_title_page.md",
            b"# Title Page\n\nText with **a bold mid-prose** inside.\n",
            "text/markdown",
            Some("en"),
            "u",
            "init",
        )
        .unwrap();
        // Out-of-scope content under the legacy prefix — must NOT be flagged
        // by the scoped scan.
        worktree::put_at(
            &conn,
            &pid,
            "out/sources/unrelated.md",
            b"# Other\n\nMid-prose **bold** here too.\n",
            "text/markdown",
            Some("en"),
            "u",
            "init",
        )
        .unwrap();

        // Legacy prefix scan over `out/sources/` finds the unrelated one
        // and misses the thesis one.
        let legacy = run_legacy(&conn, &pid, "out/sources/").unwrap();
        let legacy_bold = legacy
            .findings
            .iter()
            .filter(|f| f.category == "BOLD_OVERUSE")
            .count();
        assert_eq!(legacy_bold, 1, "prefix scan sees only the unrelated file");

        // Scoped scan over only the thesis chapter sees the thesis bold
        // and ignores the unrelated one.
        let paths = ["thesis/fhnw_00_title_page.md"];
        let scoped = run_scoped_fn(
            &conn,
            &pid,
            Scope::Paths {
                book_key: "master_thesis",
                paths: &paths,
            },
        )
        .unwrap();
        let scoped_bold = scoped
            .findings
            .iter()
            .filter(|f| f.category == "BOLD_OVERUSE")
            .count();
        assert_eq!(scoped_bold, 1, "scoped scan sees only the thesis file");
        assert!(
            scoped
                .findings
                .iter()
                .any(|f| f.location.as_deref() == Some("thesis/fhnw_00_title_page.md:3")),
            "scoped scan must locate the violation in the thesis chapter"
        );
    }
}
