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

use agentic_providers::traits::{ChatMessage, ChatRequest, Role};
use agentic_providers::{ProviderKind, Task, registry, router};
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

/// Allowed source-language tags. `en` is the natural source for the
/// initial fan-out; the other tags exist so ADR-0062 Phase A/C round-trip
/// chains (which translate non-EN content as their middle steps) work
/// end-to-end. `target == source` is rejected — nothing to translate.
const ALLOWED_SOURCES: &[&str] = &["en", "de", "fr", "it", "rm", "hi"];

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

#[derive(Debug, Clone, PartialEq, Eq)]
enum Scope {
    FrontMatter,
    Thesis,
    Corpus,
    /// ADR-0062 Phase A — translate exactly one figspec's user-facing text
    /// fields (caption, title, label / node / edge / cell strings inside
    /// `data`). The figspec is located by `id` across the corpus. Output
    /// is written as a sidecar JSON blob, not as a `_<LANG>.md` rewrite.
    Figure {
        id: String,
    },
}

impl Scope {
    fn parse(s: &str, figspec_id: Option<&str>) -> Result<Self> {
        match s {
            "front-matter" | "frontmatter" | "fm" => Ok(Scope::FrontMatter),
            "thesis" => Ok(Scope::Thesis),
            "corpus" => Ok(Scope::Corpus),
            "figure" => match figspec_id {
                Some(id) if !id.is_empty() => Ok(Scope::Figure { id: id.to_string() }),
                _ => {
                    anyhow::bail!("scope 'figure' requires --id <figspec_id> (e.g. --id fig08_03)")
                }
            },
            other => {
                anyhow::bail!(
                    "unknown scope '{other}'; valid: front-matter | thesis | corpus | figure"
                )
            }
        }
    }

    fn prefixes(&self) -> &'static [&'static str] {
        match self {
            Scope::FrontMatter => &["out/sources/frontmatter/", "thesis/fhnw_00_"],
            Scope::Thesis => &["thesis/"],
            Scope::Corpus => &["thesis/", "out/sources/"],
            // Figure scope walks the corpus to find the matching id.
            Scope::Figure { .. } => &["thesis/", "out/sources/"],
        }
    }

    fn slug(&self) -> String {
        match self {
            Scope::FrontMatter => "front-matter".to_string(),
            Scope::Thesis => "thesis".to_string(),
            Scope::Corpus => "corpus".to_string(),
            Scope::Figure { id } => format!("figure:{id}"),
        }
    }
}

/// Enumerate the paths a scope covers in the current head.
fn enumerate_scope(
    conn: &rusqlite::Connection,
    project: &str,
    scope: &Scope,
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

/// Find the figspec block whose `id` field equals `target_id` across the
/// corpus and return (containing-path, figspec-JSON-body). Searches every
/// `.md` source the corpus scope enumerates. Returns `None` if no figspec
/// with that id is found.
fn find_figspec_by_id(
    conn: &rusqlite::Connection,
    project: &str,
    target_id: &str,
) -> Result<Option<(String, String)>> {
    let scope = Scope::Corpus;
    for path in enumerate_scope(conn, project, &scope)? {
        let Ok(blob) = agentic_core::worktree::read_at(conn, project, &path) else {
            continue;
        };
        let md = String::from_utf8_lossy(&blob.content);
        // Walk every ```figspec block in this file; mirrors the
        // single-line-dump-safe parser the renderer uses.
        let mut rest = md.as_ref();
        const OPENER: &str = "```figspec";
        while let Some(start) = rest.find(OPENER) {
            let after = &rest[start..];
            let mut body_start = OPENER.len();
            if after[body_start..].starts_with('\n') {
                body_start += 1;
            }
            let Some(end_rel) = after[body_start..].find("```") else {
                break;
            };
            let json = &after[body_start..body_start + end_rel];
            // Best-effort id extraction — accept the figspec if its
            // `"id"` field (string-quoted) equals target_id.
            if json_id_matches(json, target_id) {
                return Ok(Some((path.clone(), json.trim().to_string())));
            }
            let consumed = body_start + end_rel + 3;
            rest = &after[consumed..];
        }
    }
    Ok(None)
}

/// Test whether a figspec JSON body's TOP-LEVEL `"id"` field equals
/// `target_id`. Only the first `"id"` occurrence is checked — figspecs
/// don't nest figspecs, and a `data.id` (e.g. when a sub-object happens
/// to use the same key) must not collide with our outer-id search.
/// Lightweight scan rather than a full parse so we survive minor
/// whitespace and key-order variation.
fn json_id_matches(json: &str, target_id: &str) -> bool {
    let Some(idx) = json.find("\"id\"") else {
        return false;
    };
    let after = &json[idx + 4..];
    let trimmed = after.trim_start_matches(|c: char| c.is_whitespace() || c == ':');
    let Some(stripped) = trimmed.strip_prefix('"') else {
        return false;
    };
    let Some(close) = stripped.find('"') else {
        return false;
    };
    &stripped[..close] == target_id
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

/// ISO language tag → human-readable label for the LLM system prompt.
/// Covers both source and target — `en` is a source-only entry for the
/// round-trip chains (e.g. step 3 of EN→DE→FR→EN comes back to English).
#[must_use]
fn language_label(tag: &str) -> &'static str {
    match tag {
        "en" => "English",
        // Note: the thesis writes Schweizerhochdeutsch (Swiss Standard
        // German), NOT German-from-Germany. See `swiss_german_addendum`
        // for the orthographic invariants the prompt enforces.
        "de" => "Swiss Standard German (Schweizerhochdeutsch)",
        "fr" => "French (français)",
        "it" => "Italian (italiano)",
        "rm" => "Rumantsch Grischun (the standardised Romansh of Switzerland)",
        "hi" => "Hindi (हिन्दी, written in Devanagari)",
        _ => "the target language",
    }
}

/// Swiss Standard German addendum — appended to every prompt when
/// `target == "de"` so the output uses Schweizerhochdeutsch orthography
/// instead of Bundesdeutsch. Two locked-in invariants:
/// (a) no `ß` — always render the eszett as `ss` (e.g. `Strasse`,
///     `grosse`, `dass`, `muss` — never `Straße`, `große`, `daß`, `muß`).
/// (b) Swiss guillemets `«…»` for direct quotation — never the
///     German typographic pair `„…"` / `„…"`.
/// The thesis is FHNW-MAS Swiss-issued and must follow Swiss orthography
/// across every render artefact.
#[must_use]
fn swiss_german_addendum(target: &str) -> &'static str {
    if target == "de" {
        "\n\n\
         Swiss Standard German (Schweizerhochdeutsch) orthography is MANDATORY \
         for `de` output:\n\
         (a) NO eszett `ß`. Always render as double-s `ss` — `Strasse` not \
         `Straße`, `grosse` not `große`, `dass` not `daß`, `muss` not `muß`, \
         `Schloss` not `Schloß`, `Mass` not `Maß`. This applies inside every \
         translated field — captions, titles, labels, node text, edge text.\n\
         (b) Direct-speech quotation marks MUST be Swiss guillemets `«…»` — \
         NOT the German typographic pair `„…\u{201C}`. Example: `«Operating \
         modes» entscheiden`, NOT `\u{201E}Operating modes\u{201C} entscheiden`.\n\
         (c) Word choice should prefer the Swiss-standard variant when one \
         exists (e.g. `Trottoir` over `Bürgersteig` where natural), but do not \
         force this if the source phrasing is technical/neutral.\n\
         These rules are non-negotiable: a `ß` or a `\u{201E}` / `\u{201C}` \
         in the output is a hard validation failure."
    } else {
        ""
    }
}

