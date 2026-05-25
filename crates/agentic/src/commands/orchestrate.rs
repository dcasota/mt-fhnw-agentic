//! `agentic orchestrate` — bounded autonomous sub-session orchestration.
//!
//! In-tool, monolithic replacement for the external PowerShell launcher
//! (`launch_session.ps1` + `run_*_all.ps1` + `sessions/manifest.json`). Each
//! sub-session shells out to the `claude` CLI with a fixed budget and a guard
//! system-prompt; lifecycle state lives in `orchestration_sessions` (the DB is
//! the source of truth), transcripts are tee'd to `sessions/<id>.jsonl` on disk.
//!
//! The pure logic (insert/list/status, 429 detection, reset parsing, arg
//! construction) lives in `agentic_core::orchestrate`; this module owns the I/O
//! and the process spawning so the core stays unit-testable.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcCommand, Stdio};

use anyhow::{Context, Result};
use serde_json::json;

use agentic_core::orchestrate as orch;
use agentic_core::orchestrate::Status;

use crate::cli::OrchestrateAction;

pub fn run(db_path: &Path, action: OrchestrateAction, json_out: bool) -> Result<()> {
    match action {
        OrchestrateAction::Add {
            project,
            id,
            task,
            budget,
            model,
        } => {
            let conn = agentic_core::db::open(db_path)?;
            let stored_id = orch::make_session_id(&orch::now_id_prefix(), &id);
            orch::add(&conn, &stored_id, &project, &task, budget, model.as_deref())?;
            if json_out {
                println!("{}", json!({ "id": stored_id, "status": "pending" }));
            } else {
                println!("{stored_id}");
            }
            Ok(())
        }

        OrchestrateAction::List { project } => {
            let conn = agentic_core::db::open(db_path)?;
            let sessions = orch::list(&conn, &project)?;
            if json_out {
                println!("{}", serde_json::to_string_pretty(&sessions)?);
                return Ok(());
            }
            println!("{} sub-session(s) for {project}:", sessions.len());
            for s in &sessions {
                println!(
                    "  {:<24} {:<12} ${:<5.2} {}",
                    s.id,
                    s.status,
                    s.budget_usd,
                    truncate(&s.task, 60),
                );
            }
            Ok(())
        }

        OrchestrateAction::Run {
            project,
            id,
            max,
            wave,
            root,
        } => run_sessions(db_path, &project, id.as_deref(), max, wave, &root, json_out),

        OrchestrateAction::Close { project, root } => {
            close_cycle(db_path, &project, &root, json_out)
        }
    }
}

/// Execute pending sub-sessions (or one `--id`), spawning the `claude` CLI.
fn run_sessions(
    db_path: &Path,
    project: &str,
    only_id: Option<&str>,
    max: Option<usize>,
    wave: bool,
    root: &str,
    json_out: bool,
) -> Result<()> {
    let conn = agentic_core::db::open(db_path)?;

    let sessions_dir = PathBuf::from("sessions");
    std::fs::create_dir_all(&sessions_dir)
        .with_context(|| format!("creating {}", sessions_dir.display()))?;

    let mut processed = 0usize;
    let cap = max.unwrap_or(usize::MAX);

    loop {
        // Re-query each pass so wave-mode picks up status changes (idempotent:
        // a done session never reappears).
        let queue: Vec<_> = match only_id {
            Some(target) => orch::get(&conn, target)?
                .into_iter()
                .filter(|s| s.status == "pending")
                .collect(),
            None => orch::pending(&conn, project)?,
        };

        if queue.is_empty() {
            break;
        }

        let mut made_progress = false;
        for s in queue {
            if processed >= cap {
                break;
            }

            let transcript = sessions_dir.join(format!("{}.jsonl", s.id));
            orch::set_status(&conn, &s.id, Status::Running)?;
            orch::set_transcript(&conn, &s.id, &transcript.to_string_lossy())?;

            let outcome = spawn_one(&s, root, &transcript)?;
            processed += 1;
            made_progress = true;

            match outcome {
                Outcome::Clean { tail } => {
                    orch::close(&conn, &s.id, Status::Done, Some(&tail))?;
                    if !json_out {
                        println!("[done] {} (budget ${:.2})", s.id, s.budget_usd);
                    }
                }
                Outcome::RateLimited { backoff_secs } => {
                    orch::set_status(&conn, &s.id, Status::Ratelimited)?;
                    if !json_out {
                        println!("[ratelimited] {}", s.id);
                    }
                    if wave {
                        if !json_out {
                            println!("  sleeping {backoff_secs}s, then retrying {}", s.id);
                        }
                        std::thread::sleep(std::time::Duration::from_secs(backoff_secs));
                        // Re-arm the SAME session for another attempt.
                        orch::set_status(&conn, &s.id, Status::Pending)?;
                    }
                }
                Outcome::Failed { tail } => {
                    orch::close(&conn, &s.id, Status::Failed, Some(&tail))?;
                    if !json_out {
                        println!("[failed] {}: {}", s.id, truncate(&tail, 80));
                    }
                }
            }
        }

        // Single-pass unless wave mode; also stop when nothing advanced or capped.
        if !wave || processed >= cap || !made_progress {
            break;
        }
    }

    if json_out {
        let sessions = orch::list(&conn, project)?;
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "processed": processed,
                "sessions": sessions,
            }))?
        );
    } else {
        println!("Processed {processed} sub-session(s).");
    }
    Ok(())
}

