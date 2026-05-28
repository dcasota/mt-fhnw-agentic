//! `agentic synthesize cross-stream` — perception-improvement P-3.
//!
//! Asks a configured LLM to propose draft cross-stream findings from the
//! Critical+High dimension findings and the column-max campaign findings the
//! runtime already accepted (latest model_review=accept). Writes the proposal
//! to `out/sources/synthesis/candidates_<ts>.md`; never auto-promoted to a
//! canonical deliverable. The operator reviews, edits, and merges manually.
//!
//! `--dry-run` skips the LLM call and only prints the prompt + a skeleton.

use std::io::Write;
use std::path::Path;

use anyhow::{Result, anyhow};
use serde_json::Value;

use agentic_core::passport::{self, Section};
use agentic_providers::ProviderKind;
use agentic_providers::registry;
use agentic_providers::traits::{ChatMessage, ChatRequest, Role};

use crate::cli::SynthesizeAction;

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

/// Collect the `accept`-tier model_review findings as `(path, rationale)`
/// tuples, latest-wins per path. Bounded to `limit` (0 = unlimited).
pub fn collect_accepted_findings(
    entries: &[passport::Entry],
    limit: usize,
) -> Vec<(String, String)> {
    use std::collections::HashMap;
    let mut latest: HashMap<String, (i64, String, String)> = HashMap::new();
    for e in entries {
        let Ok(v) = serde_json::from_str::<Value>(&e.payload_json) else {
            continue;
        };
        if v.get("kind").and_then(Value::as_str) != Some("model_review") {
            continue;
        }
        let Some(path) = v.get("path").and_then(Value::as_str) else {
            continue;
        };
        let assessment = v.get("assessment").and_then(Value::as_str).unwrap_or("");
        if assessment != "accept" {
            continue;
        }
        let rationale = v
            .get("rationale")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let cur = latest.get(path).map(|(id, _, _)| *id).unwrap_or(0);
        if e.id > cur {
            latest.insert(path.to_string(), (e.id, assessment.to_string(), rationale));
        }
    }
    let mut out: Vec<(String, String)> = latest.into_iter().map(|(p, (_, _, r))| (p, r)).collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    if limit > 0 {
        out.truncate(limit);
    }
    out
}

/// Build the prompt body. Pure; deterministic given inputs.
#[must_use]
pub fn build_prompt(findings: &[(String, String)]) -> String {
    let mut bullets = String::new();
    for (p, r) in findings {
        let r_short: String = r.chars().take(280).collect();
        bullets.push_str(&format!("- `{p}` — {r_short}\n"));
    }
    format!(
        "You are synthesising cross-stream findings for an FHNW master thesis on \
         governance of self-evolving autonomous software. Below is a list of \
         deliverable paths the runtime has already accepted at the Critical or \
         High tier per ADR-0046, each with the model's one-sentence rationale. \
         Propose 3-7 novel cross-stream findings — synergies that hold \
         simultaneously across two or more of the listed deliverables and that \
         no single deliverable already states. For each finding give: a one-line \
         claim, the deliverables it crosses (cite their paths), and a one-sentence \
         load-bearing rationale. Reply in markdown, one `## Finding N` heading \
         per finding. Do not invent facts; only synthesise what the rationales \
         already imply.\n\n## Accepted findings\n\n{bullets}"
    )
}

