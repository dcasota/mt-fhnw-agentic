//! `agentic check page_boundary` — three-tier body-length gate (ADR-0035).
//!
//! ADR-0035 caps the EN-core thesis body. This gate estimates the page count
//! from the word count of the markdown under `--prefix` (default
//! `thesis/`) at `words_per_page` words/page and compares it to `--max-pages`
//! (default 60). Over the limit → WARN `PAGE_OVER`; otherwise INFO `PAGE_OK`.
//! Advisory only — a soft signal, not a hard block.
//!
//! Scope can be opted into a *manifest-aware* mode via [`run_scoped`]: the
//! caller passes the exact list of chapter paths that compose a rendered
//! book, which fixes a class of bug where the bookkit manifest pulls from
//! mixed prefixes (e.g. `thesis/` + `out/sources/`) but the gate's
//! `--prefix` scan only sees one of them.

use anyhow::Result;
use rusqlite::Connection;

use agentic_core::worktree;

use crate::{CheckReport, Finding, Severity};

/// Default words per estimated page (rough manuscript convention).
///
/// For rendered FHNW Word output the empirical density is ~280 words/page
/// (verified 2026-05-28 on master_thesis.docx: 25,381 words / 91 pages =
/// 278.9 wpp). The default stays at 500 for backwards-compatibility with
/// every existing call-site; FHNW thesis-profile callers should pass 280.
pub const WORDS_PER_PAGE: usize = 500;

/// Whitespace-token word count (fence-agnostic — a coarse manuscript estimate).
#[must_use]
pub fn word_count(text: &str) -> usize {
    text.split_whitespace().count()
}

/// Estimated pages from a word count at the *default* [`WORDS_PER_PAGE`]
/// words/page (ceil). Use [`pages_from_words_wpp`] to pass a non-default rate.
#[must_use]
pub const fn pages_from_words(words: usize) -> usize {
    words.div_ceil(WORDS_PER_PAGE)
}

/// Estimated pages from a word count at the given words-per-page rate (ceil).
/// A rate of 0 is treated as the default to keep the function total.
#[must_use]
pub const fn pages_from_words_wpp(words: usize, wpp: usize) -> usize {
    let rate = if wpp == 0 { WORDS_PER_PAGE } else { wpp };
    words.div_ceil(rate)
}

/// Backwards-compatible `--prefix` scan at the default 500 wpp.
pub fn run(
    conn: &Connection,
    project: &str,
    prefix: &str,
    max_pages: usize,
) -> Result<CheckReport> {
    run_scoped(
        conn,
        project,
        Scope::Prefix(prefix),
        max_pages,
        WORDS_PER_PAGE,
    )
}

#[cfg(test)]
mod body_range_tests {
    use super::*;

    #[test]
    fn body_range_inclusive_substring_match() {
        let paths = [
            "thesis/fhnw_0_management_summary.md",
            "thesis/fhnw_1_introduction.md",
            "thesis/fhnw_2_theory.md",
            "thesis/fhnw_3_current_state.md",
            "thesis/fhnw_4_empirical.md",
            "thesis/fhnw_5_solution.md",
            "thesis/fhnw_6_conclusion.md",
            "thesis/fhnw_7_personal_reflection.md",
        ];
        let kept = apply_body_range(&paths, Some("fhnw_2_theory"), Some("fhnw_6_conclusion"));
        assert_eq!(
            kept,
            vec![
                "thesis/fhnw_2_theory.md",
                "thesis/fhnw_3_current_state.md",
                "thesis/fhnw_4_empirical.md",
                "thesis/fhnw_5_solution.md",
                "thesis/fhnw_6_conclusion.md",
            ]
        );
    }

    #[test]
    fn body_range_falls_through_on_missing_from() {
        let paths = ["a.md", "b.md", "c.md"];
        let kept = apply_body_range(&paths, Some("XXX"), Some("b"));
        assert_eq!(kept, vec!["a.md", "b.md", "c.md"]);
    }

    #[test]
    fn body_range_no_bounds_returns_all() {
        let paths = ["a.md", "b.md", "c.md"];
        let kept = apply_body_range(&paths, None, None);
        assert_eq!(kept, vec!["a.md", "b.md", "c.md"]);
    }
}

