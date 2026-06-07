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
// German management-tradition abbreviations IST / SOLL and their compounds.
// Detected case-SENSITIVELY in all-caps form (and the canonical title-cased
// compound Ist-Analyse / Soll-Zustand) so lowercase English "is" / "soll"
// elsewhere does not get flagged — those would explode the false-positive
// rate (`-i` and ADR-0037 conservative-list rule). The bare lowercase
// German forms are NOT detected here; rely on the wider DE word-list for
// the legitimate full-word forms.
static DE_ABBREV: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\b(IST|SOLL|IST/SOLL|SOLL/IST|IST-Analyse|SOLL-Zustand|Ist-Analyse|Soll-Zustand|Ist-Zustand|Soll-Ist)\b",
    )
    .unwrap()
});
static CROSSREF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"out/[\w./-]+\.docx").unwrap());
// ADR-0038 forbids idempotency/workflow-state HTML-comment markers in
// deliverables (e.g. `<!-- gap-ranked-iter9 -->`, `<!-- condensed-iter9 -->`,
// `<!-- iter12 -->`, `<!-- transition -->`). It does NOT forbid build-pipeline
// audit-trail comments (source attribution per ADR-0023, structural
// delimiters used by the bookkit renderer, figure-organization markers),
// which are legitimate metadata the audit-trail relies on. The earlier
// blanket regex `<!--.*?-->` over-enforced ADR-0038, flagging every HTML
// comment indiscriminately. Refined to match only HTML comments whose body
// contains a recognised forbidden idempotency / iteration / workflow-state
// keyword.
static MARKER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"<!--[^>]*\b(?:iter\d+|condensed|gap-ranked|ranked-iter|transition|in-progress|TODO|FIXME|XXX)\b[^>]*-->",
    )
    .unwrap()
});
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
    "bar",
    "hbar",
    "line",
    "matrix",
    "flow",
    "quadrant", // base chart types
    "icon",
    "heatmap",
    "regstack",
    "govmap", // book-figure types (AI-Norms generator ports)
    "treemap",
    "procmap",     // population treemap + ISO-5338-style process map
    "image-embed", // Wave-1 raster embed (render_image_embed.rs; sourced rasters in AI Norms book)
    "table",       // data-table-to-Word-table renderer (render_table.rs; regulatory matrices)
];

/// FHNW MAS regulatory + German-management-methodology phrases that are
/// accepted as English-academic loanwords (or mandatory German regulatory
/// terms) and must NOT fire `NON_ENGLISH_TEXT`. A match by [`DE`] or
/// [`DE_ABBREV`] whose span sits inside a recognised phrase is silenced.
///
/// Scope:
/// - `Verzeichnis der Hilfsmittel` is the **mandatory** FHNW MAS
///   accreditation phrase for the AI/tools-disclosure back-matter section;
///   it is a quoted regulatory obligation, not free German prose.
/// - `IST-Analyse`, `Ist-Analyse`, `SOLL-Zustand`, `Soll-Zustand`,
///   `Ist-Zustand`, `Soll-Ist`, `IST/SOLL`, `SOLL/IST` are German
///   management-tradition methodology compounds with no exact single-word
///   English equivalent. They are routinely used in academic English
///   business-management writing as untranslated technical terms (cf. their
///   appearance in Wirtschaftsinformatik literature, Springer Business
///   reference works, etc.). The bare-word `IST` and `SOLL` flags remain
///   active for free German prose; only the compound methodology forms are
///   exempted here.
/// - `Ch.3 IST`, `Ch. 3 IST`, `Chapter 3 IST` and the analogous `Ch.* IST`
///   shorthand are chapter pointers (the thesis's §3 is the IST-Analyse
///   chapter); they refer to the chapter by its methodology slot, not by
///   its German title.
const DE_ALLOWLIST: &[&str] = &[
    "Verzeichnis der Hilfsmittel",
    "IST-Analyse",
    "Ist-Analyse",
    "SOLL-Zustand",
    "Soll-Zustand",
    "Ist-Zustand",
    "Soll-Ist",
    "Ist-Soll",
    "IST/SOLL",
    "SOLL/IST",
    "Ch.3 IST",
    "Ch. 3 IST",
    "Chapter 3 IST",
    "Ch.3's IST",
    "Chapter 3's IST",
];

