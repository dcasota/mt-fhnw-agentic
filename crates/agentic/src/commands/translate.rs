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
//!
//! Execution engine (v1, ADR-0062 shakedown surface, 2026-06-07):
//! per-path LLM call loop with a strict system prompt that preserves
//! ``` figspec / code / table fences EXACTLY as-is; writes the translated
//! markdown to a parallel-path blob whose naming follows the existing
//! `_<UPPER>.md` suffix convention (`Dimension_05_..._EN.md` →
//! `Dimension_05_..._DE.md`) or `<base>.<lower>.md` for files without the
//! suffix (`thesis/fhnw_2_theory.md` → `thesis/fhnw_2_theory.de.md`).

use std::path::Path;

use agentic_providers::ProviderKind;
use agentic_providers::registry;
use agentic_providers::traits::{ChatMessage, ChatRequest, Role};
use anyhow::{Context, Result, anyhow};

use crate::cli::TranslateAction;

/// Provider-name → `ProviderKind` (mirrors the local helper in `review.rs`;
/// kept inline so the translate command has no cross-module dep on review).
fn parse_kind(s: &str) -> Option<ProviderKind> {
    Some(match s.to_lowercase().as_str() {
        "anthropic" => ProviderKind::Anthropic,
        "openai" => ProviderKind::OpenAi,
        "google" => ProviderKind::Google,
        "mistral" => ProviderKind::Mistral,
        "cohere" => ProviderKind::Cohere,
        "grok" | "xai" => ProviderKind::Grok,
        "ollama" => ProviderKind::Ollama,
        _ => return None,
    })
}

const ALLOWED_TARGETS: &[&str] = &["de", "fr", "it", "rm", "hi"];

/// Cloud providers eligible to serve the translation task. Mirrors the
/// review-command preference list — Anthropic / Google / Grok — picked in
/// order of `registry::has_key`. The Ollama local provider is intentionally
/// not in this list (translation cost / quality is poor on small local
/// models for the prose register the thesis uses).
const CLOUD_TRANSLATE: &[ProviderKind] = &[
    ProviderKind::Anthropic,
    ProviderKind::Google,
    ProviderKind::Grok,
    ProviderKind::OpenAi,
];

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
        // Skip files whose name already encodes a non-EN language so we
        // don't accidentally re-translate (e.g. `*_DE.md`, `*.de.md`).
        if !path_is_en_source(path) {
            continue;
        }
        if scope.prefixes().iter().any(|p| path.starts_with(*p)) {
            out.push(path.clone());
        }
    }
    out.sort();
    Ok(out)
}

/// Returns true when `path` looks like an English-source markdown (i.e.
/// the natural source for translation). Filters out paths whose stem
/// already carries a non-EN language tag — either the upper-case suffix
/// `_DE.md` / `_FR.md` etc. or the lower-case dotted `.de.md` /
/// `.fr.md` etc. — so a corpus rescan never feeds a translation back
/// into the engine as its own source.
#[must_use]
fn path_is_en_source(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    for tag in ALLOWED_TARGETS {
        let upper_tag = format!("_{}.md", tag.to_ascii_uppercase());
        let upper_tag_lc = upper_tag.to_ascii_lowercase();
        let dotted = format!(".{tag}.md");
        if lower.ends_with(&upper_tag_lc) || lower.ends_with(&dotted) {
            return false;
        }
    }
    true
}

/// Resolve `src` to the parallel-path blob for `target` language.
/// Convention: a trailing `_EN.md` suffix (case-insensitive) is replaced
/// with `_<UPPER>.md` so the renderer can pick the right file by language
/// tag; any other markdown gets `.<lower>` inserted before the `.md`
/// extension. Both conventions exist in the corpus today.
#[must_use]
fn target_path(src: &str, target: &str) -> String {
    let upper = target.to_ascii_uppercase();
    let lower = target.to_ascii_lowercase();
    if let Some(stem) = src.strip_suffix("_EN.md") {
        return format!("{stem}_{upper}.md");
    }
    if let Some(stem) = src.strip_suffix("_en.md") {
        return format!("{stem}_{upper}.md");
    }
    if let Some(stem) = src.strip_suffix(".md") {
        return format!("{stem}.{lower}.md");
    }
    format!("{src}.{lower}")
}

/// ISO target → human-readable language label for the LLM system prompt.
#[must_use]
fn language_label(target: &str) -> &'static str {
    match target {
        "de" => "German (Deutsch)",
        "fr" => "French (français)",
        "it" => "Italian (italiano)",
        "rm" => "Rumantsch Grischun (the standardised Romansh of Switzerland)",
        "hi" => "Hindi (हिन्दी, written in Devanagari)",
        _ => "the target language",
    }
}

