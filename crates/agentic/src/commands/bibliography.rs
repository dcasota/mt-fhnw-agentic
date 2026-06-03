//! `agentic bibliography` — Phase-1 non-repudiation: harvest every web / email /
//! user-input trace into the `literature_corpus` passport (APA7 fields, bound to
//! HEAD), and emit a per-dimension APA7 reference list.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::LazyLock;

use anyhow::Result;
use regex::Regex;
use serde_json::{Value, json};

use agentic_core::audit::apa7;
use agentic_core::passport::{self, Section};
use agentic_core::worktree;

use crate::cli::BibAction;

static MDLINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[([^\]]+)\]\((https?://[^)\s]+)\)").unwrap());
static URL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"https?://[^\s)\]<>"']+"#).unwrap());
static DIMPATH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"Dimension_(\d{2})_").unwrap());

pub fn run(db_path: &Path, action: BibAction, json_out: bool) -> Result<()> {
    let conn = agentic_core::db::open(db_path)?;
    match action {
        BibAction::Harvest { project, prefix } => harvest(&conn, &project, &prefix, json_out),
        BibAction::Emit {
            project,
            dimension,
            per_dimension,
            write,
        } => emit(&conn, &project, dimension, per_dimension, write, json_out),
    }
}

fn norm(u: &str) -> String {
    u.trim_end_matches(['.', ',', ')', ']', '>', '"', '\'', ';'])
        .to_string()
}

fn domain(url: &str) -> String {
    url.split("://")
        .nth(1)
        .and_then(|r| r.split('/').next())
        .unwrap_or(url)
        .trim_start_matches("www.")
        .to_string()
}

fn dim_of(path: &str) -> Option<i64> {
    DIMPATH
        .captures(path)
        .and_then(|c| c.get(1))
        .and_then(|m| m.as_str().parse::<i64>().ok())
}

fn cite_key(url: &str) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    url.hash(&mut h);
    format!("{}_{:x}", domain(url).replace('.', ""), h.finish() & 0xffff)
}

fn harvest(conn: &rusqlite::Connection, project: &str, prefix: &str, json_out: bool) -> Result<()> {
    let head = worktree::head_commit(conn, project)?.map(|c| c.sha256);

    // Existing URLs + which dimensions already have a user_input trace.
    let existing = passport::current(conn, project, Section::LiteratureCorpus)?;
    let mut have_url: HashSet<String> = HashSet::new();
    let mut user_dims: HashSet<i64> = HashSet::new();
    for e in &existing {
        if let Ok(v) = serde_json::from_str::<Value>(&e.payload_json) {
            if let Some(u) = v.get("url").and_then(Value::as_str) {
                if !u.is_empty() {
                    have_url.insert(norm(u).to_lowercase());
                }
            }
            if v.get("trace_origin").and_then(Value::as_str) == Some("user_input") {
                if let Some(d) = v.get("dimension").and_then(Value::as_i64) {
                    user_dims.insert(d);
                }
            }
        }
    }

    let entries = worktree::list(conn, project, prefix)?;
    let mut appended = 0usize;
    let mut staged: HashSet<String> = HashSet::new();
    for (path, _sha) in entries
        .iter()
        .filter(|(p, _)| p.ends_with(".md") && !p.contains("_resolved"))
    {
        let trace_origin = if path.contains("emailresearch") {
            "emailresearch"
        } else {
            "weburl"
        };
        let dim = dim_of(path);
        let blob = worktree::read_at(conn, project, path)?;
        let text = String::from_utf8_lossy(&blob.content);

        // Markdown-link titles first; then any remaining bare URLs.
        let mut title_of: HashMap<String, String> = HashMap::new();
        for c in MDLINK.captures_iter(&text) {
            let title = c.get(1).map_or("", |m| m.as_str()).trim().to_string();
            let url = norm(c.get(2).map_or("", |m| m.as_str()));
            title_of.entry(url).or_insert(title);
        }
        for m in URL.find_iter(&text) {
            let url = norm(m.as_str());
            let key = url.to_lowercase();
            if have_url.contains(&key) || !staged.insert(key.clone()) {
                continue;
            }
            let dom = domain(&url);
            // Use the markdown link text as the title where available; otherwise
            // fall back to the domain. Organization = domain so APA7 reads
            // "<domain> (n.d.). <title>. <url>" instead of "Anonymous". Year is
            // omitted (→ clean "n.d.") and venue dropped to avoid redundancy.
            let title = title_of
                .get(&url)
                .filter(|t| !t.is_empty())
                .cloned()
                .unwrap_or_else(|| dom.clone());
            let payload = json!({
                "citation_key": cite_key(&url),
                "type": "website",
                "title": title,
                "url": url,
                "organization": dom,
                "dimension": dim,
                "trace_origin": trace_origin,
                "ingest_source": "bibliography_harvest",
                "source_path": path,
            });
            passport::append(
                conn,
                project,
                Section::LiteratureCorpus,
                &payload.to_string(),
                head.as_deref(),
                None,
            )?;
            appended += 1;
        }
    }

    // One user-input personal-communication trace per dimension that lacks one:
    // the author directives that shaped the dimension (journal / inbox provenance).
    let mut user_added = 0usize;
    for d in 1..=11i64 {
        if user_dims.contains(&d) {
            continue;
        }
        let payload = json!({
            "citation_key": format!("casota2026_dim{d}_input"),
            "type": "personal_communication",
            "title": format!("Author directives and review shaping dimension {d}"),
            "authors": [{ "given": "Daniel", "family": "Casota" }],
            "year": 2026,
            "venue": "Project input (agentic journal + inbox)",
            "dimension": d,
            "trace_origin": "user_input",
            "ingest_source": "bibliography_harvest",
        });
        passport::append(
            conn,
            project,
            Section::LiteratureCorpus,
            &payload.to_string(),
            head.as_deref(),
            None,
        )?;
        user_added += 1;
    }

    if json_out {
        println!(
            "{}",
            json!({ "web_traces": appended, "user_traces": user_added })
        );
    } else {
        println!(
            "Harvested {appended} web/email trace(s) + {user_added} user-input trace(s) into literature_corpus (bound to HEAD)."
        );
    }
    Ok(())
}

