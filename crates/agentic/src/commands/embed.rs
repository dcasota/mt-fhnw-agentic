//! `agentic embed <project>` and `agentic classify <project>` handlers.

use std::path::Path;
use std::str::FromStr;

use anyhow::Result;

use agentic_import::{
    ChapterAssignment, EmbedOutcome, Slot, Strategy, auto_strategy, classify_project_with_strategy,
    default_slots, embed_project_blobs,
};

pub async fn run_embed(
    db_path: &Path,
    project: &str,
    prefix: &str,
    provider: Option<&str>,
    model: Option<&str>,
    force: bool,
    json: bool,
) -> Result<()> {
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