/// Build the system prompt for one translation call. Stresses the
/// non-negotiable invariants: keep code / figspec / table fences VERBATIM,
/// keep filenames and identifier strings untranslated, target-language
/// only output, no preamble.
#[must_use]
fn system_prompt(target: &str) -> String {
    let label = language_label(target);
    format!(
        "You are a precise technical translator working on a master-thesis manuscript.\n\
         Translate the user's markdown content from English to {label}.\n\
         \n\
         Strict invariants — non-negotiable:\n\
         1. Preserve every ``` code / figspec / table fence and its inner JSON / code EXACTLY \
         AS-IS. Do not translate JSON keys, JSON string values, filenames, file paths, \
         identifiers, ADR IDs (ADR-XXXX), CAR IDs (CAR-XX-XXX), FRD IDs (FRD-CXX), CWE numbers, \
         CVE numbers, model names (ML-DSA-87, ML-KEM-512), or URLs.\n\
         2. Preserve every markdown structural element: headings (#, ##, ###), lists (-, *, +, \
         1.), tables (| col |), block-quotes (>), links ([text](url)), images, footnotes, and \
         emphasis (*italic*, **bold**).\n\
         3. Preserve every figspec JSON value EXACTLY — including the `caption`, `title`, and \
         every label / node / edge text inside `data`. Figure translations are handled \
         separately under ADR-0062 Phase A; for now the figspec block is opaque.\n\
         4. Output ONLY the translated markdown. No preamble, no postscript, no \"Here is the \
         translation:\" lead-in, no fenced wrapping.\n\
         5. Keep the same paragraph count and the same heading hierarchy.\n\
         6. The output must be SOLELY in {label}. Mixing English fragments into the body \
         (other than the preserved-verbatim items in invariants 1 + 3) is a hard error.\n\
         \n\
         If you are uncertain about a term, default to the English form unchanged rather than \
         inventing a translation; the per-iteration fidelity gate will surface it for review."
    )
}

/// Send the prompt to the configured provider and return the translated
/// content. Bounds: the source markdown is sent in full (no chunking yet);
/// the model is responsible for the same-paragraph-count invariant.
async fn translate_one(
    prov: &dyn agentic_providers::traits::Provider,
    model: &str,
    target: &str,
    source_md: &str,
) -> Result<String> {
    let req = ChatRequest {
        model: model.to_string(),
        messages: vec![ChatMessage {
            role: Role::User,
            content: source_md.to_string(),
        }],
        temperature: Some(0.0),
        max_tokens: Some(8192),
        seed: None,
        system: Some(system_prompt(target)),
    };
    let resp = prov
        .chat(&req)
        .await
        .map_err(|e| anyhow!("provider chat failed: {e}"))?;
    let trimmed = resp.content.trim().to_string();
    if trimmed.is_empty() {
        anyhow::bail!("provider returned empty translation");
    }
    Ok(trimmed)
}