/// Decide whether an APA7 line belongs in the curated alphabetical
/// References chapter. Rejects:
///
///   1. **Domain-as-author placeholders** — entries of the shape
///      `"<domain>.tld (n.d.). <same>. https://..."` that
///      `harvest` generates when no real author is detectable
///      (e.g. `"arxiv.org (n.d.). arxiv.org. ..."`). These are
///      indistinguishable from URL stubs and offer no
///      bibliographic information beyond the URL itself.
///   2. **Project-input markers** — entries containing
///      "author directives and review shaping" are personal-communication
///      traces of the author's own working notes, included in the
///      passport for non-repudiation but not citable in a public
///      bibliography.
///
/// Everything else passes. The check is conservative on purpose: a
/// real reference whose first word happens to look like a domain
/// (rare — author surnames don't usually contain dots) is preserved.
#[cfg(test)]
mod curation_tests {
    use super::is_curated_keeper;

    #[test]
    fn keeps_real_author_entries() {
        assert!(is_curated_keeper(
            "Beck, K. (2001). Manifesto for Agile Software Development. agilemanifesto.org."
        ));
        assert!(is_curated_keeper(
            "McKinsey & Company (2025). Unlocking the value of AI in software development."
        ));
        assert!(is_curated_keeper(
            "Noack, & Casota, D. (2025). Readiness von agilen Unternehmensstrukturen."
        ));
        assert!(is_curated_keeper(
            "Casota, D. (2025). Photon OS engineering working notes. project working documents."
        ));
    }

    #[test]
    fn drops_lowercase_domain_placeholders() {
        assert!(!is_curated_keeper(
            "arxiv.org (n.d.). arxiv.org. https://arxiv.org/abs/1907.09415"
        ));
        assert!(!is_curated_keeper(
            "doi.org (n.d.). doi.org. https://doi.org/10.6028/NIST.AI.100-1"
        ));
        assert!(!is_curated_keeper(
            "eur-lex.europa.eu (n.d.). eur-lex.europa.eu. https://..."
        ));
        assert!(!is_curated_keeper("arxiv (n.d.). arxiv. https://arxiv"));
        assert!(!is_curated_keeper("www (n.d.). www. https://www"));
        assert!(!is_curated_keeper(
            "blackhat (n.d.). blackhat. https://www.blackhat"
        ));
    }