enum Outcome {
    Clean { tail: String },
    RateLimited { backoff_secs: u64 },
    Failed { tail: String },
}

/// Spawn `claude` for one session, piping the task to stdin and tee'ing stdout
/// to the transcript. Returns the classified outcome.
fn spawn_one(s: &orch::Session, root: &str, transcript: &Path) -> Result<Outcome> {
    let args = orch::claude_args(root, true, s.model.as_deref(), s.budget_usd);

    let mut child = ProcCommand::new("claude")
        .args(&args)
        // Use the claude.ai subscription OAuth, not an API key.
        .env_remove("ANTHROPIC_API_KEY")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                anyhow::anyhow!(
                    "the `claude` CLI was not found on PATH — install Claude Code and ensure `claude` is runnable"
                )
            } else {
                anyhow::Error::new(e).context("spawning `claude`")
            }
        })?;

    // Pipe the task text to the child's stdin, then close it.
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(s.task.as_bytes())
            .context("writing task to `claude` stdin")?;
    }

    let output = child
        .wait_with_output()
        .context("waiting on `claude` process")?;

    // Tee captured stdout to the transcript on disk.
    std::fs::write(transcript, &output.stdout)
        .with_context(|| format!("writing transcript {}", transcript.display()))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let last_line = stdout
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .to_string();

    let tail = tail_summary(&stdout, &output.stderr);

    if orch::detect_ratelimit(&last_line) {
        let now_min = local_minutes_of_day();
        let backoff =
            orch::parse_reset_backoff(&last_line, now_min).unwrap_or(orch::DEFAULT_BACKOFF_SECS);
        return Ok(Outcome::RateLimited {
            backoff_secs: backoff,
        });
    }

    if output.status.success() && orch::is_clean_result(&last_line) {
        Ok(Outcome::Clean { tail })
    } else {
        Ok(Outcome::Failed { tail })
    }
}

/// "Now" expressed as local minutes since midnight (for reset-time math).
fn local_minutes_of_day() -> u32 {
    use chrono::Timelike;
    let now = chrono::Local::now();
    now.hour() * 60 + now.minute()
}

/// A short tail of the transcript / stderr for the `exit_summary`.
fn tail_summary(stdout: &str, stderr: &[u8]) -> String {
    let last = stdout
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("");
    if last.is_empty() {
        let err = String::from_utf8_lossy(stderr);
        truncate(err.trim(), 300)
    } else {
        truncate(last, 300)
    }
}

/// One close-cycle step: a human label, the `agentic` args (after `--db <db>`),
/// and the `audit_verdicts` checkpoint to read the verdict back from (if any).
struct CloseStep {
    label: &'static str,
    args: Vec<String>,
    checkpoint: Option<&'static str>,
}

