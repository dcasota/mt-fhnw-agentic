//! `agentic provider …` — list, smoke-test, route inspection.

use anyhow::{Context, Result, anyhow};
use serde_json::json;

use agentic_providers::{
    ChatMessage, ChatRequest, EmbeddingRequest, ProviderKind, Role, Task, registry, router,
};

use crate::cli::ProviderAction;

pub async fn run(action: ProviderAction, json: bool) -> Result<()> {
    match action {
        ProviderAction::List => list(json),
        ProviderAction::Test { name, model } => test_provider(&name, model.as_deref(), json).await,
        ProviderAction::Route { task } => route(&task, json),
    }
}

fn list(json: bool) -> Result<()> {
    use agentic_providers::keychain;
    #[derive(serde::Serialize)]
    struct Row {
        provider: &'static str,
        configured: bool,
        source: Option<&'static str>,
        agentic_env: String,
        vendor_env: Option<&'static str>,
    }
    let rows: Vec<Row> = ProviderKind::all()
        .iter()
        .map(|k| {
            let p = k.as_str();
            let (configured, source) = if matches!(k, ProviderKind::Ollama) {
                (registry::has_key(*k), Some("no-key (local)"))
            } else {
                match keychain::get_key_with_source(p) {
                    Ok(Some((_, src))) => (true, Some(src.as_str())),
                    _ => (false, None),
                }
            };
            Row {
                provider: p,
                configured,
                source,
                agentic_env: keychain::env_var_name(p),
                vendor_env: keychain::vendor_env_var_name(p),
            }
        })
        .collect();
    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else {
        println!(
            "{:<12} {:<11} {:<14} {:<24} {}",
            "PROVIDER", "STATUS", "SOURCE", "VENDOR_ENV", "AGENTIC_ENV"
        );
        for r in rows {
            let status = if r.configured {
                "configured"
            } else {
                "missing"
            };
            println!(
                "{:<12} {:<11} {:<14} {:<24} {}",
                r.provider,
                status,
                r.source.unwrap_or("-"),
                r.vendor_env.unwrap_or("-"),
                r.agentic_env
            );
        }
    }
    Ok(())
}

async fn test_provider(name: &str, model: Option<&str>, json: bool) -> Result<()> {
    let kind: ProviderKind = name
        .parse()
        .map_err(|e| anyhow!("invalid provider '{name}': {e}"))?;
    let provider = registry::build(kind).context("failed to build provider")?;

    // Voyage has no chat; smoke-test via embed.
    if matches!(kind, ProviderKind::Voyage) {
        let model = model
            .map(str::to_owned)
            .unwrap_or_else(|| router::default_model(kind, Task::Embed).to_string());
        let resp = provider
            .embed(&EmbeddingRequest {
                model: model.clone(),
                texts: vec!["hello".into()],
            })
            .await
            .with_context(|| format!("voyage embed (model={model})"))?;
        emit_embed_result(kind, &resp, json)?;
        return Ok(());
    }

    let model = model
        .map(str::to_owned)
        .unwrap_or_else(|| router::default_model(kind, Task::Chat).to_string());
    let req = ChatRequest {
        model: model.clone(),
        messages: vec![ChatMessage {
            role: Role::User,
            content: "Reply with exactly one word: pong.".into(),
        }],
        temperature: Some(0.0),
        max_tokens: Some(16),
        seed: None,
        system: None,
    };
    let resp = provider
        .chat(&req)
        .await
        .with_context(|| format!("{} chat (model={model})", kind.as_str()))?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "provider": kind.as_str(),
                "model": resp.model,
                "content": resp.content,
                "tokens_in": resp.tokens_in,
                "tokens_out": resp.tokens_out,
                "finish_reason": resp.finish_reason,
            }))?
        );
    } else {
        println!("provider: {}", kind.as_str());
        println!("model:    {}", resp.model);
        println!("content:  {}", resp.content.trim());
        println!(
            "tokens:   in={} out={} reason={}",
            resp.tokens_in, resp.tokens_out, resp.finish_reason
        );
    }
    Ok(())
}

fn emit_embed_result(
    kind: ProviderKind,
    resp: &agentic_providers::EmbeddingResponse,
    json: bool,
) -> Result<()> {
    let dim = resp.vectors.first().map_or(0, Vec::len);
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "provider": kind.as_str(),
                "model": resp.model,
                "vectors": resp.vectors.len(),
                "dim": dim,
                "tokens_in": resp.tokens_in,
            }))?
        );
    } else {
        println!("provider: {}", kind.as_str());
        println!("model:    {}", resp.model);
        println!("vectors:  {} (dim={dim})", resp.vectors.len());
        println!("tokens:   in={}", resp.tokens_in);
    }
    Ok(())
}

fn route(task_name: &str, json: bool) -> Result<()> {
    let task = parse_task(task_name).with_context(|| format!("unknown task '{task_name}'"))?;
    let r = router::route(task);
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "task": task_name,
                "provider": r.kind.as_str(),
                "model": r.model,
                "reason": r.reason,
            }))?
        );
    } else {
        println!("task:     {task_name}");
        println!("provider: {}", r.kind.as_str());
        println!("model:    {}", r.model);
        println!("reason:   {}", r.reason);
    }
    Ok(())
}

fn parse_task(s: &str) -> Result<Task> {
    Ok(match s.to_ascii_lowercase().as_str() {
        "chat" => Task::Chat,
        "judge" => Task::Judge,
        "embed" => Task::Embed,
        "extract" => Task::Extract,
        "classify" => Task::Classify,
        "translate" => Task::Translate,
        _ => return Err(anyhow!("unknown task: {s}")),
    })
}
