//! `agentic translate` — perception-improvement P-5.
//!
//! Content translation pipeline. The runtime's chrome-only `--lang` flag
//! already exists on every command; this verb opens the held *content*
//! translation pipeline for an explicit, operator-chosen scope.
//!
//! GATED: per ADR-0047 R7 and persistent memory `hold-translation-pipeline`,
//! translation calls out to an LLM provider and writes new content blobs.
//! Both are irreversible: the LLM call costs money, and the new blobs enter
//! the audit trail. The command therefore refuses without a prior
//! `agentic authorize grant --action translate` record (the same discipline
//! the runtime applies to push/tag/publish/supersede/content_delete).
//!
//! `--dry-run` is the read-only escape: it previews the scope and the
//! provider that would be used without calling any LLM and without writing
//! any blobs. It does NOT require an authorisation.

use std::path::Path;

use anyhow::{Context, Result, anyhow};

use crate::cli::TranslateAction;

const ALLOWED_TARGETS: &[&str] = &["de", "fr", "it", "rm", "hi"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    FrontMatter,
    Thesis,
    Corpus,
}

impl Scope {
    fn parse(s: &str) -> Result<Self> {
        match s {
            "front-matter" | "frontmatter" | "fm" => Ok(Scope::FrontMatter),
            "thesis" => Ok(Scope::Thesis),
            "corpus" => Ok(Scope::Corpus),
            other => {
                anyhow::bail!("unknown scope '{other}'; valid: front-matter | thesis | corpus")
            }
        }
    }

    fn prefixes(self) -> &'static [&'static str] {
        match self {
            Scope::FrontMatter => &["out/sources/frontmatter/", "thesis/fhnw_00_"],
            Scope::Thesis => &["thesis/"],
            Scope::Corpus => &["thesis/", "out/sources/"],
        }
    }

    fn slug(self) -> &'static str {
        match self {
            Scope::FrontMatter => "front-matter",
            Scope::Thesis => "thesis",
            Scope::Corpus => "corpus",
        }
    }
}

/// Enumerate the paths a scope covers in the current head.
fn enumerate_scope(
    conn: &rusqlite::Connection,
    project: &str,
    scope: Scope,
) -> Result<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    let all = agentic_core::worktree::list(conn, project, "")?;
    for (path, _) in &all {
        if !path.ends_with(".md") {
            continue;
        }
        if scope.prefixes().iter().any(|p| path.starts_with(*p)) {
            out.push(path.clone());
        }
    }
    out.sort();
    Ok(out)
}

pub fn run(db_path: &Path, action: TranslateAction, json_out: bool) -> Result<()> {
    let TranslateAction::Scope {
        project,
        target,
        scope,
        provider,
        dry_run,
    } = action;
    if !ALLOWED_TARGETS.contains(&target.as_str()) {
        anyhow::bail!(
            "target '{target}' is not a supported language; valid: {}",
            ALLOWED_TARGETS.join(", ")
        );
    }
    let scope_enum = Scope::parse(&scope)?;
    let conn = agentic_core::db::open(db_path).context("open db")?;
    let paths = enumerate_scope(&conn, &project, scope_enum)?;
    let provider_label = provider.as_deref().unwrap_or("(first configured cloud)");

    if dry_run {
        if json_out {
            println!(
                "{}",
                serde_json::json!({
                    "dry_run": true,
                    "target": target,
                    "scope": scope_enum.slug(),
                    "provider": provider_label,
                    "paths": paths,
                    "path_count": paths.len(),
                    "would_authorise": false,
                })
            );
        } else {
            println!(
                "[dry-run] target={target} scope={} provider={}",
                scope_enum.slug(),
                provider_label
            );
            println!("[dry-run] {} path(s) in scope:", paths.len());
            for p in paths.iter().take(20) {
                println!("    {p}");
            }
            if paths.len() > 20 {
                println!("    … ({} more)", paths.len() - 20);
            }
            println!(
                "[dry-run] No LLM calls made, no blobs written. To run for real, first \
                 `agentic authorize grant --project {project} --action translate --rationale '<why>'`."
            );
        }
        return Ok(());
    }

    // Real run: refuse without an authorisation grant (ADR-0047 R7).
    agentic_core::authz::require(&conn, &project, "translate", &scope).map_err(|e| {
        anyhow!(
            "translation refused — no authorisation grant. Run: \
             agentic authorize grant --project {project} --action translate --rationale '<why>'. \
             ({e})"
        )
    })?;

    // The translation execution itself is intentionally NOT implemented yet
    // (perception P-5 is staged behind the authorise-then-execute discipline).
    // The next iteration adds the per-chapter LLM call loop, the
    // chunk-aware translator (preserving figspec / table / heading
    // structure), and the parallel-path blob writes
    // (e.g. `out/sources/Dimension_05_..._DE.md`).
    println!(
        "translate target={target} scope={scope} ({} path(s)) AUTHORISED — but the execution \
         engine is staged for the next iteration. The authorise-then-execute discipline is in \
         place; the LLM-translate-and-write loop ships next.",
        paths.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_parses_aliases() {
        assert_eq!(Scope::parse("front-matter").unwrap(), Scope::FrontMatter);
        assert_eq!(Scope::parse("frontmatter").unwrap(), Scope::FrontMatter);
        assert_eq!(Scope::parse("fm").unwrap(), Scope::FrontMatter);
        assert_eq!(Scope::parse("thesis").unwrap(), Scope::Thesis);
        assert_eq!(Scope::parse("corpus").unwrap(), Scope::Corpus);
        assert!(Scope::parse("bogus").is_err());
    }

    #[test]
    fn allowed_targets_match_chrome_languages() {
        for t in ALLOWED_TARGETS {
            // mirrors the chrome i18n set (en intentionally excluded — there
            // is nothing to translate FROM english to english).
            assert!(matches!(*t, "de" | "fr" | "it" | "rm" | "hi"));
        }
    }

    #[test]
    fn scope_prefixes_cover_expected_paths() {
        let fm = Scope::FrontMatter.prefixes();
        assert!(fm.iter().any(|p| p.contains("frontmatter")));
        assert!(fm.iter().any(|p| p.contains("fhnw_00")));

        let th = Scope::Thesis.prefixes();
        assert_eq!(th, &["thesis/"]);

        let co = Scope::Corpus.prefixes();
        assert!(co.iter().any(|p| *p == "thesis/"));
        assert!(co.iter().any(|p| *p == "out/sources/"));
    }
}