/// Build the figure-scope system prompt — used for `Scope::Figure { id }`.
/// The user-facing text fields of a figspec (caption, title, label /
/// node / edge / cell strings inside `data`) get translated; the
/// structural keys (id, type, palette, data shape) MUST be preserved
/// byte-for-byte so the renderer round-trip stays deterministic.
#[must_use]
fn figure_system_prompt(source: &str, target: &str) -> String {
    let src_label = language_label(source);
    let label = language_label(target);
    format!(
        "You are a precise technical translator working on a figure inside a master-thesis \
         manuscript. You will receive a JSON object that describes one figure (a `figspec`) \
         whose user-facing text fields are written in {src_label}. \
         Translate ONLY the user-facing text fields to {label}; preserve every structural \
         key and value byte-for-byte.\n\
         \n\
         Translate these fields (when present): `caption`, `title`, every string inside \
         `data.labels`, every string inside `data.rows`, every string inside `data.cols`, \
         every string inside `data.nodes`, every string inside `data.cells` (when the \
         cell is a free-text string rather than a status code like `OK` / `block` / `review`), \
         the third element of every entry in `data.edges` (the edge-label text), and any \
         `xlabel` / `ylabel` field.\n\
         \n\
         Do NOT translate:\n\
         1. JSON keys (`id`, `type`, `palette`, `data`, `nodes`, `edges`, etc.) — keep them \
         exactly as given.\n\
         2. The `id` value, the `type` value, the `palette` value.\n\
         3. Numeric values, booleans, hex colour codes, status codes (`OK`, `block`, `review`, \
         `L0`–`L4`, `HITL`).\n\
         4. Identifier strings: ADR-XXXX, CAR-XX-XXX, FRD-CXX, CWE-XXX, CVE-XXXX-XXXXX, \
         CVSS, NIST IR XXXX, CNSA, ISO/IEC NNNNN, ML-DSA-87, ML-KEM-512, SLH-DSA, FIPS 203, \
         filenames, file paths, URLs.\n\
         5. Markdown emphasis markers (`**`, `_`) — preserve them in-place.\n\
         \n\
         No-hallucination invariants (most important — observed shakedown bug \
         2026-06-07: an early run produced \
         `drei Betriebsmodi (normal / Eskalation / katastrophal) Betriebsmodi` for \
         the source phrase `operating modes`. That output violates ALL of A-D below):\n\
         A. Do NOT add explanatory content that has no direct correspondent in the \
         source. If the source says `operating modes`, translate that exactly to \
         `Betriebsmodi` (or `modes opératoires` / `modi operativi` / etc.) — do \
         NOT add `(normal / escalation / catastrophic)`, do NOT add `drei` / \
         `trois` / `tre`, do NOT add any tooltip-like elaboration.\n\
         B. Do NOT expand abbreviations or acronyms beyond the literal text. If the \
         source says `HITL`, write `HITL` (or its target-language equivalent acronym), \
         not `Human-In-The-Loop oversight`.\n\
         C. Do NOT repeat a word for emphasis. Each source word maps to exactly one \
         target-language word (or canonical multi-word translation), and no source \
         word may be inserted twice. Specifically, do NOT write `Betriebsmodi … \
         Betriebsmodi` — one occurrence per source `modes` only.\n\
         D. The translation must carry the SAME information content as the source — \
         neither more nor less. If you find yourself wanting to clarify a term, do \
         not; that's the editor's job, not the translator's. Word count of the target \
         caption should be within ±2 of the source caption's word count.\n\
         \n\
         Output requirements:\n\
         - Return ONLY the modified JSON object, no preamble, no postscript, no markdown \
         fences around it.\n\
         - The JSON must be a single valid object that re-parses with the same structure as \
         the input (same keys at every level, same array lengths).\n\
         - The output must be SOLELY in {label} for the translated fields; mixing English \
         fragments other than the preserved-verbatim items above is a hard error.{}",
        swiss_german_addendum(target),
    )
}

/// Build the system prompt for one translation call. Stresses the
/// non-negotiable invariants: keep code / figspec / table fences VERBATIM,
/// keep filenames and identifier strings untranslated, target-language
/// only output, no preamble.
#[must_use]
fn system_prompt(source: &str, target: &str) -> String {
    let src_label = language_label(source);
    let label = language_label(target);
    format!(
        "You are a precise technical translator working on a master-thesis manuscript.\n\
         Translate the user's markdown content from {src_label} to {label}.\n\
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
         inventing a translation; the per-iteration fidelity gate will surface it for review.{}",
        swiss_german_addendum(target),
    )
}

/// Send the prompt to the configured provider and return the translated
/// content. Bounds: the source markdown is sent in full (no chunking yet);
/// the model is responsible for the same-paragraph-count invariant.
/// Default model name per provider for the Translate task.
#[must_use]
fn translate_model_for(kind: ProviderKind) -> String {
    match kind {
        ProviderKind::Anthropic => "claude-opus-4-7".to_string(),
        ProviderKind::Google => "gemini-2.5-pro".to_string(),
        ProviderKind::Grok => "grok-4.3".to_string(),
        ProviderKind::OpenAi => "gpt-4o".to_string(),
        other => format!("{other:?}").to_lowercase(),
    }
}

