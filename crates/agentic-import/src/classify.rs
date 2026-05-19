//! Folder classification — two strategies.
//!
//! Given a set of "slots" (each is a name + short description such as
//! "intro = introductory chapter that motivates the work"), rank slots per
//! chapter against the slot list. Two strategies:
//!
//!   * **Embed** (`classify_project`): embed both the slot descriptions and
//!     every chapter, rank by cosine similarity. Best when a high-quality
//!     embedding model (Voyage, OpenAI text-embedding-3) is available.
//!   * **Chat** (`classify_project_chat`): send each chapter + the slot list
//!     to the chat provider, ask for a JSON `{placement, score, justification,
//!     alternatives}` and parse it. Works with any chat-capable provider —
//!     no separate embed key needed. Recommended for the Anthropic-only
//!     (or any single-frontier-provider) setup.
//!
//! `auto_classify_project` picks Chat when no embed-capable provider is
//! configured, Embed otherwise.

use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use agentic_core::{content::blob, embeddings, worktree};
use agentic_providers::{
    ChatMessage, ChatRequest, EmbeddingRequest, Provider, ProviderKind, Role, Task, registry,
    router, traits::ProviderError,
};

use crate::embed::embed_project_blobs;

/// Which ranking strategy to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Strategy {
    /// Cosine ranking on embedded vectors. Needs an embed-capable provider.
    Embed,
    /// LLM-driven ranking via `provider.chat()`. Works with any chat-capable provider.
    Chat,
}

impl std::str::FromStr for Strategy {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "embed" | "embeddings" | "cosine" => Ok(Self::Embed),
            "chat" | "llm" => Ok(Self::Chat),
            other => Err(anyhow!("unknown classify strategy: {other}")),
        }
    }
}

/// Pick the default strategy by asking the router which provider *would*
/// actually be picked for each task, then verifying that provider has a
/// configured key.
///
/// This is stricter than "is there any embed-capable provider with a key?"
/// because the router's per-task fallback can resolve to a provider that
/// isn't actually configured (e.g. the embed fallback is hard-coded to
/// Voyage in `agentic-providers::router`). If the router would commit us to
/// a keyless Voyage call, that's not a viable Embed strategy.
///
/// Logic:
/// 1. Ask `router::route(Task::Embed)`. If that provider has a key and can
///    serve `Task::Embed` → [`Strategy::Embed`].
/// 2. Else ask `router::route(Task::Chat)`. If that provider has a key and
///    can serve `Task::Chat` → [`Strategy::Chat`].
/// 3. Else error — no provider is configured for either path.
pub fn auto_strategy() -> Result<Strategy> {
    let embed_route = router::route(Task::Embed);
    if registry::has_key(embed_route.kind) && router::supports_task(embed_route.kind, Task::Embed) {
        return Ok(Strategy::Embed);
    }
    let chat_route = router::route(Task::Chat);
    if registry::has_key(chat_route.kind) && router::supports_task(chat_route.kind, Task::Chat) {
        return Ok(Strategy::Chat);
    }
    Err(anyhow!(
        "no provider with a configured key for either Embed or Chat — \
         set ANTHROPIC_API_KEY (or another vendor env var: OPENAI_API_KEY, \
         GOOGLE_API_KEY, MISTRAL_API_KEY, COHERE_API_KEY, VOYAGE_API_KEY, \
         XAI_API_KEY) and retry. Currently router would pick \
         {embed_for_embed:?} for Embed and {chat_for_chat:?} for Chat.",
        embed_for_embed = embed_route.kind,
        chat_for_chat = chat_route.kind
    ))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Slot {
    pub key: String,
    pub description: String,
}

impl Slot {
    pub fn new(key: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            description: description.into(),
        }
    }
}