pub async fn run(db_path: &Path, action: SynthesizeAction, json_out: bool) -> Result<()> {
    let SynthesizeAction::CrossStream {
        project,
        provider,
        model,
        limit,
        dry_run,
        to,
    } = action;
    let conn = agentic_core::db::open(db_path)?;
    let entries = passport::current(&conn, &project, Section::ClaimAuditResults)?;
    let findings = collect_accepted_findings(&entries, limit);
    if findings.is_empty() {
        anyhow::bail!(
            "no model_review=accept entries in the passport (run `agentic review run` first)"
        );
    }
    let prompt = build_prompt(&findings);

    if dry_run {
        let body = format!(
            "# Cross-stream synthesis — dry-run\n\n## Inputs ({} accepted finding(s), limit={limit})\n\n## Prompt (would send to LLM)\n\n```\n{prompt}\n```\n\n## Candidate skeleton\n\n(an LLM response would land here)\n",
            findings.len()
        );
        if let Some(p) = to.as_ref() {
            std::fs::write(p, body.as_bytes())?;
            println!("Wrote dry-run preview to {}", p.display());
        } else {
            std::io::stdout().write_all(body.as_bytes())?;
        }
        return Ok(());
    }

    // Resolve provider.
    let kind = match &provider {
        Some(p) => parse_kind(p).ok_or_else(|| anyhow!("unknown provider '{p}'"))?,
        None => CLOUD
            .iter()
            .copied()
            .find(|k| registry::has_key(*k))
            .ok_or_else(|| {
                anyhow!("no chat provider configured (set e.g. XAI_API_KEY or use --dry-run)")
            })?,
    };
    let model = model.unwrap_or_else(|| match kind {
        ProviderKind::Grok => "grok-4.3".to_string(),
        _ => "default".to_string(),
    });
    let prov = registry::build(kind).map_err(|e| anyhow!("provider build failed: {e}"))?;
    let req = ChatRequest {
        model: model.clone(),
        messages: vec![ChatMessage {
            role: Role::User,
            content: prompt.clone(),
        }],
        temperature: Some(0.3),
        max_tokens: Some(2000),
        seed: None,
        system: Some(
            "You synthesise cross-stream findings from explicit rationales only. \
             Do not fabricate. If no genuine synergy exists, say so."
                .into(),
        ),
    };
    let reply = prov
        .chat(&req)
        .await
        .map_err(|e| anyhow!("provider chat: {e}"))?;
    let ts = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let candidate_path = match to {
        Some(p) => p,
        None => {
            let base = format!("out/sources/synthesis/candidates_{ts}.md");
            std::path::PathBuf::from(base)
        }
    };
    let body = format!(
        "# Cross-stream synthesis candidate ({ts})\n\n> Generated by `agentic synthesize cross-stream` on {} model `{}`. \
         These are DRAFT cross-stream findings proposed by a configured LLM from the \
         runtime's current accept-tier model_review set. NOT auto-promoted; the operator \
         reviews, edits, and (if accepted) hand-merges into the student-notes companion.\n\n## Inputs ({} accepted finding(s))\n\n## LLM proposal\n\n{}\n",
        match kind {
            ProviderKind::Grok => "xAI Grok",
            ProviderKind::Anthropic => "Anthropic Claude",
            ProviderKind::OpenAi => "OpenAI",
            ProviderKind::Google => "Google",
            ProviderKind::Mistral => "Mistral",
            ProviderKind::Cohere => "Cohere",
            ProviderKind::Ollama => "Ollama",
            _ => "configured",
        },
        model,
        findings.len(),
        reply.content
    );
    std::fs::write(&candidate_path, body.as_bytes())?;
    if json_out {
        println!(
            "{}",
            serde_json::json!({
                "candidate_path": candidate_path.display().to_string(),
                "inputs": findings.len(),
                "model": model,
                "ts": ts,
            })
        );
    } else {
        println!(
            "Wrote synthesis candidate to {} ({} inputs)",
            candidate_path.display(),
            findings.len()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: i64, path: &str, assessment: &str, rationale: &str) -> passport::Entry {
        passport::Entry {
            id,
            project_id: "T".into(),
            section: "claim_audit_results".into(),
            payload_json: format!(
                r#"{{"kind":"model_review","path":"{path}","assessment":"{assessment}","rationale":"{rationale}"}}"#
            ),
            added_at: "now".into(),
            commit_sha: None,
            replaces: None,
        }
    }

    #[test]
    fn collect_only_latest_accept_per_path() {
        let entries = vec![
            entry(1, "a.md", "revise", "first take"),
            entry(2, "a.md", "accept", "second take"),
            entry(3, "b.md", "exclude", "skip"),
            entry(4, "c.md", "accept", "fresh"),
        ];
        let got = collect_accepted_findings(&entries, 0);
        assert_eq!(got.len(), 2);
        // Sorted by path.
        assert_eq!(got[0].0, "a.md");
        assert_eq!(got[0].1, "second take");
        assert_eq!(got[1].0, "c.md");
        // b.md (latest exclude) is correctly excluded.
    }

    #[test]
    fn build_prompt_includes_all_findings_in_order() {
        let f = vec![
            ("a.md".into(), "rationale-a".into()),
            ("z.md".into(), "rationale-z".into()),
        ];
        let p = build_prompt(&f);
        assert!(p.contains("a.md"));
        assert!(p.contains("rationale-a"));
        assert!(p.contains("z.md"));
        assert!(p.contains("Propose 3-7 novel cross-stream findings"));
    }

    #[test]
    fn limit_truncates() {
        let entries: Vec<_> = (1..=10)
            .map(|i| entry(i, &format!("{i}.md"), "accept", "r"))
            .collect();
        let got = collect_accepted_findings(&entries, 3);
        assert_eq!(got.len(), 3);
    }
}
