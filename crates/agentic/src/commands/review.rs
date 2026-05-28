//! `agentic review run` — LLM document/ranking review (ADR-0049).
//!
//! A second model (e.g. xAI Grok) sequentially reviews each deliverable
//! document and the current rankings, recording a structured verdict
//! `{assessment, score, issues, ranking_feedback, rationale}` as a signed
//! `claim_audit_results` entry (kind=model_review). Those entries are the
//! ranking/adoption store the cascade consults, so the reviews feed adoption.
//! No fabrication: a provider/parse failure records an explicit `unknown`
//! verdict with the raw response, never a synthetic "accept" (ADR-0036).

use anyhow::{Result, anyhow};
use serde_json::{Value, json};

use agentic_core::passport::{self, Section};
use agentic_core::worktree;
use agentic_providers::ProviderKind;
use agentic_providers::registry;
use agentic_providers::traits::{ChatMessage, ChatRequest, Role};

use crate::cli::ReviewAction;

const CLOUD: &[ProviderKind] = &[
    ProviderKind::Anthropic,
    ProviderKind::OpenAi,
    ProviderKind::Google,
    ProviderKind::Mistral,
    ProviderKind::Cohere,
    ProviderKind::Grok,
];

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

/// Infer the deliverable class from a content-store path (case-insensitive).
fn class_of(path: &str) -> &'static str {
    let pl = path.to_lowercase();
    let base = pl.rsplit('/').next().unwrap_or(&pl);
    if base.starts_with("dimension_") {
        "dimension"
    } else if base.starts_with("campaign_") {
        "campaign"
    } else if base.starts_with("pt-") {
        "project"
    } else if base.starts_with("studentnotes") {
        "student_notes"
    } else if pl.contains("/norms/") || base.starts_with("norms") {
        "norms"
    } else if pl.starts_with("thesis/") || base.contains("master_thesis") {
        "master_thesis"
    } else if base.contains("aibom") || base.contains("audit_bom") {
        "aibom"
    } else if base.contains("solution") {
        "solution"
    } else if base.contains("tool") {
        "tool"
    } else {
        "deliverable"
    }
}

/// Extract the first balanced `{...}` JSON object from a possibly-prose reply.
fn extract_json(s: &str) -> Option<Value> {
    let start = s.find('{')?;
    let end = s.rfind('}')?;
    if end <= start {
        return None;
    }
    serde_json::from_str(&s[start..=end]).ok()
}