/// Default thesis-chapter slot set (six standard chapters). Override at the
/// call site for portfolio / non-standard projects.
#[must_use]
pub fn default_slots() -> Vec<Slot> {
    vec![
        Slot::new(
            "intro",
            "Introduction: motivation, research question, scope, contributions.",
        ),
        Slot::new(
            "related_work",
            "Related work / literature review: prior approaches, gaps, positioning.",
        ),
        Slot::new(
            "methodology",
            "Methodology / approach: research design, datasets, models, procedures.",
        ),
        Slot::new(
            "results",
            "Results / evaluation: experiments, measurements, tables, figures.",
        ),
        Slot::new(
            "discussion",
            "Discussion: interpretation, threats to validity, limitations, implications.",
        ),
        Slot::new(
            "conclusion",
            "Conclusion and future work: takeaways, open problems, next steps.",
        ),
    ]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlotMatch {
    pub key: String,
    pub score: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChapterAssignment {
    pub path: String,
    pub blob_sha: String,
    /// Slots ranked best-first; element 0 is the suggested assignment.
    pub ranked: Vec<SlotMatch>,
}

/// Classify every embedded markdown chapter in the project against `slots`.
///
/// The function first ensures embeddings exist for the chapters (it will
/// embed any missing ones), then embeds each slot description, then ranks.
/// Returns one [`ChapterAssignment`] per chapter.
pub async fn classify_project(
    conn: &Connection,
    project_id: &str,
    prefix: &str,
    slots: &[Slot],
    provider_override: Option<&str>,
    model_override: Option<&str>,
) -> Result<Vec<ChapterAssignment>> {
    if slots.is_empty() {
        return Err(anyhow!("no slots provided"));
    }

    // Make sure every chapter has an embedding (no-op for those that already do).
    let _ = embed_project_blobs(
        conn,
        project_id,
        prefix,
        provider_override,
        model_override,
        true,
    )
    .await?;

    // Resolve target identically to the embed pipeline so we use the same
    // (provider, model) for slot embeddings.
    let kind = match provider_override {
        Some(s) => s
            .parse::<ProviderKind>()
            .map_err(|e| anyhow!("invalid provider {s}: {e}"))?,
        None => router::route(Task::Embed).kind,
    };
    let model = model_override
        .map(str::to_owned)
        .unwrap_or_else(|| router::default_model(kind, Task::Embed).to_owned());

    let provider: Arc<dyn Provider> =
        registry::build(kind).map_err(|e| anyhow!("build provider: {e}"))?;

    let slot_descs: Vec<String> = slots.iter().map(|s| s.description.clone()).collect();
    let slot_resp = match provider
        .embed(&EmbeddingRequest {
            model: model.clone(),
            texts: slot_descs,
        })
        .await
    {
        Ok(r) => r,
        Err(ProviderError::Unimplemented(p)) => {
            return Err(anyhow!("provider {p} has no embedding API"));
        }
        Err(e) => return Err(anyhow!("embed slots: {e}")),
    };
    if slot_resp.vectors.len() != slots.len() {
        return Err(anyhow!(
            "provider returned {} vectors for {} slot descriptions",
            slot_resp.vectors.len(),
            slots.len()
        ));
    }

    let entries =
        worktree::list(conn, project_id, prefix).map_err(|e| anyhow!("list working tree: {e}"))?;

    let mut out = Vec::new();
    for (path, sha) in entries {
        if !is_markdown_path(&path) {
            continue;
        }
        let Some(chapter_emb) = embeddings::get_embedding(conn, &sha, &model, 0)
            .with_context(|| format!("load embedding for {sha}"))?
        else {
            continue;
        };
        let mut ranked: Vec<SlotMatch> = slots
            .iter()
            .zip(slot_resp.vectors.iter())
            .map(|(slot, vec)| SlotMatch {
                key: slot.key.clone(),
                score: embeddings::cosine(&chapter_emb.vector, vec),
            })
            .collect();
        ranked.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        out.push(ChapterAssignment {
            path,
            blob_sha: sha,
            ranked,
        });
    }
    Ok(out)
}

fn is_markdown_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".md") || lower.ends_with(".markdown")
}

// ----- chat strategy --------------------------------------------------------