/// Scoped variant used by the cascade thesis-profile invocation.
///
/// `Scope::Prefix` matches the original behaviour (every `*.md` under the
/// prefix is summed). `Scope::Paths` measures *exactly* the supplied list of
/// paths (the chapter list of one bookkit manifest entry). `wpp` overrides
/// the words-per-page rate (pass [`WORDS_PER_PAGE`] for the default).
pub fn run_scoped(
    conn: &Connection,
    project: &str,
    scope: Scope<'_>,
    max_pages: usize,
    wpp: usize,
) -> Result<CheckReport> {
    let mut findings = Vec::new();
    let mut words = 0usize;
    let (scope_label, paths) = match scope {
        Scope::Prefix(prefix) => {
            let mut ps = Vec::new();
            for (path, _sha) in worktree::list(conn, project, prefix)? {
                ps.push(path);
            }
            (format!("'{prefix}'"), ps)
        }
        Scope::Paths {
            book_key,
            paths,
            body_from,
            body_to,
        } => {
            let body_paths = apply_body_range(paths, body_from, body_to);
            let scope_msg = match (body_from, body_to) {
                (Some(from), Some(to)) => format!(
                    "book '{book_key}' body subset {from} → {to} ({} of {} chapters)",
                    body_paths.len(),
                    paths.len()
                ),
                (Some(from), None) => format!(
                    "book '{book_key}' from {from} ({} of {} chapters)",
                    body_paths.len(),
                    paths.len()
                ),
                (None, Some(to)) => format!(
                    "book '{book_key}' up to {to} ({} of {} chapters)",
                    body_paths.len(),
                    paths.len()
                ),
                (None, None) => format!("book '{book_key}' ({} chapters)", paths.len()),
            };
            (
                scope_msg,
                body_paths.iter().map(|s| (*s).to_string()).collect(),
            )
        }
    };

    for path in &paths {
        if !std::path::Path::new(path)
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("md"))
        {
            continue;
        }
        let blob = worktree::read_at(conn, project, path)?;
        let text = String::from_utf8_lossy(&blob.content);
        words += word_count(&text);
    }

    let pages = pages_from_words_wpp(words, wpp);
    if pages > max_pages {
        findings.push(Finding {
            category: "PAGE_OVER".into(),
            severity: Severity::Warn,
            message: format!(
                "≈{pages} pages > {max_pages} (ADR-0035 body limit) — {words} words \
                 @ {wpp} wpp in {scope_label}"
            ),
            location: Some(scope_label.clone()),
        });
    } else {
        findings.push(Finding {
            category: "PAGE_OK".into(),
            severity: Severity::Info,
            message: format!(
                "≈{pages} pages <= {max_pages} ({words} words @ {wpp} wpp in {scope_label})"
            ),
            location: Some("page_boundary".into()),
        });
    }

    Ok(CheckReport::new("page_boundary", findings))
}

/// Where to source the word count from.
#[derive(Debug, Clone, Copy)]
pub enum Scope<'a> {
    /// Sum every `*.md` whose path starts with `prefix` (the legacy behaviour).
    Prefix(&'a str),
    /// Sum exactly the chapter paths listed for one bookkit manifest entry.
    /// Optional `body_from` / `body_to` further narrow the count to an
    /// inclusive sub-range of the manifest (e.g. Related Work → Discussion),
    /// matched against the chapter-path substrings; entries outside the
    /// range are excluded from the word count.
    Paths {
        /// Manifest key for the audit message (`master_thesis`, etc.).
        book_key: &'a str,
        /// The chapter path list, in manifest order.
        paths: &'a [&'a str],
        /// Optional inclusive start of the body sub-range — first chapter
        /// whose path contains this substring becomes the first counted.
        body_from: Option<&'a str>,
        /// Optional inclusive end of the body sub-range — last chapter
        /// (counted from `body_from`) whose path contains this substring
        /// becomes the last counted.
        body_to: Option<&'a str>,
    },
}

