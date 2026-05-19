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

/// Pick the default strategy based on which providers are configured.
///
/// * If any embed-capable provider has a key → [`Strategy::Embed`].
/// * Else if any chat-capable provider has a key → [`Strategy::Chat`].
/// * Else returns an error so the caller can surface "no providers configured".
pub fn auto_strategy() -> Result<Strategy> {
    let mut embed_ok = false;
    let mut chat_ok = false;
    for kind in ProviderKind::all() {
        if !registry::has_key(kind) {
            continue;
        }
        if router::supports_task(kind, Task::Embed) {
            embed_ok = true;
        }
        if router::supports_task(kind, Task::Chat) {
            chat_ok = true;
        }
    }
    if embed_ok {
        Ok(Strategy::Embed)
    } else if chat_ok {
        Ok(Strategy::Chat)
    } else {
        Err(anyhow!(
            "no providers configured — set ANTHROPIC_API_KEY (or another vendor env var) and retry"
        ))
    }
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