/// The fixed sequence of gate + audit steps run by `orchestrate close`.
fn close_steps(project: &str, root: &str) -> Vec<CloseStep> {
    let chk = |sub: &str, cp: Option<&'static str>, extra: &[&str]| {
        let mut args = vec![
            "check".to_string(),
            sub.to_string(),
            "--project".to_string(),
            project.to_string(),
        ];
        args.extend(extra.iter().map(|s| (*s).to_string()));
        (args, cp)
    };
    let aud = |sub: &str| {
        vec![
            "audit".to_string(),
            sub.to_string(),
            "--project".to_string(),
            project.to_string(),
        ]
    };
    let (self_args, self_cp) = chk("self", Some("self"), &[]);
    let (del_args, del_cp) = chk("deliverable", Some("deliverable"), &[]);
    let (cit_args, cit_cp) = chk("citations", Some("citation_tracker"), &[]);
    let (facts_args, facts_cp) = chk("facts-integrity", Some("facts_integrity"), &[]);
    let (bib_args, bib_cp) = chk("bibliography", Some("bibliography"), &[]);
    let (aibom_args, aibom_cp) = chk("aibom", Some("aibom"), &[]);
    let (docs_args, docs_cp) = chk("docs", Some("docs"), &["--root", root]);
    vec![
        CloseStep {
            label: "check self",
            args: self_args,
            checkpoint: self_cp,
        },
        CloseStep {
            label: "check deliverable",
            args: del_args,
            checkpoint: del_cp,
        },
        CloseStep {
            label: "check citations",
            args: cit_args,
            checkpoint: cit_cp,
        },
        CloseStep {
            label: "check facts-integrity",
            args: facts_args,
            checkpoint: facts_cp,
        },
        CloseStep {
            label: "check bibliography",
            args: bib_args,
            checkpoint: bib_cp,
        },
        CloseStep {
            label: "check aibom",
            args: aibom_args,
            checkpoint: aibom_cp,
        },
        CloseStep {
            label: "check docs",
            args: docs_args,
            checkpoint: docs_cp,
        },
        CloseStep {
            label: "audit sign-commits",
            args: aud("sign-commits"),
            checkpoint: None,
        },
        CloseStep {
            label: "audit report",
            args: aud("report"),
            checkpoint: None,
        },
    ]
}

/// Cycle-close: run the in-tool gates + audit in sequence and print verdicts.
///
/// Each step is run by re-invoking THIS executable as a subprocess. That is the
/// robust choice: `check::run` calls `std::process::exit(1)` on a FAIL gate, so
/// calling it in-process would abort the whole close cycle on the first failing
/// gate. As a subprocess, a FAIL surfaces as a non-zero exit code we can record
/// and keep going. The verdict is harvested from `audit_verdicts` (every gate
/// records its verdict there per ADR-0041) and falls back to the exit code.
fn close_cycle(db_path: &Path, project: &str, root: &Path, json_out: bool) -> Result<()> {
    let exe = std::env::current_exe().context("locating the agentic executable")?;
    let db = db_path.to_string_lossy().to_string();
    let root_s = root.to_string_lossy().to_string();
    let steps = close_steps(project, &root_s);

    let mut rows: Vec<(String, String)> = Vec::new();
    for step in steps {
        let mut cmd = ProcCommand::new(&exe);
        cmd.arg("--db").arg(&db).args(&step.args);
        let verdict = match cmd.status() {
            Ok(st) => step.checkpoint.map_or_else(
                || {
                    if st.success() {
                        "OK".into()
                    } else {
                        "FAIL".into()
                    }
                },
                |cp| {
                    // Prefer the recorded gate verdict; fall back to the exit code.
                    latest_verdict(db_path, project, cp).unwrap_or_else(|| {
                        if st.success() {
                            "PASS".into()
                        } else {
                            "FAIL".into()
                        }
                    })
                },
            ),
            Err(e) => format!("ERR: {e}"),
        };
        rows.push((step.label.to_string(), verdict));
    }

    if json_out {
        let obj: serde_json::Map<String, serde_json::Value> =
            rows.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
        println!("{}", serde_json::to_string_pretty(&obj)?);
    } else {
        println!("\n=== orchestrate close — cycle verdicts ===");
        for (label, verdict) in &rows {
            println!("  {label:<24} {verdict}");
        }
    }
    Ok(())
}

/// The verdict value (PASS/WARN/FAIL) of the newest verdict row for a checkpoint.
fn latest_verdict(db_path: &Path, project: &str, checkpoint: &str) -> Option<String> {
    let conn = agentic_core::db::open(db_path).ok()?;
    conn.query_row(
        "SELECT verdict FROM audit_verdicts WHERE project_id = ?1 AND checkpoint = ?2 \
         ORDER BY id DESC LIMIT 1",
        rusqlite::params![project, checkpoint],
        |r| r.get::<_, String>(0),
    )
    .ok()
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}