/// Walk `attempt_order` building each provider and calling chat. Returns
/// the first successful (kind, model, content) tuple, or aggregates every
/// failure into a single anyhow error so the operator sees the full
/// chain (e.g. "Anthropic → 400 credit balance too low; Grok → success"
/// vs the previous "first provider failed; gave up").
///
/// `extract` post-processes the raw model output (figure scope strips
/// ```json fences and validates JSON; document scope is the identity).
async fn try_translate_chain<F>(
    attempt_order: &[ProviderKind],
    system: String,
    user: String,
    max_tokens: u32,
    extract: F,
) -> Result<(ProviderKind, String, String)>
where
    F: Fn(&str) -> Result<String>,
{
    let mut failures: Vec<(ProviderKind, String)> = Vec::new();
    for kind in attempt_order {
        let model = translate_model_for(*kind);
        let prov = match registry::build(*kind) {
            Ok(p) => p,
            Err(e) => {
                failures.push((*kind, format!("build failed: {e}")));
                continue;
            }
        };
        let req = ChatRequest {
            model: model.clone(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: user.clone(),
            }],
            temperature: Some(0.0),
            max_tokens: Some(max_tokens),
            seed: None,
            system: Some(system.clone()),
        };
        match prov.chat(&req).await {
            Ok(resp) => {
                let trimmed = resp.content.trim().to_string();
                if trimmed.is_empty() {
                    failures.push((*kind, "provider returned empty content".into()));
                    eprintln!("  ! {kind:?} returned empty content; trying next provider…");
                    continue;
                }
                match extract(&trimmed) {
                    Ok(out) => return Ok((*kind, model, out)),
                    Err(e) => {
                        failures.push((*kind, format!("post-process: {e}")));
                        eprintln!(
                            "  ! {kind:?} response failed post-processing ({e}); \
                             trying next provider…"
                        );
                        continue;
                    }
                }
            }
            Err(e) => {
                failures.push((*kind, format!("{e}")));
                eprintln!("  ! {kind:?} call failed: {e}; trying next provider…");
                continue;
            }
        }
    }
    let summary: String = failures
        .iter()
        .map(|(k, msg)| format!("{k:?}: {msg}"))
        .collect::<Vec<_>>()
        .join("; ");
    anyhow::bail!("all providers failed in the fallback chain — {summary}")
}

/// Outcome of one cache-aware translate call. `cached` is `true` when
/// the cache hit short-circuited the LLM round-trip; `false` when the
/// chain ran for real and the result was stored.
#[derive(Debug, Clone)]
struct CacheOutcome {
    cached: bool,
    provider: String,
    model: String,
    target: String,
}

/// Cache-then-chain: look the segment up in `translation_cache`; on
/// miss, run the provider fallback chain and store the result.
/// `segment_kind` is a human-readable tag stored alongside the cached
/// entry (`figspec`, `document`, `paragraph` — useful for analytics).
async fn translate_with_cache<F>(
    conn: &rusqlite::Connection,
    project: &str,
    source_lang: &str,
    target_lang: &str,
    source_text: &str,
    segment_kind: &str,
    attempt_order: &[ProviderKind],
    system: String,
    max_tokens: u32,
    extract: F,
) -> Result<CacheOutcome>
where
    F: Fn(&str) -> Result<String>,
{
    if let Some(hit) = agentic_core::translation_cache::get(
        conn,
        source_lang,
        target_lang,
        source_text,
        Some(project),
    )? {
        return Ok(CacheOutcome {
            cached: true,
            provider: hit.provider,
            model: hit.model,
            target: hit.target_text,
        });
    }
    let (used_kind, used_model, target) = try_translate_chain(
        attempt_order,
        system,
        source_text.to_string(),
        max_tokens,
        extract,
    )
    .await?;
    let provider = format!("{used_kind:?}");
    agentic_core::translation_cache::put(
        conn,
        source_lang,
        target_lang,
        source_text,
        &target,
        &provider,
        &used_model,
        Some(project),
        segment_kind,
    )?;
    Ok(CacheOutcome {
        cached: false,
        provider,
        model: used_model,
        target,
    })
}

/// Post-processor for figure scope: strip optional ```json fences and
/// validate that the result parses as a JSON object. Returns the cleaned
/// JSON body.
fn extract_figure_json(raw: &str) -> Result<String> {
    let cleaned = raw
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim()
        .to_string();
    serde_json::from_str::<serde_json::Value>(&cleaned)
        .with_context(|| format!("figure translation did not parse as JSON: {cleaned}"))?;
    Ok(cleaned)
}