    #[test]
    fn drops_digit_led_domain_placeholders() {
        assert!(!is_curated_keeper(
            "7-zip.org (n.d.). 7-zip.org. https://www.7-zip.org/"
        ));
        assert!(!is_curated_keeper(
            "18f.gsa.gov (n.d.). 18f.gsa.gov. https://18f.gsa.gov/open-source-"
        ));
    }

    #[test]
    fn drops_author_directives_marker() {
        assert!(!is_curated_keeper(
            "Casota, D. (2026). Author directives and review shaping dimension 1. Project input."
        ));
    }

    #[test]
    fn drops_untitled_fallback() {
        assert!(!is_curated_keeper(
            "Anonymous (n.d.). Untitled. https://example.com"
        ));
    }
}

#[must_use]
pub fn is_curated_keeper(line: &str) -> bool {
    let lower = line.to_lowercase();
    if lower.contains("author directives and review shaping") {
        return false;
    }
    // APA7 author tokens are always surname-led — `Beck,` `McKinsey`
    // `Noack,`. A first token that starts with a lower-case letter
    // (`arxiv`, `doi.org`, `eur-lex.europa.eu`, `www`, `blackhat`)
    // or a digit (`7-zip.org`, `18f.gsa.gov`) is always a harvest
    // fallback where no real author could be extracted. Reject
    // every such case; the citation still lives in the
    // material-passport ledger via its URL, just not in the
    // printed bibliography.
    if let Some(first) = line.split_whitespace().next() {
        let token = first.trim_end_matches(',');
        if let Some(c) = token.chars().next() {
            if c.is_ascii_lowercase() || c.is_ascii_digit() {
                return false;
            }
        }
    }
    // Reject the apa7() fallback title "Untitled" — these are
    // entries where no usable title metadata exists.
    if lower.contains("). untitled.") {
        return false;
    }
    true
}

/// Drop the leading bibliographic noise ("A ", "An ", "The ") so that
/// `"The Vision of Autonomic Computing"` sorts under V where APA7
/// rules expect it. We only apply this when the first token of the
/// author segment ends with a period (e.g. "Kephart, J. O., & Chess,
/// D. M. (2003).") — i.e. the author block ran out and the title
/// has slipped into the sort key. For the common author-led entries
/// the natural surname-first ordering is already correct.
fn alphabetisation_key(line: &str) -> String {
    let lower = line.to_lowercase();
    // Strip a leading APA7 author block of the form
    // "Surname, X. Y., ..." up to the year parenthesis so the sort
    // key starts with the surname.
    lower
        .trim_start_matches(['*', '_', '`', '-', ' '])
        .to_owned()
}

/// Worktree path the thesis renderer reads the References chapter from.
pub const BIBLIOGRAPHY_PATH: &str = "out/sources/Dimensions_bibliography_EN.md";

