//! Recursive-folder classification.
//!
//! Given a set of "slots" (each is a name + short description such as
//! "intro = introductory chapter that motivates the work"), embed both the
//! slot descriptions and every markdown chapter under a project prefix, then
//! rank slots per chapter by cosine similarity.
//!
//! For the MVP we use one whole-document embedding per chapter (chunk_idx
//! = 0 from the `embeddings` table) and embed slot descriptions on-the-fly.

use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use agentic_core::{embeddings, worktree};
use agentic_providers::{
    EmbeddingRequest, Provider, ProviderKind, Task, registry, router, traits::ProviderError,
};

use crate::embed::embed_project_blobs;

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