/// High-frequency thesis identifiers (ADR-XXXX, CAR-XX-XXX, FRD-CXX) →
/// short, audience-friendly description that fits inside a figure caption
/// or table label without losing the load-bearing meaning.
///
/// The deflation table is intentionally CONSERVATIVE: it only covers
/// identifiers that appear in `figspec` blocks today and whose
/// translated form ("ADR-0017 Betriebsmodi" / "ADR-0017 modes") would
/// otherwise read as a mystery code to anyone without the SDD chain to
/// hand. Identifiers absent from the table are left untouched — better
/// to preserve precision than to invent a phrase.
const DEFLATION_MAP: &[(&str, &str)] = &[
    // The table is ordered by raw frequency inside the corpus's figspec
    // blocks (2026-06-07 tally): ADR-0040 / ADR-0017 first (18×), then
    // ADR-0054 (9×), then the FRD-Cxx campaign frontmatter (4-5×), then
    // the load-bearing rest. Anything not in the table is kept verbatim.
    //
    // ADR-0017: three operating modes (normal / escalation / catastrophic).
    (
        "ADR-0017",
        "three operating modes (normal / escalation / catastrophic)",
    ),
    // ADR-0040: 5/10/15-year Risk-Adjusted Metadata Prediction (RAMP).
    (
        "ADR-0040",
        "5/10/15-year Risk-Adjusted Metadata Prediction (RAMP)",
    ),
    // ADR-0054: dynamic SBOM / CRA post-deployment lifecycle (VEX + SBOM-update).
    (
        "ADR-0054",
        "dynamic SBOM lifecycle (CRA VEX + SBOM-update service)",
    ),
    // ADR-0035: FHNW 60-page body cap + EN-core / DE-bilingual rendering.
    (
        "ADR-0035",
        "FHNW 60-page body cap with EN-core thesis policy",
    ),
    // ADR-0039: ML-DSA-87 signed-audit-report seal (FIPS 204).
    ("ADR-0039", "ML-DSA-87 signed-audit-report seal (FIPS 204)"),
    // ADR-0047: irreversible-action discipline (push / tag / publish gates).
    ("ADR-0047", "irreversible-action authorisation discipline"),
    // ADR-0062: multi-language translation pipeline shakedown.
    ("ADR-0062", "multi-language translation pipeline shakedown"),
    // ADR-0013: claim-audit-result records (score + placement + justification).
    (
        "ADR-0013",
        "claim-audit-result records (score + placement + justification)",
    ),
    // ADR-0016: material passport (per-item provenance ledger).
    ("ADR-0016", "material-passport per-item provenance ledger"),
    // ADR-0032: Rust runtime replaces the Python tooling.
    (
        "ADR-0032",
        "Rust runtime (agentic binary) replacing the Python tooling",
    ),
    // ADR-0055: scan-artefact persistence cap (e.g. 100 MB Snyk cap).
    (
        "ADR-0055",
        "scan-artefact persistence cap (100 MB Snyk-export bound)",
    ),
    // ADR-0052: enforced_by frontmatter on every ADR.
    ("ADR-0052", "machine-enforced frontmatter on every ADR"),
    // ADR-0051: SDD chain entry-point checklist.
    ("ADR-0051", "SDD chain entry-point checklist"),
    // ADR-0009: adversarial arbitration protocol (HITL when gates disagree).
    (
        "ADR-0009",
        "adversarial-arbitration protocol (HITL escalation when gates disagree)",
    ),
    // FRD-C09 / C10 / C11 — three campaign feature definitions referenced
    // across the campaign book and the Snyk evidence figures.
    ("FRD-C09", "Sovereign Photon OS campaign brief"),
    (
        "FRD-C10",
        "Deterministic Dependency Constraint (DDC) Pilot brief",
    ),
    ("FRD-C11", "ISO 5230 OpenChain licence-compliance brief"),
    ("FRD-C01", "claim-audit-discipline campaign brief"),
    ("FRD-C02", "least-privilege service-account campaign brief"),
    ("FRD-C03", "PQC migration campaign brief"),
    // CAR-XX-XXX: numbered claim-audit records that appear in the campaign
    // figspec blocks. We don't try to summarise every CAR (~30+ exist);
    // we collapse the ones that recur most often in the figures.
    (
        "CAR-06-002",
        "Photon OS classical-baseline CAR (quantum migration)",
    ),
    (
        "CAR-10-003",
        "Photon OS software-engineering CAR (deterministic-builder)",
    ),
    // CWE-79 / CWE-89 / CWE-787 / CWE-327 / CWE-129 / CWE-798 —
    // expand to the recognisable weakness name; do NOT keep the CWE-XXX
    // identifier inside the replacement (it would re-match on the next
    // deflation pass and nest the prefix infinitely, corrupting the
    // figspec body — observed bug 2026-06-07).
    ("CWE-79", "Cross-Site Scripting"),
    ("CWE-89", "SQL Injection"),
    ("CWE-129", "Improper Validation of Array Index"),
    ("CWE-327", "Use of Broken or Risky Cryptographic Algorithm"),
    ("CWE-787", "Out-of-Bounds Write"),
    ("CWE-798", "Hard-Coded Credentials"),
    // CVSS 7+ band — informal but how the thesis renders the severity ribbon.
    ("CVSS≥7", "high-or-critical severity"),
];

/// Old, retired CWE pairs from the buggy first DEFLATION_MAP where the
/// replacement string contained the needle itself. Listed here ONLY so
/// the one-shot repair pass can detect and collapse the nested-prefix
/// corruption it produced (`name (name (name (CWE-XXX)))` etc.). Do NOT
/// add new entries; the invariant test
/// `deflate_map_has_no_self_recursive_replacements` enforces that the
/// live `DEFLATION_MAP` can never seed a new chain.
const NESTED_CORRUPTION_REPAIR: &[(&str, &str)] = &[
    ("Cross-Site Scripting", "CWE-79"),
    ("SQL Injection", "CWE-89"),
    ("Improper Validation of Array Index", "CWE-129"),
    ("Use of Broken or Risky Cryptographic Algorithm", "CWE-327"),
    ("Out-of-Bounds Write", "CWE-787"),
    ("Hard-Coded Credentials", "CWE-798"),
];

/// Detect and collapse nested-CWE corruption produced by an earlier
/// buggy deflation pass. For each (name, needle) pair, find chains of
/// the form `(name (name (… (needle) …)))` with K opening `(name (`
/// prefixes and K matching trailing `)` characters, and replace the
/// whole chain with just `name`. Idempotent on clean text.
///
/// After the chain collapse, a separate dedupe pass merges any
/// `<name> <name>` adjacency that the chain collapse leaves behind
/// (the FIRST `name` instance from pass 1 of the buggy deflater has
/// no leading `(`, so the chain matcher cannot consume it — the
/// dedupe pass cleans the duplicate).
#[must_use]
fn collapse_nested_corruption(text: &str) -> String {
    let mut out = text.to_string();
    for (name, needle) in NESTED_CORRUPTION_REPAIR {
        let outer_open = format!("({} ", name);
        let inner = format!("({})", needle);
        let mut search_start = 0usize;
        loop {
            let Some(rel) = out[search_start..].find(&outer_open) else {
                break;
            };
            let start = search_start + rel;
            let mut depth = 1usize;
            let mut cursor = start + outer_open.len();
            while out[cursor..].starts_with(&outer_open) {
                depth += 1;
                cursor += outer_open.len();
            }
            if !out[cursor..].starts_with(&inner) {
                search_start = start + outer_open.len();
                continue;
            }
            let after_inner = cursor + inner.len();
            let close_count = out[after_inner..].chars().take_while(|c| *c == ')').count();
            if close_count < depth {
                search_start = start + outer_open.len();
                continue;
            }
            let final_end = after_inner + depth;
            let mut acc = String::with_capacity(out.len());
            acc.push_str(&out[..start]);
            acc.push_str(name);
            acc.push_str(&out[final_end..]);
            out = acc;
            search_start = start + name.len();
        }
        // Dedupe pass: `<name> <name>` → `<name>`. Iterates until no
        // more dups exist. Safe because legitimate figspec text never
        // repeats one of the deflation names back-to-back.
        let dup = format!("{name} {name}");
        while let Some(pos) = out.find(&dup) {
            out.replace_range(pos..pos + dup.len(), name);
        }
    }
    out
}