/// Dispatch a single classify call to whichever strategy the caller (or
/// auto-detect) chose.
pub async fn classify_project_with_strategy(
    conn: &Connection,
    project_id: &str,
    prefix: &str,
    slots: &[Slot],
    provider_override: Option<&str>,
    model_override: Option<&str>,
    strategy: Strategy,
) -> Result<Vec<ChapterAssignment>> {
    match strategy {
        Strategy::Embed => {
            classify_project(
                conn,
                project_id,
                prefix,
                slots,
                provider_override,
                model_override,
            )
            .await
        }
        Strategy::Chat => {
            classify_project_chat(
                conn,
                project_id,
                prefix,
                slots,
                provider_override,
                model_override,
            )
            .await
        }
    }
}

/// LLM-driven classification. For each markdown blob under `prefix`, send
/// the content + slot list to the chat provider and parse the resulting
/// JSON ranking. **No embeddings, no router fallback to Voyage.**
///
/// The provider is resolved via the standard router precedence for
/// [`Task::Classify`]; explicit `--provider` / `--model` overrides win.
pub async fn classify_project_chat(
    conn: &Connection,
    project_id: &str,
    prefix: &str,
    slots: &[Slot],
    provider_override: Option<&str>,
    model_override: Option<&str>,
) -> Result<Vec<ChapterAssignment>> {
    if slots.is_empty() {
        return Err(anyhow!("no slots provided"));
    }

    // Resolve provider + model for Task::Classify (or honour overrides).
    let kind = match provider_override {
        Some(s) => s
            .parse::<ProviderKind>()
            .map_err(|e| anyhow!("invalid provider {s}: {e}"))?,
        None => router::route(Task::Classify).kind,
    };
    if !router::supports_task(kind, Task::Chat) {
        return Err(anyhow!(
            "provider {kind:?} cannot serve chat-classify (no chat API)"
        ));
    }
    let model = model_override
        .map(str::to_owned)
        .unwrap_or_else(|| router::default_model(kind, Task::Classify).to_owned());
    let provider: Arc<dyn Provider> =
        registry::build(kind).map_err(|e| anyhow!("build provider: {e}"))?;

    let slot_block = slots
        .iter()
        .map(|s| format!("- {}: {}", s.key, s.description))
        .collect::<Vec<_>>()
        .join("\n");
    let valid_keys = slots
        .iter()
        .map(|s| format!("\"{}\"", s.key))
        .collect::<Vec<_>>()
        .join(", ");

    let entries =
        worktree::list(conn, project_id, prefix).map_err(|e| anyhow!("list working tree: {e}"))?;

    let mut out = Vec::new();
    for (path, sha) in entries {
        if !is_markdown_path(&path) {
            continue;
        }
        let b = blob::get_blob(conn, &sha).map_err(|e| anyhow!("load blob {sha}: {e}"))?;
        let body = String::from_utf8(b.content)
            .with_context(|| format!("blob {sha} is not valid utf-8"))?;
        let prompt = build_chat_prompt(&body, &slot_block, &valid_keys);
        let req = ChatRequest {
            model: model.clone(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: prompt,
            }],
            temperature: Some(0.0),
            max_tokens: Some(1024),
            seed: None,
            system: Some(SYSTEM_PROMPT.into()),
        };
        let resp = match provider.chat(&req).await {
            Ok(r) => r,
            Err(ProviderError::Unimplemented(p)) => {
                return Err(anyhow!("provider {p} has no chat API"));
            }
            Err(e) => return Err(anyhow!("chat call on {path}: {e}")),
        };
        let ranked = parse_chat_ranking(&resp.content, slots)
            .with_context(|| format!("parse ranking for {path}"))?;
        out.push(ChapterAssignment {
            path,
            blob_sha: sha,
            ranked,
        });
    }
    Ok(out)
}

const SYSTEM_PROMPT: &str = "You are a research-thesis content classifier. You read a document fragment and rank it \
     against a fixed set of slot descriptions. You respond ONLY with a single JSON object, no \
     markdown fences, no commentary.";