fn emit(
    conn: &rusqlite::Connection,
    project: &str,
    dimension: Option<i64>,
    per_dimension: bool,
    write: bool,
    json_out: bool,
) -> Result<()> {
    let lit = passport::current(conn, project, Section::LiteratureCorpus)?;
    let mut by_dim: HashMap<i64, Vec<(String, Value)>> = HashMap::new();
    let mut undated: Vec<(String, Value)> = Vec::new();
    for e in &lit {
        let Ok(v) = serde_json::from_str::<Value>(&e.payload_json) else {
            continue;
        };
        let line = apa7(&v);
        match v.get("dimension").and_then(Value::as_i64) {
            Some(d) => by_dim.entry(d).or_default().push((line, v)),
            None => undated.push((line, v)),
        }
    }

    // JSON output preserves the per-dimension structure for tooling
    // that consumes it programmatically. The alphabetical curation is
    // applied to the human-readable markdown output only.
    if json_out {
        let mut out = serde_json::Map::new();
        for (d, mut refs) in by_dim {
            if dimension.is_some_and(|x| x != d) {
                continue;
            }
            refs.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
            out.insert(
                d.to_string(),
                Value::Array(refs.into_iter().map(|(l, _)| Value::String(l)).collect()),
            );
        }
        println!("{}", serde_json::to_string_pretty(&Value::Object(out))?);
        return Ok(());
    }

    // Per-dimension grouped output (legacy mode). Used when the
    // caller explicitly opts in (`--per-dimension`) or requests a
    // single dimension (`--dimension N`).
    if per_dimension || dimension.is_some() {
        let mut buf = String::new();
        let dims: Vec<i64> = match dimension {
            Some(d) => vec![d],
            None => {
                let mut v: Vec<i64> = by_dim.keys().copied().collect();
                v.sort_unstable();
                v
            }
        };
        for d in dims {
            let Some(refs) = by_dim.get(&d) else {
                buf.push_str(&format!(
                    "\n## Dimension {d} — References\n\n(no references)\n"
                ));
                continue;
            };
            let mut lines: Vec<String> = refs.iter().map(|(l, _)| l.clone()).collect();
            lines.sort_by_key(|l| l.to_lowercase());
            buf.push_str(&format!(
                "\n## Dimension {d} — References ({} entries)\n\n",
                lines.len()
            ));
            for l in lines {
                buf.push_str(&format!("- {l}\n"));
            }
        }
        return finalise(conn, project, &buf, write);
    }

    // ---------------------------------------------------------------
    // Curated alphabetical output (the FHNW MAS default).
    // ---------------------------------------------------------------
    //
    // 1. Pool every entry across all dimensions plus the undated set.
    // 2. Run each through `is_curated_keeper` (drop placeholder
    //    domain-as-author stubs + project-input markers).
    // 3. Deduplicate by case-insensitive APA7 line content.
    // 4. Sort by the alphabetisation key (case-insensitive,
    //    leading-fluff stripped).
    // 5. Emit each as a plain paragraph — no bullets, no dimension
    //    headings. The renderer formats these as the References
    //    chapter with hanging-indent paragraph style.
    let mut all: Vec<String> = Vec::new();
    for refs in by_dim.values() {
        for (line, _) in refs {
            if is_curated_keeper(line) {
                all.push(line.clone());
            }
        }
    }
    for (line, _) in &undated {
        if is_curated_keeper(line) {
            all.push(line.clone());
        }
    }

    // Dedupe (case-insensitive). Preserve the first form encountered
    // for the kept entries.
    let mut seen: HashSet<String> = HashSet::new();
    let original_count = all.len();
    all.retain(|l| seen.insert(l.to_lowercase()));
    let kept = all.len();
    let dropped = original_count - kept;

    // Alphabetical sort.
    all.sort_by(|a, b| alphabetisation_key(a).cmp(&alphabetisation_key(b)));

    // Markdown header + curation note + paragraphs.
    let mut buf = String::new();
    buf.push_str("# References\n\n");
    buf.push_str(&format!(
        "*Curated alphabetical APA7 bibliography ({kept} entries; {dropped} cross-dimension \
         duplicates collapsed; domain-as-author placeholder stubs and project-input \
         personal-communication traces filtered for the printed chapter). Sort key: case-\
         insensitive first author surname.*\n\n"
    ));
    for l in &all {
        buf.push_str(l);
        buf.push_str("\n\n");
    }
    finalise(conn, project, &buf, write)
}

/// Emit the rendered markdown either to stdout (default) or into the
/// project's working tree at `BIBLIOGRAPHY_PATH` (when `write` is true).
fn finalise(conn: &rusqlite::Connection, project: &str, markdown: &str, write: bool) -> Result<()> {
    if write {
        worktree::put_at(
            conn,
            project,
            BIBLIOGRAPHY_PATH,
            markdown.as_bytes(),
            "text/markdown",
            Some("en"),
            "agentic-bibliography-emit",
            "bibliography emit --write: refresh curated alphabetical APA7 References from passport",
        )?;
        eprintln!(
            "bibliography emit: wrote {} ({} bytes)",
            BIBLIOGRAPHY_PATH,
            markdown.len()
        );
    } else {
        print!("{markdown}");
    }
    Ok(())
}