/// Apply the deflation map to a single string. Identifiers are replaced
/// left-to-right; an identifier already inside another identifier (e.g.
/// `ADR-00170` would not match `ADR-0017` because we anchor on a
/// non-digit boundary) is left alone. Returns the rewritten string.
#[must_use]
fn deflate_identifiers_in_text(text: &str) -> String {
    let mut out = text.to_string();
    for (needle, replacement) in DEFLATION_MAP {
        // Word-boundary-safe pass: scan with manual index walk so we can
        // assert the character following the match is NOT a digit or
        // letter (so `ADR-0017` doesn't capture `ADR-00170`).
        let mut acc = String::with_capacity(out.len());
        let mut cursor = 0usize;
        while let Some(pos) = out[cursor..].find(needle) {
            let abs = cursor + pos;
            let end = abs + needle.len();
            let next_is_word = out[end..]
                .chars()
                .next()
                .map(|c| c.is_ascii_alphanumeric())
                .unwrap_or(false);
            if next_is_word {
                acc.push_str(&out[cursor..end]);
                cursor = end;
                continue;
            }
            acc.push_str(&out[cursor..abs]);
            acc.push_str(replacement);
            cursor = end;
        }
        acc.push_str(&out[cursor..]);
        out = acc;
    }
    out
}

/// Walk every ` ```figspec ` block in `md`, apply [`deflate_identifiers_in_text`]
/// to its body, and re-stitch the document. Prose outside the blocks is
/// preserved verbatim so the running text of the thesis keeps its
/// SDD-precise identifiers.
#[must_use]
fn deflate_figspec_blocks(md: &str) -> String {
    let mut out = String::with_capacity(md.len());
    let mut rest = md;
    const OPENER: &str = "```figspec";
    while let Some(start) = rest.find(OPENER) {
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        let mut body_start = OPENER.len();
        let mut newline_consumed = false;
        if after[body_start..].starts_with('\n') {
            body_start += 1;
            newline_consumed = true;
        }
        let Some(end_rel) = after[body_start..].find("```") else {
            out.push_str(after);
            return out;
        };
        let body = &after[body_start..body_start + end_rel];
        out.push_str(OPENER);
        if newline_consumed {
            out.push('\n');
        }
        // First repair any nested-CWE corruption left by the buggy
        // pre-2026-06-07 map, then apply the live deflation map.
        let repaired = collapse_nested_corruption(body);
        out.push_str(&deflate_identifiers_in_text(&repaired));
        out.push_str("```");
        rest = &after[body_start + end_rel + 3..];
    }
    out.push_str(rest);
    out
}

/// Run the corpus-wide deflation pass. Scans every `*.md` blob in the
/// project's worktree, applies `deflate_figspec_blocks`, and writes the
/// blob back when content changed. With `dry_run` the writes are skipped
/// and the per-path delta is printed. The pass is idempotent.
pub fn run_deflate(
    db_path: &Path,
    project: &str,
    id: Option<&str>,
    dry_run: bool,
    json_out: bool,
) -> Result<()> {
    let conn = agentic_core::db::open(db_path).context("open db")?;
    let all = agentic_core::worktree::list(&conn, project, "")?;
    let mut changed = 0usize;
    let mut scanned = 0usize;
    let mut changed_paths: Vec<String> = Vec::new();
    for (path, _) in &all {
        if !path.ends_with(".md") {
            continue;
        }
        let blob = match agentic_core::worktree::read_at(&conn, project, path) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let body = String::from_utf8_lossy(&blob.content).to_string();
        // Cheap reject: nothing to do if there's no figspec opener.
        if !body.contains("```figspec") {
            continue;
        }
        scanned += 1;
        // When `--id <figspec_id>` is set, skip files that don't contain
        // that specific figspec block.
        if let Some(target_id) = id {
            if !figspec_id_present(&body, target_id) {
                continue;
            }
        }
        let updated = deflate_figspec_blocks(&body);
        if updated == body {
            continue;
        }
        changed += 1;
        changed_paths.push(path.clone());
        if dry_run {
            println!("  · would deflate {path}");
            continue;
        }
        let commit_msg =
            format!("deflate-identifiers: {path} (ADR/CWE collapsed to audience-friendly forms)");
        agentic_core::worktree::put_at(
            &conn,
            project,
            path,
            updated.as_bytes(),
            "text/markdown",
            Some("en"),
            "agentic-translate-deflate",
            &commit_msg,
        )?;
        println!("  ✓ deflated {path}");
    }
    if json_out {
        println!(
            "{}",
            serde_json::json!({
                "dry_run": dry_run,
                "id": id,
                "scanned_figspec_files": scanned,
                "changed_files": changed,
                "changed_paths": changed_paths,
            })
        );
    } else {
        println!(
            "\ndeflate-identifiers DONE: scanned={scanned} figspec file(s), \
             {} {} change(s).",
            if dry_run { "would apply" } else { "applied" },
            changed
        );
    }
    Ok(())
}

/// Cheap scan: does any ` ```figspec ` block in `md` carry a top-level
/// `"id"` equal to `target`? Used to scope a deflation pass to a single
/// figspec when `--id` is set.
fn figspec_id_present(md: &str, target: &str) -> bool {
    let mut rest = md;
    const OPENER: &str = "```figspec";
    while let Some(start) = rest.find(OPENER) {
        let after = &rest[start..];
        let mut body_start = OPENER.len();
        if after[body_start..].starts_with('\n') {
            body_start += 1;
        }
        let Some(end_rel) = after[body_start..].find("```") else {
            return false;
        };
        if json_id_matches(&after[body_start..body_start + end_rel], target) {
            return true;
        }
        rest = &after[body_start + end_rel + 3..];
    }
    false
}

