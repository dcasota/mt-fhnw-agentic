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
static URL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"https?://[^\s)\]<>"']+"#).unwrap());
static DIMPATH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"Dimension_(\d{2})_").unwrap());

pub fn run(db_path: &Path, action: BibAction, json_out: bool) -> Result<()> {
    let conn = agentic_core::db::open(db_path)?;
    match action {
        BibAction::Harvest { project, prefix } => harvest(&conn, &project, &prefix, json_out),
        BibAction::Emit { project, dimension } => emit(&conn, &project, dimension, json_out),
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
        println!("{}", json!({ "web_traces": appended, "user_traces": user_added }));
    } else {
        println!(
            "Harvested {appended} web/email trace(s) + {user_added} user-input trace(s) into literature_corpus (bound to HEAD)."
        );
    }
    Ok(())
}

fn emit(
    conn: &rusqlite::Connection,
    project: &str,
    dimension: Option<i64>,
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
            println!("\n## Dimension {d} — References\n\n(no references)\n");
            continue;
        };
        let mut lines: Vec<String> = refs.iter().map(|(l, _)| l.clone()).collect();
        lines.sort_by_key(|l| l.to_lowercase());
        println!("\n## Dimension {d} — References ({} entries)\n", lines.len());
        for l in lines {
            println!("- {l}");
        }
    }
    Ok(())
}
