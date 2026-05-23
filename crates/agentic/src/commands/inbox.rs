//! `agentic inbox` — inbox lifecycle (DB-native port of the Scramblings
//! "meccano"). See the inbox section of QUICKSTART/ARCHITECTURE.

use std::path::Path;

use anyhow::{Context, Result};
use serde_json::json;

use agentic_core::{inbox, journal};

use crate::cli::InboxAction;

pub fn run(db_path: &Path, action: InboxAction, json_out: bool) -> Result<()> {
    let conn = agentic_core::db::open(db_path)?;
    match action {
        InboxAction::Register { project } => {
            let added = inbox::register(&conn, &project)?;
            let total = inbox::list(&conn, &project)?.len();
            if json_out {
                println!("{}", json!({ "registered_new": added, "total": total }));
            } else {
                println!("Registered {added} new inbox item(s); {total} tracked in total.");
            }
        }

        InboxAction::Status { project } => {
            let items = inbox::list(&conn, &project)?;
            let pending = items
                .iter()
                .filter(|i| !matches!(i.state.as_str(), "archived" | "skipped"))
                .count();
            if json_out {
                println!("{}", serde_json::to_string_pretty(&items)?);
            } else {
                println!(
                    "=== inbox: {} item(s); {pending} active, {} retired/skipped ===",
                    items.len(),
                    items.len() - pending
                );
                for i in &items {
                    let dup = i
                        .dup_of
                        .as_deref()
                        .map(|d| format!("  dup-of={d}"))
                        .unwrap_or_default();
                    let place = i.placement.as_deref().unwrap_or("-");
                    println!(
                        "  [{:<9}] {:<45} place={:<14} by={}{}",
                        i.state,
                        i.path,
                        place,
                        i.accepted_by.as_deref().unwrap_or("-"),
                        dup
                    );
                }
                println!(
                    "{}",
                    if pending == 0 {
                        "inbox clear: every item processed (empty-inbox = done)."
                    } else {
                        "inbox has active items still to process."
                    }
                );
            }
        }

        InboxAction::Accept {
            project,
            path,
            score,
            placement,
            justification,
            hitl,
        } => {
            inbox::accept(
                &conn,
                &project,
                &path,
                score,
                placement.as_deref(),
                justification.as_deref(),
                hitl,
            )?;
            if !json_out {
                println!(
                    "Accepted {path} ({}).",
                    if hitl { "HITL" } else { "autonomous" }
                );
            }
        }

        InboxAction::Skip { project, path } => {
            inbox::skip(&conn, &project, &path)?;
            if !json_out {
                println!("Skipped {path}.");
            }
        }

        InboxAction::Retire {
            project,
            path,
            root,
        } => {
            // Core checks the precondition (blob present + accepted/justified/skipped)
            // and marks the item archived; we then delete the disk copy + journal it.
            let sha = inbox::retire(&conn, &project, &path)?;
            let disk = root.join(path.replace('/', std::path::MAIN_SEPARATOR_STR));
            let removed = if disk.exists() {
                std::fs::remove_file(&disk)
                    .with_context(|| format!("removing {}", disk.display()))?;
                true
            } else {
                false
            };
            journal::append(
                &conn,
                &project,
                &journal::NewEntry {
                    actor: "inbox",
                    action_type: "inbox_archive",
                    description: &format!(
                        "Retired inbox item {path} (state=archived; disk copy {}; blob {} kept as permanent archive)",
                        if removed { "removed" } else { "already absent" },
                        &sha[..sha.len().min(12)]
                    ),
                    files_affected: Some(vec![path.clone()]),
                    ..Default::default()
                },
            )?;
            if json_out {
                println!(
                    "{}",
                    json!({ "retired": path, "disk_removed": removed, "blob": sha })
                );
            } else {
                println!(
                    "Retired {path}: disk copy {}, content preserved in DB ({}). 'check tree' will show it unmaterialised.",
                    if removed {
                        "removed"
                    } else {
                        "was already absent"
                    },
                    &sha[..sha.len().min(12)]
                );
            }
        }

        InboxAction::Dedup {
            project,
            model,
            threshold,
        } => {
            let items = inbox::list(&conn, &project)?;
            let exact: Vec<_> = items.iter().filter(|i| i.dup_of.is_some()).collect();
            println!("=== exact duplicates (shared SHA-256): {} ===", exact.len());
            for i in &exact {
                println!("  {} == {}", i.path, i.dup_of.as_deref().unwrap_or("?"));
            }
            match model {
                Some(m) => {
                    println!("=== semantic near-duplicates (cosine ≥ {threshold}, model {m}) ===");
                    let mut found = 0;
                    for i in &items {
                        if let Some(sha) = &i.content_sha {
                            if let Some((other, c)) =
                                inbox::nearest_duplicate(&conn, &m, sha, threshold)?
                            {
                                println!(
                                    "  {} ~ {} (cosine {:.3})",
                                    i.path,
                                    &other[..other.len().min(12)],
                                    c
                                );
                                found += 1;
                            }
                        }
                    }
                    if found == 0 {
                        println!("  none (or no embeddings yet — run `agentic embed`)");
                    }
                }
                None => {
                    println!("(pass --model <embed-model> to also check semantic near-duplicates)")
                }
            }
        }

        InboxAction::Process {
            project,
            model,
            accept_threshold,
            near_dup,
            auto_mainline,
        } => {
            let rep = inbox::process(
                &conn,
                &project,
                model.as_deref(),
                accept_threshold,
                near_dup,
                auto_mainline,
                "inbox-pipeline",
            )?;
            if json_out {
                println!("{}", serde_json::to_string_pretty(&rep)?);
            } else {
                println!(
                    "Processed: {} ranked, {} auto-accepted ({} redundant), {} held for HITL ({} lacked embeddings).",
                    rep.ranked,
                    rep.auto_accepted,
                    rep.redundant,
                    rep.held_for_hitl,
                    rep.no_embedding
                );
                if rep.held_for_hitl > 0 {
                    println!(
                        "  -> confirm held items with: agentic inbox accept --path <p> --placement thesis_main --hitl"
                    );
                }
            }
        }
    }
    Ok(())
}