pub async fn run(db_path: &Path, action: TranslateAction, json_out: bool) -> Result<()> {
    if let TranslateAction::DeflateIdentifiers {
        project,
        id,
        dry_run,
    } = &action
    {
        return run_deflate(db_path, project, id.as_deref(), *dry_run, json_out);
    }
    let TranslateAction::Scope {
        project,
        target,
        source,
        scope,
        id,
        provider,
        dry_run,
    } = action
    else {
        unreachable!("DeflateIdentifiers handled above")
    };
    if !ALLOWED_TARGETS.contains(&target.as_str()) {
        anyhow::bail!(
            "target '{target}' is not a supported language; valid: {}",
            ALLOWED_TARGETS.join(", ")
        );
    }
    if !ALLOWED_SOURCES.contains(&source.as_str()) {
        anyhow::bail!(
            "source '{source}' is not a supported language; valid: {}",
            ALLOWED_SOURCES.join(", ")
        );
    }
    if source == target {
        anyhow::bail!("source and target must differ ({source} == {target}); nothing to translate");
    }
    let scope_enum = Scope::parse(&scope, id.as_deref())?;
    let conn = agentic_core::db::open(db_path).context("open db")?;
    let paths = enumerate_scope(&conn, &project, &scope_enum)?;
    let provider_label = provider.as_deref().unwrap_or("(first configured cloud)");

    if dry_run {
        // Figure scope: locate the figspec by id (no LLM call) and report
        // the resolved (source-path, sidecar-target) pair so the operator
        // sees exactly what would happen. For non-EN source the locator
        // points at `out/figures/<id>_<source>.json` (a previous-step
        // sidecar) rather than the EN corpus markdown.
        if let Scope::Figure { id } = &scope_enum {
            let sidecar = format!("out/figures/{id}_{target}.json");
            let (located_path, located_ok): (Option<String>, bool) = if source == "en" {
                let r = find_figspec_by_id(&conn, &project, id)?;
                (r.as_ref().map(|(p, _)| p.clone()), r.is_some())
            } else {
                let p = format!("out/figures/{id}_{source}.json");
                let ok = agentic_core::worktree::read_at(&conn, &project, &p).is_ok();
                (Some(p), ok)
            };
            if json_out {
                println!(
                    "{}",
                    serde_json::json!({
                        "dry_run": true,
                        "source": source,
                        "target": target,
                        "scope": scope_enum.slug(),
                        "id": id,
                        "located": located_ok,
                        "source_path": located_path,
                        "sidecar_path": sidecar,
                        "provider": provider_label,
                        "would_authorise": false,
                    })
                );
            } else {
                println!(
                    "[dry-run] source={source} target={target} scope={} provider={}",
                    scope_enum.slug(),
                    provider_label
                );
                match (&located_path, located_ok) {
                    (Some(src), true) => {
                        println!("[dry-run] figspec id={id} located in {src}");
                        println!("[dry-run] would write sidecar to {sidecar}");
                    }
                    (Some(src), false) => {
                        println!(
                            "[dry-run] non-EN source sidecar {src} NOT FOUND; run \
                             the EN→{source} step first"
                        );
                    }
                    (None, _) => {
                        println!("[dry-run] figspec id={id} NOT FOUND in the corpus");
                    }
                }
                println!(
                    "[dry-run] No LLM calls made, no blobs written. To run for real, first \
                     `agentic authorize grant --project {project} --action translate --rationale '<why>'`."
                );
            }
            return Ok(());
        }
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

    // Build the provider attempt order. The first-choice provider honors:
    //   1. explicit --provider <p> (no fallback — user said exactly which)
    //   2. AGENTIC_TRANSLATE_PROVIDER env var (via router::route)
    //   3. AGENTIC_DEFAULT_PROVIDER env var (via router::route)
    //   4. CLI context default (Claude Code → Anthropic, etc.)
    //   5. Available-key scan in the router's preferred order
    // The 2026-06-07 fallback addition: when the first-choice provider's
    // chat call fails (e.g. Anthropic returns HTTP 400 "credit balance too
    // low" — a common shakedown gotcha because Anthropic Workbench API
    // credits are a separate billing pool from Claude Max subscription
    // quota), walk the rest of CLOUD_TRANSLATE skipping already-tried
    // providers, picking only those with a configured key. Explicit
    // --provider disables this walk (the user said exactly which one).
    let explicit_choice = provider
        .as_deref()
        .map(|p| parse_kind(p).ok_or_else(|| anyhow!("unknown provider '{p}'")))
        .transpose()?;
    let first_choice = if let Some(k) = explicit_choice {
        k
    } else {
        router::route(Task::Translate).kind
    };
    let mut attempt_order: Vec<ProviderKind> = vec![first_choice];
    if explicit_choice.is_none() {
        for k in CLOUD_TRANSLATE {
            if !attempt_order.contains(k) && registry::has_key(*k) {
                attempt_order.push(*k);
            }
        }
    }

    // ADR-0062 Phase A — per-figure scope. Locate the figspec by `id`,
    // translate its user-facing text fields with the figure-aware
    // system prompt, and write a sidecar JSON blob. When `source != en`
    // we're inside a round-trip chain and the source comes from a
    // previously-written sidecar (`out/figures/<id>_<source>.json`)
    // rather than the EN corpus markdown.
    if let Scope::Figure { id } = &scope_enum {
        let (source_path, figspec_json) = if source == "en" {
            find_figspec_by_id(&conn, &project, id)?
                .ok_or_else(|| anyhow!("no figspec with id='{id}' found in the corpus"))?
        } else {
            let sidecar_src = format!("out/figures/{id}_{source}.json");
            let blob = agentic_core::worktree::read_at(&conn, &project, &sidecar_src)
                .with_context(|| {
                    format!(
                        "non-EN source figure sidecar {sidecar_src} not found; run \
                         the EN→{source} step first"
                    )
                })?;
            (
                sidecar_src,
                String::from_utf8_lossy(&blob.content).to_string(),
            )
        };
        println!(
            "translate-figure: id={id} located in {source_path} (source={source}); \
             fallback order = {attempt_order:?}"
        );
        let outcome = translate_with_cache(
            &conn,
            &project,
            &source,
            &target,
            &figspec_json,
            "figspec",
            &attempt_order,
            figure_system_prompt(&source, &target),
            4096,
            extract_figure_json,
        )
        .await?;
        let sidecar_path = format!("out/figures/{id}_{target}.json");
        let commit_msg = format!(
            "translate-figure: {id} (from {source_path}) → {sidecar_path} \
             (target={target}, provider={}, model={}, cached={})",
            outcome.provider, outcome.model, outcome.cached,
        );
        let sha = agentic_core::worktree::put_at(
            &conn,
            &project,
            &sidecar_path,
            outcome.target.as_bytes(),
            "application/json",
            Some(&target),
            "agentic-translate",
            &commit_msg,
        )?;
        let cache_tag = if outcome.cached {
            "CACHE-HIT"
        } else {
            "cache-miss"
        };
        println!(
            "  ✓ figure id={id} translated to {target} via {} ({}) [{cache_tag}] \
             ({} bytes) → {sidecar_path} (commit {})",
            outcome.provider,
            outcome.model,
            outcome.target.len(),
            &sha[..12.min(sha.len())]
        );
        if json_out {
            println!(
                "{}",
                serde_json::json!({
                    "scope": scope_enum.slug(),
                    "id": id,
                    "target": target,
                    "source_path": source_path,
                    "sidecar_path": sidecar_path,
                    "bytes_written": outcome.target.len(),
                    "provider": outcome.provider,
                    "model": outcome.model,
                    "cached": outcome.cached,
                    "commit": sha,
                })
            );
        }
        return Ok(());
    }

    println!(
        "translate target={target} scope={scope} fallback_order={attempt_order:?} \
         paths={} — beginning per-path LLM-translate-and-write loop (ADR-0062 v1).",
        paths.len()
    );

    let mut written = 0usize;
    let mut skipped_empty = 0usize;
    let mut cache_hits = 0usize;
    let mut cache_misses = 0usize;
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
        match translate_with_cache(
            &conn,
            &project,
            &source,
            &target,
            &source_md,
            "document",
            &attempt_order,
            system_prompt(&source, &target),
            8192,
            |raw| Ok(raw.to_string()),
        )
        .await
        {
            Ok(outcome) => {
                let bytes = outcome.target.as_bytes();
                let cache_tag = if outcome.cached {
                    cache_hits += 1;
                    "CACHE-HIT"
                } else {
                    cache_misses += 1;
                    "cache-miss"
                };
                let commit_msg = format!(
                    "translate: {src} → {target_path_str} \
                     (target={target}, provider={}, model={}, cached={})",
                    outcome.provider, outcome.model, outcome.cached,
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
                            "    ✓ {} bytes written to {target_path_str} via {} \
                             ({}) [{cache_tag}] (commit {})",
                            outcome.target.len(),
                            outcome.provider,
                            outcome.model,
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
                "fallback_order": attempt_order.iter().map(|k| format!("{k:?}")).collect::<Vec<_>>(),
                "written": written,
                "skipped_empty": skipped_empty,
                "cache_hits": cache_hits,
                "cache_misses": cache_misses,
                "failed": failed,
            })
        );
    } else {
        println!(
            "\ntranslate target={target} scope={scope} DONE: written={written}, \
             skipped_empty={skipped_empty}, cache_hits={cache_hits}, \
             cache_misses={cache_misses}, failed={}.",
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
        assert_eq!(
            Scope::parse("front-matter", None).unwrap(),
            Scope::FrontMatter
        );
        assert_eq!(
            Scope::parse("frontmatter", None).unwrap(),
            Scope::FrontMatter
        );
        assert_eq!(Scope::parse("fm", None).unwrap(), Scope::FrontMatter);
        assert_eq!(Scope::parse("thesis", None).unwrap(), Scope::Thesis);
        assert_eq!(Scope::parse("corpus", None).unwrap(), Scope::Corpus);
        assert!(Scope::parse("bogus", None).is_err());
        // `figure` without --id fails fast.
        assert!(Scope::parse("figure", None).is_err());
        assert!(Scope::parse("figure", Some("")).is_err());
        // `figure` with an id resolves to Figure { id }.
        assert_eq!(
            Scope::parse("figure", Some("fig08_03")).unwrap(),
            Scope::Figure {
                id: "fig08_03".into()
            }
        );
    }

    #[test]
    fn json_id_match_handles_realistic_figspec() {
        let body = r#"{"id":"fig08_03","type":"bar","title":"x","caption":"c","data":{"labels":["Low"],"values":[22]}}"#;
        assert!(json_id_matches(body, "fig08_03"));
        assert!(!json_id_matches(body, "fig08_04"));
        // Spaces around the colon and other ids in the body do not confuse
        // the matcher.
        let spaced = r#"{ "id"  :  "fig09_01" , "type":"flow", "data":{"id":"ignored"} }"#;
        assert!(json_id_matches(spaced, "fig09_01"));
        assert!(!json_id_matches(spaced, "ignored"));
    }

    #[test]
    fn figure_system_prompt_locks_invariants() {
        let p = figure_system_prompt("en", "de");
        assert!(p.contains("English"));
        assert!(p.contains("Swiss Standard German"));
        assert!(p.contains("preserve every structural")); // structural keys preserved
        assert!(p.contains("ADR-XXXX")); // identifier invariant
        assert!(p.contains("ONLY the modified JSON")); // output format
        // Swiss orthography addendum must appear for DE target.
        assert!(p.contains("NO eszett"));
        assert!(p.contains("«…»"));
    }

    #[test]
    fn figure_system_prompt_handles_non_en_source() {
        // Round-trip chain middle step: DE → FR. The DE source label is
        // the source-language label (Swiss German), and FR is the target.
        let p = figure_system_prompt("de", "fr");
        assert!(p.contains("Swiss Standard German"));
        assert!(p.contains("French (français)"));
        // Round-trip closing step: HI → EN. English is allowed as a target.
        let q = figure_system_prompt("hi", "en");
        assert!(q.contains("Hindi"));
        assert!(q.contains("English"));
    }

    #[test]
    fn swiss_german_addendum_only_for_de_target() {
        assert!(swiss_german_addendum("de").contains("NO eszett"));
        assert!(swiss_german_addendum("de").contains("«…»"));
        assert_eq!(swiss_german_addendum("fr"), "");
        assert_eq!(swiss_german_addendum("it"), "");
        assert_eq!(swiss_german_addendum("rm"), "");
        assert_eq!(swiss_german_addendum("hi"), "");
        assert_eq!(swiss_german_addendum("en"), "");
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
        assert_eq!(
            language_label("de"),
            "Swiss Standard German (Schweizerhochdeutsch)"
        );
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
        let p = system_prompt("en", "de");
        assert!(p.contains("English"));
        assert!(p.contains("Swiss Standard German"));
        assert!(p.contains("EXACTLY AS-IS")); // figspec / code invariant
        assert!(p.contains("ADR-XXXX")); // identifier invariant
        assert!(p.contains("SOLELY in")); // purity invariant
        // Swiss orthography addendum applies to document scope too.
        assert!(p.contains("NO eszett"));
    }

    #[test]
    fn deflate_known_identifiers_replaces_high_frequency_ids() {
        // High-frequency thesis identifiers must collapse to their
        // audience-friendly form so the translation pipeline doesn't
        // carry "ADR-0017" through into the German caption.
        let before = "Per ADR-0017 the runtime degrades safely.";
        let after = deflate_identifiers_in_text(before);
        assert!(after.contains("operating modes"));
        assert!(!after.contains("ADR-0017"));

        let before2 = "The signed-report stack is rooted in ADR-0039.";
        let after2 = deflate_identifiers_in_text(before2);
        assert!(after2.contains("ML-DSA-87"));
        assert!(!after2.contains("ADR-0039"));
    }

    /// Idempotency invariant: a replacement string MUST NOT contain
    /// ANY needle from the map. Otherwise re-running the deflater nests
    /// the replacement infinitely (observed corruption bug 2026-06-07
    /// where `CWE-327` → `"… (CWE-327)"` produced six-deep nesting).
    /// This test is the load-bearing safety net that prevents the bug
    /// from reappearing the next time someone adds an entry.
    #[test]
    fn deflate_map_has_no_self_recursive_replacements() {
        for (needle_a, replacement_a) in DEFLATION_MAP {
            for (needle_b, _) in DEFLATION_MAP {
                assert!(
                    !replacement_a.contains(needle_b),
                    "replacement for `{needle_a}` (`{replacement_a}`) contains \
                     needle `{needle_b}` — this would nest infinitely on \
                     subsequent deflation passes."
                );
            }
        }
    }

    #[test]
    fn deflate_leaves_unknown_identifiers_untouched() {
        // Anything not in the deflation table is preserved verbatim so
        // we never silently drop SDD precision the reader might need.
        let before = "See ADR-9999 (no such ADR) and CVE-2024-00001.";
        let after = deflate_identifiers_in_text(before);
        assert_eq!(after, before);
    }

    #[test]
    fn collapse_repairs_5_level_cwe_corruption() {
        // The exact shape observed in Dimension_02 block 2 (2026-06-07).
        // 5 nested `(Use of Broken or Risky Cryptographic Algorithm (` chains
        // wrapping a single `(CWE-327)` token with 5 trailing `)`s.
        let corrupted = "(1,385 Use of Broken or Risky Cryptographic Algorithm \
                         (Use of Broken or Risky Cryptographic Algorithm \
                         (Use of Broken or Risky Cryptographic Algorithm \
                         (Use of Broken or Risky Cryptographic Algorithm \
                         (Use of Broken or Risky Cryptographic Algorithm \
                         (CWE-327))))) crypto)";
        let repaired = collapse_nested_corruption(corrupted);
        assert!(
            !repaired.contains(
                "(Use of Broken or Risky Cryptographic Algorithm \
                                (Use of Broken or Risky"
            ),
            "nested-chain prefix not collapsed: {repaired}"
        );
        assert!(
            repaired.contains("Use of Broken or Risky Cryptographic Algorithm"),
            "expected single weakness name to survive the collapse: {repaired}"
        );
        assert!(
            !repaired.contains("(CWE-327)"),
            "expected bare CWE-327 token to be removed: {repaired}"
        );
    }

    #[test]
    fn collapse_repairs_single_level_corruption() {
        // K=1 form — single wrap from one deflate pass with the buggy map.
        let corrupted = "(1,385 Use of Broken or Risky Cryptographic Algorithm \
                         (CWE-327) crypto)";
        let repaired = collapse_nested_corruption(corrupted);
        assert!(
            !repaired.contains("(CWE-327)"),
            "K=1 chain not collapsed: {repaired}"
        );
        assert!(
            repaired.contains("Use of Broken or Risky Cryptographic Algorithm"),
            "expected name to survive: {repaired}"
        );
    }

    #[test]
    fn collapse_leaves_clean_text_untouched() {
        let clean = "(1,385 CWE-327 crypto) and CWE-79 too";
        let repaired = collapse_nested_corruption(clean);
        assert_eq!(repaired, clean);
    }

    #[test]
    fn collapse_dedupes_already_repaired_chain_residue() {
        // Real-world residue from the first (buggy) repair pass: the
        // bare leading NAME survived the chain collapse and now sits
        // next to a second NAME from the replacement. The dedupe pass
        // must merge the two.
        let residue = "(1,385 Use of Broken or Risky Cryptographic Algorithm \
                       Use of Broken or Risky Cryptographic Algorithm crypto)";
        let repaired = collapse_nested_corruption(residue);
        assert!(
            repaired.contains("Use of Broken or Risky Cryptographic Algorithm crypto"),
            "expected single weakness name before `crypto`: {repaired}"
        );
        assert!(
            !repaired.contains(
                "Use of Broken or Risky Cryptographic Algorithm \
                 Use of Broken or Risky Cryptographic Algorithm"
            ),
            "dedupe pass left adjacent duplicate: {repaired}"
        );
    }

    #[test]
    fn deflate_only_text_fields_in_figspec_block() {
        let md = "preamble\n\n```figspec\n{\
\"id\":\"fig08_03\",\
\"type\":\"bar\",\
\"title\":\"ADR-0017 modes\",\
\"caption\":\"Per ADR-0017\",\
\"data\":{\"labels\":[\"Normal\"],\"values\":[1]}\
}\n```\n\npostamble ADR-0017 should NOT change here";
        let out = deflate_figspec_blocks(md);
        // Outside the block, identifier untouched (it's body prose).
        assert!(out.contains("postamble ADR-0017 should NOT change here"));
        // Inside the block, ADR-0017 collapsed to its friendly form.
        let block_start = out.find("```figspec").unwrap();
        let block_end = out[block_start + 10..].find("```").unwrap() + block_start + 10;
        let block = &out[block_start..block_end];
        assert!(!block.contains("ADR-0017"));
        assert!(block.contains("operating modes"));
    }
}