fn build_chat_prompt(body: &str, slot_block: &str, valid_keys: &str) -> String {
    // Cap body length to keep token usage bounded; chapter bodies past
    // ~30 KB are very rare for thesis fragments.
    let truncated = if body.len() > 30_000 {
        format!(
            "{}\n\n[…truncated — original {} bytes]",
            &body[..30_000],
            body.len()
        )
    } else {
        body.to_owned()
    };
    format!(
        r#"Classify the following document fragment against the slot list.

DOCUMENT FRAGMENT:
---
{truncated}
---

SLOTS:
{slot_block}

Return ONE JSON object only, with this shape:
{{
  "placement": "<one of: {valid_keys}>",
  "score": <float in [0.0, 1.0] — confidence of the placement>,
  "justification": "<one to three sentences>",
  "alternatives": [
    {{ "key": "<slot key>", "score": <float in [0.0, 1.0]> }},
    ...
  ]
}}

`alternatives` should list the OTHER slots in your ranked order (best to worst), at most {n}
entries. The top-ranked slot must equal `placement`. Do not include `placement` itself in
`alternatives`. Output JSON only.
"#,
        n = valid_keys.matches(',').count() + 1
    )
}

fn parse_chat_ranking(text: &str, slots: &[Slot]) -> Result<Vec<SlotMatch>> {
    let json_str = extract_json(text)
        .ok_or_else(|| anyhow!("chat response did not contain a JSON object: {text:.200}"))?;
    #[derive(Deserialize)]
    struct ChatRanking {
        placement: String,
        score: f32,
        #[allow(dead_code)]
        #[serde(default)]
        justification: String,
        #[serde(default)]
        alternatives: Vec<SlotMatch>,
    }
    let parsed: ChatRanking = serde_json::from_str(json_str)
        .with_context(|| format!("parse JSON ranking: {json_str:.300}"))?;
    let valid_keys: std::collections::HashSet<&str> =
        slots.iter().map(|s| s.key.as_str()).collect();
    if !valid_keys.contains(parsed.placement.as_str()) {
        return Err(anyhow!(
            "placement '{}' is not in the slot list",
            parsed.placement
        ));
    }
    let mut ranked = vec![SlotMatch {
        key: parsed.placement,
        score: parsed.score,
    }];
    for alt in parsed.alternatives {
        if !valid_keys.contains(alt.key.as_str()) {
            continue;
        }
        if ranked.iter().any(|m| m.key == alt.key) {
            continue;
        }
        ranked.push(alt);
    }
    // Fill missing slots with score 0.0 so the consumer always sees a
    // complete ranking shape.
    for s in slots {
        if !ranked.iter().any(|m| m.key == s.key) {
            ranked.push(SlotMatch {
                key: s.key.clone(),
                score: 0.0,
            });
        }
    }
    Ok(ranked)
}

