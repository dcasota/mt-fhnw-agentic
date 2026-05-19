//! Embed a project's working-tree blobs via a provider's embedding API,
//! storing the resulting vectors in the `embeddings` table.
//!
//! One vector per blob (chunk_idx = 0). The model name is recorded so that
//! downstream tools (classifier, similarity search) can pick a consistent
//! basis.

use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use tracing::warn;

use agentic_core::{content::blob, embeddings, worktree};
use agentic_providers::{
    EmbeddingRequest, Provider, ProviderKind, Task, registry, router, traits::ProviderError,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbedOutcome {
    pub path: String,
    pub blob_sha: String,
    pub model: String,
    pub dims: usize,
    pub skipped: bool,
    pub reason: Option<String>,
}

/// Resolve a `(provider, model)` pair from explicit overrides + the router.
fn resolve_target(
    provider_override: Option<&str>,
    model_override: Option<&str>,
) -> Result<(ProviderKind, String)> {
    let kind = match provider_override {
        Some(s) => s
            .parse::<ProviderKind>()
            .map_err(|e| anyhow!("invalid provider {s}: {e}"))?,
        None => router::route(Task::Embed).kind,
    };
    let model = model_override
        .map(str::to_owned)
        .unwrap_or_else(|| router::default_model(kind, Task::Embed).to_owned());
    Ok((kind, model))
}

/// Embed every markdown blob in the project's working tree under `prefix`.
///
/// `skip_existing = true` reuses already-stored embeddings for the same
/// `(blob_sha, model)` pair. Returns one [`EmbedOutcome`] per processed entry.
pub async fn embed_project_blobs(
    conn: &Connection,
    project_id: &str,
    prefix: &str,
    provider_override: Option<&str>,
    model_override: Option<&str>,
    skip_existing: bool,
) -> Result<Vec<EmbedOutcome>> {
    let (kind, model) = resolve_target(provider_override, model_override)?;
    let provider: Arc<dyn Provider> =
        registry::build(kind).map_err(|e| anyhow!("build provider: {e}"))?;

    let entries =
        worktree::list(conn, project_id, prefix).map_err(|e| anyhow!("list working tree: {e}"))?;

    let mut outcomes = Vec::with_capacity(entries.len());
    for (path, sha) in entries {
        if !is_markdown_path(&path) {
            continue;
        }

        // Skip if already embedded for this (blob, model).
        if skip_existing
            && embeddings::get_embedding(conn, &sha, &model, 0)
                .map_err(|e| anyhow!("look up existing embedding: {e}"))?
                .is_some()
        {
            outcomes.push(EmbedOutcome {
                path,
                blob_sha: sha,
                model: model.clone(),
                dims: 0,
                skipped: true,
                reason: Some("already embedded".into()),
            });
            continue;
        }

        let b = blob::get_blob(conn, &sha).map_err(|e| anyhow!("load blob {sha}: {e}"))?;
        let text = String::from_utf8(b.content)
            .with_context(|| format!("blob {sha} is not valid utf-8"))?;

        // One whole-document call. Truncation / chunking is a later concern.
        let req = EmbeddingRequest {
            model: model.clone(),
            texts: vec![text.clone()],
        };
        let resp = match provider.embed(&req).await {
            Ok(r) => r,
            Err(ProviderError::Unimplemented(p)) => {
                warn!(provider = p, "provider has no embedding API; skipping");
                outcomes.push(EmbedOutcome {
                    path,
                    blob_sha: sha,
                    model: model.clone(),
                    dims: 0,
                    skipped: true,
                    reason: Some(format!("provider {p} has no embed API")),
                });
                continue;
            }
            Err(e) => return Err(anyhow!("embed call: {e}")),
        };

        let vec0 = resp
            .vectors
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("provider returned 0 vectors"))?;
        let dims = vec0.len();
        embeddings::put_embedding(conn, &sha, &model, 0, &text, &vec0)
            .map_err(|e| anyhow!("persist embedding: {e}"))?;
        outcomes.push(EmbedOutcome {
            path,
            blob_sha: sha,
            model: model.clone(),
            dims,
            skipped: false,
            reason: None,
        });
    }
    Ok(outcomes)
}

fn is_markdown_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".md") || lower.ends_with(".markdown")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_target_honours_explicit_overrides() {
        let (kind, model) = resolve_target(Some("openai"), Some("text-embedding-3-small")).unwrap();
        assert_eq!(kind, ProviderKind::OpenAi);
        assert_eq!(model, "text-embedding-3-small");
    }

    #[test]
    fn resolve_target_falls_back_to_router() {
        let (_kind, model) = resolve_target(None, None).unwrap();
        // The router picks an embed-capable provider; the default model is non-empty.
        assert!(!model.is_empty());
    }

    #[test]
    fn resolve_target_rejects_unknown_provider() {
        let err = resolve_target(Some("imaginary"), None).unwrap_err();
        assert!(err.to_string().contains("invalid provider"));
    }
}