pub async fn run(db_path: &std::path::Path, action: ReviewAction, json_out: bool) -> Result<()> {
    let ReviewAction::Run {
        project,
        prefix,
        provider,
        model,
        limit,
        force,
    } = action;
    let conn = agentic_core::db::open(db_path)?;

    // Pick a configured cloud provider (explicit or first available).
    let kind = match &provider {
        Some(p) => parse_kind(p).ok_or_else(|| anyhow!("unknown provider '{p}'"))?,
        None => CLOUD
            .iter()
            .copied()
            .find(|k| registry::has_key(*k))
            .ok_or_else(|| anyhow!("no chat provider configured (set e.g. XAI_API_KEY)"))?,
    };
    let model = model.unwrap_or_else(|| match kind {
        ProviderKind::Grok => "grok-4.3".to_string(),
        _ => "default".to_string(),
    });
    let prov = registry::build(kind).map_err(|e| anyhow!("provider build failed: {e}"))?;

    // Enumerate deliverable documents (skip the derived merged compendium).
    let mut docs: Vec<(String, String)> = worktree::list(&conn, &project, &prefix)?
        .into_iter()
        .filter(|(p, _)| p.ends_with(".md") && p != agentic_core::paths::MERGED_DOC)
        .collect();
    docs.sort_by(|a, b| a.0.cmp(&b.0));
    if limit > 0 {
        docs.truncate(limit);
    }
    if docs.is_empty() {
        return Err(anyhow!("no .md deliverables under {prefix}"));
    }

    let head = worktree::head_commit(&conn, &project)?.map(|c| c.sha256);
    let reviewed_at = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let mut tally: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();

    // Build a (path -> latest (id, blob_sha)) map from prior model_reviews so
    // each new verdict SUPERSEDES the previous one for the same path AND we can
    // skip an unchanged doc (input-delta gating) — latest-wins on adoption, no
    // wasted LLM call when nothing changed.
    let prior_entries = passport::current(&conn, &project, Section::ClaimAuditResults)?;
    let mut prior_path: std::collections::HashMap<String, (i64, String)> =
        std::collections::HashMap::new();
    let mut prior_rankings: Option<i64> = None;
    for e in &prior_entries {
        let Ok(v) = serde_json::from_str::<Value>(&e.payload_json) else {
            continue;
        };
        if v.get("kind").and_then(Value::as_str) != Some("model_review") {
            continue;
        }
        if v.get("scope").and_then(Value::as_str) == Some("rankings") {
            // Latest-id wins for the rankings review too.
            if prior_rankings.is_none_or(|id| id < e.id) {
                prior_rankings = Some(e.id);
            }
        } else if let Some(p) = v.get("path").and_then(Value::as_str) {
            let sha = v
                .get("blob_sha")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let cur = prior_path.get(p).map(|(id, _)| *id).unwrap_or(0);
            if e.id > cur {
                prior_path.insert(p.to_string(), (e.id, sha));
            }
        }
    }
    let mut unchanged = 0usize;

    for (path, sha) in &docs {
        let class = class_of(path);
        // Input-delta gating: skip docs whose content blob hasn't changed since
        // the last review. --force re-reviews everything regardless.
        if !force {
            if let Some((_, prior_sha)) = prior_path.get(path) {
                if prior_sha == sha {
                    unchanged += 1;
                    if !json_out {
                        println!("  · [{class}] {path} → unchanged (skipped)");
                    }
                    continue;
                }
            }
        }
        let blob = worktree::read_at(&conn, &project, path)?;
        let text = String::from_utf8_lossy(&blob.content);
        // Bound the prompt: review the leading content (cost/token control).
        let excerpt: String = text.chars().take(8000).collect();
        let req = ChatRequest {
            model: model.clone(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: format!(
                    "Review this master-thesis deliverable ({class}: {path}). Judge fitness for \
                     the thesis mainline and whether its ranking/adoption is justified. Reply with \
                     ONLY a JSON object, no prose:\n\
                     {{\"assessment\":\"accept|revise|exclude\",\"score\":0-100,\
                     \"issues\":[\"short issue\"],\"ranking_feedback\":\"one sentence\",\
                     \"rationale\":\"one sentence\"}}\n\n---\n{excerpt}"
                ),
            }],
            temperature: Some(0.0),
            max_tokens: Some(500),
            seed: None,
            system: Some(
                "You are a rigorous, independent master-thesis reviewer. Do not fabricate; \
                 if unsure, say so. Output strictly the requested JSON."
                    .into(),
            ),
        };
        let (assessment, payload) = match prov.chat(&req).await {
            Ok(resp) => {
                let parsed = extract_json(&resp.content);
                let assessment = parsed
                    .as_ref()
                    .and_then(|v| v.get("assessment").and_then(Value::as_str))
                    .unwrap_or("unknown")
                    .to_string();
                let mut p = json!({
                    "kind": "model_review", "class": class, "path": path, "blob_sha": sha,
                    "provider": kind.as_str(), "model": resp.model, "assessment": assessment,
                    "reviewed_at": reviewed_at,
                });
                if let Some(v) = parsed {
                    for f in ["score", "issues", "ranking_feedback", "rationale"] {
                        if let Some(val) = v.get(f) {
                            p[f] = val.clone();
                        }
                    }
                } else {
                    // No parseable JSON — keep the raw reply, mark unknown (no fabrication).
                    p["raw"] = json!(resp.content.chars().take(400).collect::<String>());
                }
                (assessment, p)
            }
            Err(e) => (
                "unknown".to_string(),
                json!({
                    "kind": "model_review", "class": class, "path": path, "blob_sha": sha,
                    "provider": kind.as_str(), "model": model, "assessment": "unknown",
                    "error": e.to_string(), "reviewed_at": reviewed_at,
                }),
            ),
        };
        passport::append(
            &conn,
            &project,
            Section::ClaimAuditResults,
            &payload.to_string(),
            head.as_deref(),
            prior_path.get(path).map(|(id, _)| *id),
        )?;
        *tally.entry(assessment.clone()).or_default() += 1;
        if !json_out {
            println!("  + [{class}] {path} → {assessment}");
        }
    }

    // If nothing changed (no per-doc reviews actually ran), the rankings are
    // the same as last time — skip the rankings-scope LLM call too.
    let reviewed_now = tally.values().sum::<usize>();
    if reviewed_now == 0 && !force {
        if !json_out {
            println!(
                "No documents changed since the last review ({unchanged} unchanged) — \
                 rankings review skipped. Use --force to re-review."
            );
        } else {
            println!(
                "{}",
                json!({"reviewed": 0, "unchanged": unchanged, "provider": kind.as_str(), "model": model})
            );
        }
        return Ok(());
    }

    // Review the rankings as a whole: feed the per-document assessments back to
    // the model for an overall adoption opinion.
    let summary: Vec<String> = tally.iter().map(|(k, n)| format!("{n} {k}")).collect();
    let rank_req = ChatRequest {
        model: model.clone(),
        messages: vec![ChatMessage {
            role: Role::User,
            content: format!(
                "Across {} reviewed thesis deliverables the per-document verdicts are: {}. \
                 Assess whether the overall ranking/adoption (what reaches the thesis mainline vs. \
                 is held) is justified. Reply with ONLY JSON: \
                 {{\"assessment\":\"accept|revise|exclude\",\"score\":0-100,\
                 \"ranking_feedback\":\"one sentence\",\"rationale\":\"one sentence\"}}",
                docs.len(),
                summary.join(", ")
            ),
        }],
        temperature: Some(0.0),
        max_tokens: Some(400),
        seed: None,
        system: Some("You are a rigorous, independent reviewer of thesis rankings.".into()),
    };
    if let Ok(resp) = prov.chat(&rank_req).await {
        let parsed = extract_json(&resp.content);
        let mut p = json!({
            "kind": "model_review", "scope": "rankings", "provider": kind.as_str(),
            "model": resp.model, "reviewed_at": reviewed_at,
            "reviewed_docs": docs.len(),
        });
        match parsed {
            Some(v) => {
                for f in ["assessment", "score", "ranking_feedback", "rationale"] {
                    if let Some(val) = v.get(f) {
                        p[f] = val.clone();
                    }
                }
            }
            None => p["raw"] = json!(resp.content.chars().take(400).collect::<String>()),
        }
        passport::append(
            &conn,
            &project,
            Section::ClaimAuditResults,
            &p.to_string(),
            head.as_deref(),
            prior_rankings,
        )?;
    }

    if json_out {
        println!(
            "{}",
            json!({"reviewed": reviewed_now, "unchanged": unchanged, "total": docs.len(),
                   "provider": kind.as_str(), "model": model, "verdicts": tally})
        );
    } else {
        let unchanged_note = if unchanged > 0 {
            format!(" ({unchanged} unchanged, skipped)")
        } else {
            String::new()
        };
        println!(
            "Reviewed {reviewed_now} deliverable(s) + rankings via {} ({model}): {}{unchanged_note}",
            kind.as_str(),
            summary.join(", ")
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn class_inference_is_case_insensitive() {
        assert_eq!(
            class_of("out/sources/Dimension_06_quantum_EN.md"),
            "dimension"
        );
        assert_eq!(class_of("out/sources/Campaign_01_cve_EN.md"), "campaign");
        assert_eq!(
            class_of("out/sources/projects/PT-C02-1_rbac_EN.md"),
            "project"
        );
        assert_eq!(
            class_of("out/sources/StudentNotes_Synthesis_EN.md"),
            "student_notes"
        );
        assert_eq!(class_of("out/sources/norms/06_norms_EN.md"), "norms");
        assert_eq!(class_of("out/sources/AI_Audit_BOM_EN.md"), "aibom");
        assert_eq!(class_of("thesis/01_introduction.md"), "master_thesis");
        assert_eq!(class_of("out/sources/Briefing_Doc2_EN.md"), "deliverable");
    }

    #[test]
    fn extract_json_handles_prose_and_fences() {
        let v = extract_json("Sure! ```json\n{\"assessment\":\"accept\",\"score\":80}\n``` done")
            .unwrap();
        assert_eq!(v["assessment"], "accept");
        assert_eq!(v["score"], 80);
        assert!(extract_json("no json here").is_none());
    }
}