/// True if the [`m_start`..`m_end`] byte span in `ln` sits inside any
/// [`DE_ALLOWLIST`] phrase (case-insensitive). Used to silence
/// `NON_ENGLISH_TEXT` on FHNW MAS regulatory phrases and recognised
/// German-management-methodology compounds.
fn inside_de_allowlist(ln: &str, m_start: usize, m_end: usize) -> bool {
    let lower = ln.to_lowercase();
    DE_ALLOWLIST.iter().any(|p| {
        let pl = p.to_lowercase();
        let mut search_from = 0;
        while let Some(idx) = lower[search_from..].find(&pl) {
            let abs = search_from + idx;
            let end = abs + pl.len();
            if m_start >= abs && m_end <= end {
                return true;
            }
            search_from = abs + 1;
        }
        false
    })
}

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
    // ADR-0037 English-only is scoped to EN deliverables. A `_DE.md` /
    // `.de.md` (and the fr / it / rm / hi siblings) is by definition a
    // non-English sidecar — `Empfehlungen` in `about_this_book.de.md` is
    // CORRECT German, not a NON_ENGLISH_TEXT error. Pre-2026-06-07 the
    // gate over-reached on those paths; this flag suppresses the
    // language-specific German/abbreviation/translation-not-applicable
    // checks below for sidecar files. Other checks (CAPTION_TOO_LONG,
    // NUMBER_UNSOURCED, etc.) still run.
    let is_non_en_target = {
        let lc = label.to_ascii_lowercase();
        const TAGS: &[&str] = &["de", "fr", "it", "rm", "hi"];
        TAGS.iter().any(|t| {
            lc.ends_with(&format!(".{t}.md"))
                || lc.ends_with(&format!("_{t}.md"))
                || lc.ends_with(&format!(".{t}.markdown"))
        })
    };
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
        // Suppressed entirely for non-EN sidecars — see is_non_en_target above.
        if !is_non_en_target {
            if let Some(m) = DE.find(ln) {
                let glossed = ln[..m.start()]
                    .chars()
                    .rev()
                    .take(2)
                    .any(|c| c == '*' || c == '(');
                let allowlisted = inside_de_allowlist(ln, m.start(), m.end());
                if !glossed && !allowlisted {
                    out.push(err(
                        "NON_ENGLISH_TEXT",
                        format!("L{i}: German term '{}'", m.as_str()),
                        here(),
                    ));
                }
            }
        }
        // German management-tradition abbreviations (IST / SOLL) — case-sensitive
        // to avoid matching lowercase English "is" / "soll". Also exempted when
        // glossed parenthetically (e.g. `actual (IST)` is fine), and when the
        // token is preceded immediately by `-` (which means it is the middle
        // segment of a compound identifier like `CAR-IST-001`, not the
        // standalone German abbreviation). The follow-character is checked
        // symmetrically for `IST-Analyse` — if the match is the bare `IST` and
        // the next char is `-` that introduces a non-management compound
        // (e.g. `IST-001` as a CAR id continuation), skip it too.
        if !is_non_en_target && let Some(m) = DE_ABBREV.find(ln) {
            let pre_char = ln[..m.start()].chars().last();
            let post_char = ln[m.end()..].chars().next();
            let glossed = ln[..m.start()]
                .chars()
                .rev()
                .take(2)
                .any(|c| c == '*' || c == '(');
            let inside_compound_id = matches!(pre_char, Some('-')); // `CAR-IST-…`
            // Bare `IST` followed by `-<digits>` is a CAR-id continuation, not
            // the German abbreviation. We rely on the regex's anchors: the
            // long forms (`IST-Analyse`, `Soll-Zustand`, …) are alternations
            // in DE_ABBREV themselves and will not match the bare-IST branch.
            let id_continuation = m.as_str() == "IST"
                && matches!(post_char, Some('-'))
                && ln[m.end()..]
                    .chars()
                    .nth(1)
                    .is_some_and(|c| c.is_ascii_digit());
            let allowlisted = inside_de_allowlist(ln, m.start(), m.end());
            if !glossed && !inside_compound_id && !id_continuation && !allowlisted {
                out.push(err(
                    "NON_ENGLISH_TEXT",
                    format!(
                        "L{i}: German management-tradition abbreviation '{}' — use English (actual/target)",
                        m.as_str()
                    ),
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
        let md = "## Schlussbetrachtung\nSee out/dimensions/D_07.docx for more.\n<!-- gap-ranked-iter9 -->\nThe D-N cell wins.\n";
        let fs = findings_for("x.md", md);
        let cats: Vec<&str> = fs.iter().map(|f| f.category.as_str()).collect();
        assert!(cats.contains(&"NON_ENGLISH_TEXT"));
        assert!(cats.contains(&"CROSS_REFERENCE"));
        assert!(cats.contains(&"INTERNAL_MARKER"));
        assert!(cats.contains(&"CRYPTIC_LABEL"));
    }

    #[test]
    fn internal_marker_fires_only_on_forbidden_idempotency_keywords() {
        // ADR-0038 forbids idempotency / workflow-state markers in
        // deliverables, NOT build-pipeline audit-trail comments. The earlier
        // blanket regex flagged every HTML comment; this guard locks the
        // refined behaviour.
        let must_flag = [
            "<!-- iter9 -->",
            "<!-- gap-ranked-iter9 -->",
            "<!-- condensed-iter9 -->",
            "<!-- ranked-iter12 -->",
            "<!-- transition -->",
            "<!-- in-progress -->",
            "<!-- TODO: backfill -->",
            "<!-- FIXME: line wrap -->",
        ];
        for s in must_flag {
            let fs = findings_for("x.md", s);
            assert!(
                fs.iter().any(|f| f.category == "INTERNAL_MARKER"),
                "expected INTERNAL_MARKER on {s:?}, got {fs:?}"
            );
        }
        let must_pass = [
            "<!-- source: book_build/chapter_extras.py :: africa -->",
            "<!-- ai_norms-figures-wave5 -->",
            "<!-- wave3-table-figspec begin -->",
            "<!-- wave3-table-figspec end -->",
            "<!-- ai_norms-numbered-roundh begin -->",
            "<!-- note -->", // a plain note is metadata, not workflow state
            "<!-- caption-key: foo -->",
        ];
        for s in must_pass {
            let fs = findings_for("x.md", s);
            assert!(
                !fs.iter().any(|f| f.category == "INTERNAL_MARKER"),
                "INTERNAL_MARKER must NOT fire on legitimate build-pipeline metadata {s:?}, got {fs:?}"
            );
        }
    }

    #[test]
    fn non_english_text_allowlists_fhnw_regulatory_and_methodology_phrases() {
        // FHNW MAS mandates the German phrase "Verzeichnis der Hilfsmittel"
        // for the AI-tools-disclosure back-matter section; the methodology
        // compounds IST-Analyse / SOLL-Zustand etc. are domain-standard
        // German-management terms in academic English. They must NOT fire
        // NON_ENGLISH_TEXT.
        let must_pass = [
            "Table: AI-/translation-/editing-tool inventory used during thesis preparation (FHNW MAS Verzeichnis der Hilfsmittel).",
            "> form the Chapter 3 IST-Analyse — not in the social-media corpus.",
            "| substrate + delta analysis | Ch.3 IST + Ch.5 | High | Draft ready |",
            "> through Chapter 3's IST-Analyse, not through the social-media corpus.",
            "Soll-Zustand der Konformität ist erreicht.", // edge: phrase appears in line, exempts the SOLL match
        ];
        for s in must_pass {
            let fs = findings_for("x.md", s);
            assert!(
                !fs.iter().any(|f| f.category == "NON_ENGLISH_TEXT"),
                "NON_ENGLISH_TEXT must NOT fire on allowlisted phrase {s:?}, got {fs:?}"
            );
        }
        // Free-prose German that is NOT in the allowlist still flags.
        let must_flag = [
            "Die Lösung erfordert eine vollständige Umsetzung der Empfehlungen.",
            "Ergebnis der Bewertung: SOLL nicht erreicht.", // bare SOLL flags
        ];
        for s in must_flag {
            let fs = findings_for("x.md", s);
            assert!(
                fs.iter().any(|f| f.category == "NON_ENGLISH_TEXT"),
                "NON_ENGLISH_TEXT must still fire on free German prose {s:?}, got {fs:?}"
            );
        }
    }

    #[test]
    fn figure_not_graphical_accepts_image_embed_and_table_types() {
        // ADR-0030 figure-type list must follow the figspec renderer catalogue
        // in agentic-figures. `image-embed` (Wave-1 raster embed) and `table`
        // (Word-table renderer) ship as production renderers; the gate's
        // GRAPHICAL allowlist must include them, otherwise every sourced
        // raster and every regulatory matrix flags as FIGURE_NOT_GRAPHICAL.
        let ok = [
            "```figspec\n{\"id\":\"f1\",\"type\":\"image-embed\",\"caption\":\"x\"}\n```",
            "```figspec\n{\"id\":\"f2\",\"type\":\"table\",\"caption\":\"x\"}\n```",
        ];
        for s in ok {
            let fs = findings_for("x.md", s);
            assert!(
                !fs.iter().any(|f| f.category == "FIGURE_NOT_GRAPHICAL"),
                "FIGURE_NOT_GRAPHICAL must NOT fire on supported type {s:?}, got {fs:?}"
            );
        }
        // Unknown types still flag — guard against accidental allow-all.
        let bad = "```figspec\n{\"id\":\"f3\",\"type\":\"banana\",\"caption\":\"x\"}\n```";
        let fs = findings_for("x.md", bad);
        assert!(
            fs.iter().any(|f| f.category == "FIGURE_NOT_GRAPHICAL"),
            "FIGURE_NOT_GRAPHICAL must still fire on unknown type, got {fs:?}"
        );
    }

    #[test]
    fn catches_german_management_abbreviation_ist_soll() {
        // C18 regression: bookkit gate now detects IST / SOLL / IST-Analyse /
        // Soll-Zustand case-sensitively (lowercase English `is` / `soll` is NOT
        // flagged). User-supplied requirement 2026-05-28.
        let md = "| Dimension | Measured IST | Target (SOLL) | Gap |\n";
        let fs = findings_for("x.md", md);
        let count = fs
            .iter()
            .filter(|f| f.category == "NON_ENGLISH_TEXT")
            .count();
        assert!(
            count >= 1,
            "must flag IST or SOLL (got {} non-English findings: {:?})",
            count,
            fs.iter()
                .filter(|f| f.category == "NON_ENGLISH_TEXT")
                .map(|f| &f.message)
                .collect::<Vec<_>>()
        );
        // SOLL appears parenthetically as `(SOLL)` — the gloss-exemption rule
        // should silence it. So the remaining flag should be IST (or
        // Ist-Analyse / SOLL-Zustand). Verify at least one mentions IST.
        assert!(
            fs.iter()
                .any(|f| f.category == "NON_ENGLISH_TEXT" && f.message.contains("IST")),
            "expected IST to be flagged; got: {:?}",
            fs.iter()
                .filter(|f| f.category == "NON_ENGLISH_TEXT")
                .map(|f| &f.message)
                .collect::<Vec<_>>()
        );
        // And lowercase English `is` must NOT be flagged.
        let prose = findings_for("x.md", "The system is robust.\n");
        assert!(
            !prose.iter().any(|f| f.category == "NON_ENGLISH_TEXT"),
            "lowercase English 'is' must not be flagged as German IST"
        );
        // CAR-IST-NNN, CAR-SOLL-NNN are project-internal IDs, not German
        // abbreviations. The `-` before IST signals a compound identifier.
        let ids = findings_for(
            "x.md",
            "See CAR-IST-001 and CAR-IST-003 for the gap analysis.\n",
        );
        assert!(
            !ids.iter().any(|f| f.category == "NON_ENGLISH_TEXT"),
            "CAR-IST-NNN identifiers must not be flagged as German IST"
        );
        // Bare `IST` followed by `-<digit>` is a CAR-id continuation
        let cont = findings_for("x.md", "Reference IST-001 in the table.\n");
        assert!(
            !cont.iter().any(|f| f.category == "NON_ENGLISH_TEXT"),
            "IST-<digits> identifier suffix must not be flagged"
        );
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
