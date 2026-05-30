//! `agentic embed <project>` and `agentic classify <project>` handlers.

use std::path::Path;
use std::str::FromStr;

use anyhow::Result;

use agentic_import::{
    ChapterAssignment, EmbedOutcome, Slot, Strategy, auto_strategy, classify_project_with_strategy,
    default_slots, embed_project_blobs,
};
use agentic_providers::{Task, router};

pub async fn run_embed(
    db_path: &Path,
    project: &str,
    prefix: &str,
    provider: Option<&str>,
    model: Option<&str>,
    force: bool,
    json: bool,
) -> Result<()> {
    // ADR-0051 §3.3 — graceful SKIP when no provider key is available.
    // If the caller didn't pin a specific provider via `--provider` AND
    // the available-key scan finds no embed-capable provider with a
    // configured key, exit 0 with a SKIPPED marker (cascade gate then
    // displays OK instead of FAIL). Distinguishes "intentionally
    // running keyless" from "configured key + actual error".
    if provider.is_none() && router::available_provider_for(Task::Embed).is_none() {
        if json {
            println!(
                "{}",
                serde_json::json!({
                    "status": "skipped",
                    "reason": "no embed-capable provider key in env or OS keychain (ADR-0051 §3.3)"
                })
            );
        } else {
            println!(
                "SKIPPED: no embed-capable provider key configured (ADR-0051 §3.3). \
                 Set one of GOOGLE_API_KEY / GEMINI_API_KEY (Google), OPENAI_API_KEY, \
                 VOYAGE_API_KEY, MISTRAL_API_KEY, or COHERE_API_KEY to enable the \
                 inbox embed gate; otherwise the gate is informational and does not \
                 block the cascade."
            );
        }
        return Ok(());
    }
    let conn = agentic_core::db::open(db_path)?;
    let outcomes = embed_project_blobs(&conn, project, prefix, provider, model, !force).await?;
    report_embed(&outcomes, json)
}

fn report_embed(items: &[EmbedOutcome], json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(items)?);
        return Ok(());
    }
    if items.is_empty() {
        println!("(no markdown blobs found in working tree)");
        return Ok(());
    }
    let mut embedded = 0;
    let mut skipped = 0;
    for o in items {
        if o.skipped {
            skipped += 1;
            println!(
                "  - {:<32} {}  ({})",
                o.path,
                "skipped",
                o.reason.as_deref().unwrap_or("?")
            );
        } else {
            embedded += 1;
            println!("  + {:<32} dims={} model={}", o.path, o.dims, o.model);
        }
    }
    println!("\n{embedded} embedded, {skipped} skipped.");
    Ok(())
}

pub async fn run_classify(
    db_path: &Path,
    project: &str,
    prefix: &str,
    slot_keys: Option<&str>,
    strategy_arg: Option<&str>,
    provider: Option<&str>,
    model: Option<&str>,
    json: bool,
) -> Result<()> {
    // ADR-0051 §3.3 — graceful SKIP when no provider key is available
    // for either Embed or Chat. Mirrors the embed-gate behaviour above:
    // classify can use either strategy, so we only skip when BOTH lanes
    // have no available key.
    if provider.is_none() && strategy_arg.is_none() {
        let no_embed = router::available_provider_for(Task::Embed).is_none();
        let no_chat = router::available_provider_for(Task::Chat).is_none();
        if no_embed && no_chat {
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "status": "skipped",
                        "reason": "no provider key for either Embed or Chat (ADR-0051 §3.3)"
                    })
                );
            } else {
                println!(
                    "SKIPPED: no provider key configured for either Embed or Chat \
                     (ADR-0051 §3.3). Set any of the supported vendor env vars \
                     (ANTHROPIC_API_KEY, GEMINI_API_KEY / GOOGLE_API_KEY, \
                     OPENAI_API_KEY, XAI_API_KEY, etc.) to enable inbox \
                     classification; otherwise the gate is informational."
                );
            }
            return Ok(());
        }
    }
    let conn = agentic_core::db::open(db_path)?;
    let slots = match slot_keys {
        None => default_slots(),
        Some(s) => slots_from_csv(s),
    };
    let strategy = match strategy_arg {
        Some(s) => Strategy::from_str(s)?,
        None => auto_strategy()?,
    };
    if !json {
        eprintln!("strategy: {strategy:?}");
    }
    let assignments =
        classify_project_with_strategy(&conn, project, prefix, &slots, provider, model, strategy)
            .await?;
    report_classify(&assignments, json, strategy)
}

fn slots_from_csv(s: &str) -> Vec<Slot> {
    let defaults = default_slots();
    s.split(',')
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .map(|k| {
            defaults
                .iter()
                .find(|d| d.key == k)
                .cloned()
                .unwrap_or_else(|| Slot::new(k, format!("Section about {k}.")))
        })
        .collect()
}

fn report_classify(items: &[ChapterAssignment], json: bool, strategy: Strategy) -> Result<()> {
    if json {
        let payload = serde_json::json!({
            "strategy": strategy,
            "assignments": items,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }
    if items.is_empty() {
        match strategy {
            Strategy::Embed => {
                println!("(no embedded chapters found — run `agentic embed <project>` first)")
            }
            Strategy::Chat => {
                println!("(no markdown chapters found under prefix)")
            }
        }
        return Ok(());
    }
    for item in items {
        println!("{}", item.path);
        for (i, m) in item.ranked.iter().enumerate().take(3) {
            let marker = if i == 0 { "→" } else { " " };
            println!("    {marker} {:<14} {:.3}", m.key, m.score);
        }
    }
    Ok(())
}