pub async fn run(db_path: &Path, action: TranslateAction, json_out: bool) -> Result<()> {
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

    // Pick a configured cloud provider (explicit or first available in the
    // translate-preferred cloud list).
    let kind = match &provider {
        Some(p) => parse_kind(p).ok_or_else(|| anyhow!("unknown provider '{p}'"))?,
        None => CLOUD_TRANSLATE
            .iter()
            .copied()
            .find(|k| registry::has_key(*k))
            .ok_or_else(|| {
                anyhow!(
                    "no chat provider configured (set e.g. AGENTIC_ANTHROPIC_KEY, \
                     ANTHROPIC_API_KEY, AGENTIC_GOOGLE_KEY, or GOOGLE_API_KEY)"
                )
            })?,
    };
    let model = match kind {
        ProviderKind::Anthropic => "claude-opus-4-7".to_string(),
        ProviderKind::Google => "gemini-2.5-pro".to_string(),
        ProviderKind::Grok => "grok-4.3".to_string(),
        ProviderKind::OpenAi => "gpt-4o".to_string(),
        other => format!("{other:?}").to_lowercase(),
    };
    let prov = registry::build(kind).map_err(|e| anyhow!("provider build failed: {e}"))?;

    println!(
        "translate target={target} scope={scope} provider={kind:?} model={model} \
         paths={} — beginning per-path LLM-translate-and-write loop (ADR-0062 v1).",
        paths.len()
    );

    let mut written = 0usize;
    let mut skipped_empty = 0usize;
    let mut failed: Vec<(String, String)> = Vec::new();

    for src in &paths {
        let blob = match agentic_core::worktree::read_at(&conn, &project, src) {
            Ok(b) => b,
            Err(e) => {
                failed.push((src.clone(), format!("read source blob: {e}")));
                continue;
            }
        };
        let source_md = String::from_utf8_lossy(&blob.content).to_string();
        if source_md.trim().is_empty() {
            skipped_empty += 1;
            println!("  · {src} → empty source, skipped");
            continue;
        }
        let target_path_str = target_path(src, &target);
        println!("  → translating {src}  →  {target_path_str}");
        match translate_one(prov.as_ref(), &model, &target, &source_md).await {
            Ok(translated) => {
                // Stage the translation as a parallel-path blob via
                // `worktree::put_at` — one commit per write, captured by
                // the project's default branch the same way every other
                // content writer in the runtime works. The commit message
                // names the source path + target language so the audit
                // trail tells the story without the operator opening the
                // diff.
                let bytes = translated.as_bytes();
                let commit_msg = format!(
                    "translate: {} → {target_path_str} (target={target}, model={model})",
                    src
                );
                match agentic_core::worktree::put_at(
                    &conn,
                    &project,
                    &target_path_str,
                    bytes,
                    "text/markdown",
                    Some(&target),
                    "agentic-translate",
                    &commit_msg,
                ) {
                    Ok(sha) => {
                        written += 1;
                        println!(
                            "    ✓ {} bytes written to {target_path_str} (commit {})",
                            translated.len(),
                            &sha[..12.min(sha.len())]
                        );
                    }
                    Err(e) => {
                        failed.push((src.clone(), format!("worktree::put_at: {e}")));
                    }
                }
            }
            Err(e) => {
                failed.push((src.clone(), format!("translate: {e}")));
                println!("    ✗ {src}: {e}");
            }
        }
    }

    if json_out {
        println!(
            "{}",
            serde_json::json!({
                "target": target,
                "scope": scope_enum.slug(),
                "provider": format!("{kind:?}"),
                "model": model,
                "written": written,
                "skipped_empty": skipped_empty,
                "failed": failed,
            })
        );
    } else {
        println!(
            "\ntranslate target={target} scope={scope} DONE: written={written}, \
             skipped_empty={skipped_empty}, failed={}.",
            failed.len()
        );
        if !failed.is_empty() {
            println!("Failures:");
            for (p, why) in &failed {
                println!("  · {p}: {why}");
            }
        }
    }
    if !failed.is_empty() {
        anyhow::bail!(
            "{} of {} paths failed to translate; see per-path failure log above",
            failed.len(),
            paths.len()
        );
    }
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

    #[test]
    fn target_path_replaces_en_suffix() {
        assert_eq!(
            target_path("out/sources/Dimension_05_operating_systems_EN.md", "de"),
            "out/sources/Dimension_05_operating_systems_DE.md"
        );
        assert_eq!(
            target_path("out/sources/Campaign_01_cve_self_patch_EN.md", "fr"),
            "out/sources/Campaign_01_cve_self_patch_FR.md"
        );
    }

    #[test]
    fn target_path_dots_for_non_en_suffix() {
        // Non-suffix markdown gets the dotted lowercase tag.
        assert_eq!(
            target_path("thesis/fhnw_2_theory.md", "de"),
            "thesis/fhnw_2_theory.de.md"
        );
        assert_eq!(
            target_path("specs/adr/0035-en-core.md", "hi"),
            "specs/adr/0035-en-core.hi.md"
        );
    }

    #[test]
    fn target_path_lower_case_en_suffix_handled() {
        assert_eq!(
            target_path("thesis/some_chapter_en.md", "rm"),
            "thesis/some_chapter_RM.md"
        );
    }

    #[test]
    fn path_is_en_filters_out_existing_translations() {
        // EN sources keep going.
        assert!(path_is_en_source(
            "out/sources/Dimension_05_operating_systems_EN.md"
        ));
        assert!(path_is_en_source("thesis/fhnw_2_theory.md"));
        // Existing translations are filtered out so a rescan doesn't feed
        // a translated blob back into the engine as its own source.
        assert!(!path_is_en_source(
            "out/sources/Dimension_05_operating_systems_DE.md"
        ));
        assert!(!path_is_en_source(
            "out/sources/Dimension_05_operating_systems_HI.md"
        ));
        assert!(!path_is_en_source("thesis/fhnw_2_theory.de.md"));
        assert!(!path_is_en_source("thesis/fhnw_2_theory.fr.md"));
    }

    #[test]
    fn language_label_renders_human_readable_target_names() {
        // Cosmetic, but the prompt text relies on these — keep the
        // mappings visible in tests so a future rename surfaces here.
        assert_eq!(language_label("de"), "German (Deutsch)");
        assert_eq!(language_label("fr"), "French (français)");
        assert_eq!(language_label("it"), "Italian (italiano)");
        assert_eq!(
            language_label("rm"),
            "Rumantsch Grischun (the standardised Romansh of Switzerland)"
        );
        assert_eq!(language_label("hi"), "Hindi (हिन्दी, written in Devanagari)");
    }

    #[test]
    fn system_prompt_mentions_target_language_label_and_invariants() {
        let p = system_prompt("de");
        assert!(p.contains("German (Deutsch)"));
        assert!(p.contains("EXACTLY AS-IS")); // figspec / code invariant
        assert!(p.contains("ADR-XXXX")); // identifier invariant
        assert!(p.contains("SOLELY in")); // purity invariant
    }
}