fn extract_json(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if start <= end {
        Some(&text[start..=end])
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_slots_has_six_named_entries() {
        let slots = default_slots();
        assert_eq!(slots.len(), 6);
        let keys: Vec<&str> = slots.iter().map(|s| s.key.as_str()).collect();
        assert!(keys.contains(&"intro"));
        assert!(keys.contains(&"conclusion"));
    }

    #[test]
    fn extract_json_finds_object_inside_chatter() {
        let s = "Sure! Here is the ranking:\n```json\n{\"placement\":\"intro\",\"score\":0.9}\n```\nThanks";
        let j = extract_json(s).unwrap();
        assert!(j.starts_with('{'));
        assert!(j.ends_with('}'));
        assert!(j.contains("placement"));
    }

    #[test]
    fn parse_chat_ranking_happy_path() {
        let slots = default_slots();
        let resp = r#"{
          "placement": "intro",
          "score": 0.82,
          "justification": "Frames the problem.",
          "alternatives": [
            {"key": "related_work", "score": 0.4},
            {"key": "discussion",   "score": 0.1}
          ]
        }"#;
        let ranked = parse_chat_ranking(resp, &slots).unwrap();
        assert_eq!(ranked[0].key, "intro");
        assert_eq!(ranked[1].key, "related_work");
        assert_eq!(ranked.len(), slots.len(), "missing slots filled with 0.0");
    }

    #[test]
    fn parse_chat_ranking_rejects_unknown_placement() {
        let slots = default_slots();
        let resp =
            r#"{"placement":"appendix-xyz","score":0.5,"justification":"x","alternatives":[]}"#;
        let err = parse_chat_ranking(resp, &slots).unwrap_err();
        assert!(err.to_string().contains("not in the slot list"));
    }

    #[test]
    #[allow(unsafe_code)]
    fn auto_strategy_errors_when_no_provider_has_a_key() {
        // Env-var-isolated test: clear every AGENTIC_* and vendor-native
        // key we know about, save originals, then assert the error path.
        // (Other tests don't touch env, so this can stay in a single test.)
        let keys = [
            "AGENTIC_ANTHROPIC_KEY",
            "AGENTIC_OPENAI_KEY",
            "AGENTIC_GOOGLE_KEY",
            "AGENTIC_MISTRAL_KEY",
            "AGENTIC_COHERE_KEY",
            "AGENTIC_VOYAGE_KEY",
            "AGENTIC_GROK_KEY",
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "GOOGLE_API_KEY",
            "MISTRAL_API_KEY",
            "COHERE_API_KEY",
            "VOYAGE_API_KEY",
            "XAI_API_KEY",
            // Also clear the CLI-context envs so the router takes the
            // fallback path (Voyage for Embed, Anthropic for Chat).
            "CLAUDECODE",
            "CURSOR_TRACE_ID",
            "GEMINI_CLI",
            "CODEX_SESSION",
            "FACTORYAI",
            "GROK_BUILD",
            "XAI_BUILD",
            "AGENTIC_DEFAULT_PROVIDER",
        ];
        let saved: Vec<(String, Option<String>)> = keys
            .iter()
            .map(|k| (k.to_string(), std::env::var(k).ok()))
            .collect();
        // SAFETY: set_var/remove_var are safe in single-threaded test
        // execution. cargo test parallelism is per-test-binary; within a
        // test binary tests run on a thread pool but env mutation IS
        // racy. This test is isolated by clearing/restoring every env
        // var that affects routing, and `cargo test` for this crate
        // runs serially enough in practice. If this becomes flaky, gate
        // it behind --test-threads=1 or use `serial_test`.
        unsafe {
            for k in &keys {
                std::env::remove_var(k);
            }
        }
        let result = auto_strategy();
        // Restore env.
        unsafe {
            for (k, v) in &saved {
                match v {
                    Some(val) => std::env::set_var(k, val),
                    None => std::env::remove_var(k),
                }
            }
        }
        // Now check: with NO keys set, auto_strategy should error
        // (router would route Embed→Voyage and Chat→Anthropic, neither
        // configured) — and the error message should hint at vendor env vars.
        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("ANTHROPIC_API_KEY"),
            "error message should hint at vendor env vars; got: {msg}"
        );
    }

    #[test]
    fn strategy_parses_from_str() {
        use std::str::FromStr;
        assert_eq!(Strategy::from_str("chat").unwrap(), Strategy::Chat);
        assert_eq!(Strategy::from_str("embed").unwrap(), Strategy::Embed);
        assert_eq!(Strategy::from_str("cosine").unwrap(), Strategy::Embed);
        assert_eq!(Strategy::from_str("llm").unwrap(), Strategy::Chat);
        assert!(Strategy::from_str("imaginary").is_err());
    }

    #[test]
    fn slot_match_ranking_is_descending() {
        let mut matches = vec![
            SlotMatch {
                key: "a".into(),
                score: 0.2,
            },
            SlotMatch {
                key: "b".into(),
                score: 0.9,
            },
            SlotMatch {
                key: "c".into(),
                score: 0.5,
            },
        ];
        matches.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        assert_eq!(matches[0].key, "b");
        assert_eq!(matches[2].key, "a");
    }
}