/// Filter `paths` to the inclusive sub-range bounded by `body_from`
/// (substring of the start chapter) and `body_to` (substring of the end
/// chapter). When either bound is `None` it falls through to the natural
/// extreme. If `body_from` matches no entry, the original list is
/// returned (fail-safe: the FHNW operator gets the full body count rather
/// than zero when a typo'd substring doesn't match).
fn apply_body_range<'a>(
    paths: &'a [&'a str],
    body_from: Option<&str>,
    body_to: Option<&str>,
) -> Vec<&'a str> {
    let start = body_from
        .and_then(|needle| paths.iter().position(|p| p.contains(needle)))
        .unwrap_or(0);
    let end = body_to
        .and_then(|needle| paths.iter().rposition(|p| p.contains(needle)))
        .unwrap_or_else(|| paths.len().saturating_sub(1));
    // If body_from didn't match (start stayed at 0 by fall-through), preserve
    // the original list — the operator passed a bound that didn't resolve.
    if body_from.is_some()
        && !paths
            .iter()
            .any(|p| p.contains(body_from.unwrap_or_default()))
    {
        return paths.to_vec();
    }
    if start > end {
        // Bounds inverted (body_to before body_from in manifest order) →
        // fall through to the full list so the operator notices.
        return paths.to_vec();
    }
    paths[start..=end].to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentic_core::{
        db::open_in_memory,
        project::{ProjectKind, create as create_project},
    };

    #[test]
    fn page_estimate_ceils() {
        assert_eq!(pages_from_words(0), 0);
        assert_eq!(pages_from_words(1), 1);
        assert_eq!(pages_from_words(500), 1);
        assert_eq!(pages_from_words(501), 2);
    }

    #[test]
    fn over_limit_warns() {
        let conn = open_in_memory().unwrap();
        let pid = create_project(&conn, "T", ProjectKind::Thesis, "en", None).unwrap();
        // 600 words → ≈2 pages; max 1 → over.
        let body = "word ".repeat(600);
        worktree::put_at(
            &conn,
            &pid,
            "thesis-draft-v5/ch.md",
            body.as_bytes(),
            "text/markdown",
            Some("en"),
            "u",
            "init",
        )
        .unwrap();
        let report = run(&conn, &pid, "thesis-draft-v5/", 1).unwrap();
        assert!(report.findings.iter().any(|f| f.category == "PAGE_OVER"));
    }

    #[test]
    fn under_limit_ok() {
        let conn = open_in_memory().unwrap();
        let pid = create_project(&conn, "T", ProjectKind::Thesis, "en", None).unwrap();
        worktree::put_at(
            &conn,
            &pid,
            "thesis-draft-v5/ch.md",
            b"a short chapter body",
            "text/markdown",
            Some("en"),
            "u",
            "init",
        )
        .unwrap();
        let report = run(&conn, &pid, "thesis-draft-v5/", 60).unwrap();
        assert!(report.findings.iter().any(|f| f.category == "PAGE_OK"));
        assert_eq!(report.verdict, crate::Verdict::Pass);
    }

    #[test]
    fn custom_words_per_page_changes_estimate() {
        // 600 words at 500 wpp → 2 pages (over a max-1 limit).
        // 600 words at 1000 wpp → 1 page (within a max-1 limit).
        assert_eq!(pages_from_words_wpp(600, 500), 2);
        assert_eq!(pages_from_words_wpp(600, 1000), 1);
        // wpp=0 falls back to the default rate (no division by zero).
        assert_eq!(pages_from_words_wpp(500, 0), 1);
    }

    #[test]
    fn scoped_paths_only_counts_listed_files() {
        // Mixed-prefix bookkit (the master-thesis case): the manifest pulls
        // from both `thesis/` and `out/sources/`. A prefix-only scan would
        // miss whichever side is not selected; the scoped variant sees both.
        let conn = open_in_memory().unwrap();
        let pid = create_project(&conn, "T", ProjectKind::Thesis, "en", None).unwrap();
        let body_300 = "word ".repeat(300);
        let body_400 = "word ".repeat(400);
        let body_other = "word ".repeat(1000);
        worktree::put_at(
            &conn,
            &pid,
            "thesis/fhnw_5.md",
            body_300.as_bytes(),
            "text/markdown",
            Some("en"),
            "u",
            "init",
        )
        .unwrap();
        worktree::put_at(
            &conn,
            &pid,
            "out/sources/frontmatter/acronyms.md",
            body_400.as_bytes(),
            "text/markdown",
            Some("en"),
            "u",
            "init",
        )
        .unwrap();
        worktree::put_at(
            &conn,
            &pid,
            "out/sources/some_other_book_chapter.md",
            body_other.as_bytes(),
            "text/markdown",
            Some("en"),
            "u",
            "init",
        )
        .unwrap();

        // Prefix scan over `out/sources/` includes the 1000-word stray
        // chapter that does NOT belong to the master_thesis book.
        let prefix_report = run(&conn, &pid, "out/sources/", 60).unwrap();
        let prefix_msg = &prefix_report.findings[0].message;
        assert!(
            prefix_msg.contains("1400 words"),
            "prefix scan must sum every *.md under prefix: {prefix_msg}"
        );

        // Scoped scan over only the two paths the master_thesis book lists
        // sums 300 + 400 = 700 words.
        let paths = ["thesis/fhnw_5.md", "out/sources/frontmatter/acronyms.md"];
        let scoped_report = run_scoped(
            &conn,
            &pid,
            Scope::Paths {
                book_key: "master_thesis",
                paths: &paths,
                body_from: None,
                body_to: None,
            },
            60,
            WORDS_PER_PAGE,
        )
        .unwrap();
        let scoped_msg = &scoped_report.findings[0].message;
        assert!(
            scoped_msg.contains("700 words"),
            "scoped scan must sum only listed paths: {scoped_msg}"
        );
        assert!(
            scoped_msg.contains("book 'master_thesis'"),
            "scoped scan must name the book in its message: {scoped_msg}"
        );
    }

    #[test]
    fn calibration_280wpp_flips_a_borderline_estimate() {
        // 25,000 words at the legacy 500 wpp → 50 pages (under the 60-page
        // FHNW cap). The same word count at the empirically-measured 280 wpp
        // → 90 pages, which correctly triggers PAGE_OVER.
        let conn = open_in_memory().unwrap();
        let pid = create_project(&conn, "T", ProjectKind::Thesis, "en", None).unwrap();
        let body = "word ".repeat(25_000);
        worktree::put_at(
            &conn,
            &pid,
            "thesis/fhnw.md",
            body.as_bytes(),
            "text/markdown",
            Some("en"),
            "u",
            "init",
        )
        .unwrap();
        let legacy = run_scoped(&conn, &pid, Scope::Prefix("thesis/"), 60, WORDS_PER_PAGE).unwrap();
        assert!(legacy.findings.iter().any(|f| f.category == "PAGE_OK"));
        let calibrated = run_scoped(&conn, &pid, Scope::Prefix("thesis/"), 60, 280).unwrap();
        assert!(
            calibrated
                .findings
                .iter()
                .any(|f| f.category == "PAGE_OVER")
        );
    }
}
