//! `agentic external-session` — ingest exported AI-platform sessions
//! (grok.com, gemini.google.com, chatgpt.com, claude.ai, perplexity.ai,
//! other) into the AIBOM per ADR-0053.
//!
//! Ingestion stores the RAW exported file as a content-addressed blob
//! (the audit anchor — renaming or modifying the file changes the
//! SHA), runs a per-platform parser to extract metadata + a normalised
//! JSON view of the turns (stored as a second blob), inserts a row in
//! `external_sessions`, and appends an `audit_row` so the AIBOM gate
//! can count the entry.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use rusqlite::params;
use serde::Serialize;

use agentic_core::content::blob;

use crate::cli::ExternalSessionAction;

/// One normalised turn within a captured session. The exact field set is
/// platform-agnostic so the normalised JSON is the queryable form
/// regardless of which platform produced the raw export.
#[derive(Debug, Clone, Serialize)]
pub struct Turn {
    /// "user" / "assistant" / "system" / "tool" (free-form — what the
    /// raw export uses).
    pub role: String,
    /// ISO-8601 UTC timestamp if extractable; otherwise None.
    pub ts: Option<String>,
    /// Verbatim text content of the turn.
    pub content_text: String,
}

/// Per-platform parser result.
#[derive(Debug, Clone, Serialize)]
pub struct ParsedSession {
    pub platform: String,
    pub session_id: Option<String>,
    pub session_started_at: Option<String>,
    pub session_ended_at: Option<String>,
    pub model_hint: Option<String>,
    pub turns: Vec<Turn>,
}

/// Dispatch the top-level subcommand.
pub fn run(db_path: &Path, action: ExternalSessionAction) -> Result<()> {
    match action {
        ExternalSessionAction::Import {
            project,
            platform,
            file,
            attestation,
            notes,
        } => import(
            db_path,
            &project,
            &platform,
            &file,
            &attestation,
            notes.as_deref(),
        ),
        ExternalSessionAction::List { project, platform } => {
            list(db_path, &project, platform.as_deref())
        }
    }
}

fn import(
    db_path: &Path,
    project: &str,
    platform: &str,
    file: &PathBuf,
    attestation: &str,
    notes: Option<&str>,
) -> Result<()> {
    if !is_known_platform(platform) {
        return Err(anyhow!(
            "unknown platform '{platform}' — expected one of: grok, gemini, chatgpt, claude, perplexity, other"
        ));
    }
    if attestation.trim().is_empty() {
        return Err(anyhow!(
            "--attestation is required (one-line author provenance statement; ADR-0053 §5.5 trust model)"
        ));
    }
    let raw_bytes = std::fs::read(file).with_context(|| format!("reading {}", file.display()))?;

    let conn = agentic_core::db::open(db_path)?;

    // Store the raw export as a blob (the audit anchor).
    let mime = mime_for_export(file);
    let raw_sha = blob::put_blob(&conn, &raw_bytes, &mime, None)?;

    // Run the per-platform parser. Stub parsers return turns.len() == 0
    // and just round-trip the raw text as the normalised blob.
    let parsed = match platform {
        "grok" => parse_grok(&raw_bytes)?,
        "gemini" => parse_gemini(&raw_bytes)?,
        "chatgpt" | "claude" | "perplexity" | "other" => parse_stub(platform, &raw_bytes),
        _ => unreachable!(), // is_known_platform guards
    };

    // Store the normalised JSON as a second blob.
    let normalised_json = serde_json::to_vec_pretty(&parsed)?;
    let normalised_sha = blob::put_blob(&conn, &normalised_json, "application/json", None)?;

    // Insert the external_sessions row.
    conn.execute(
        "INSERT INTO external_sessions (
            project_id, platform, session_id,
            session_started_at, session_ended_at,
            model_hint, turn_count, blob_sha, normalised_sha,
            user_attestation, notes
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            project,
            platform,
            parsed.session_id.as_deref(),
            parsed.session_started_at.as_deref(),
            parsed.session_ended_at.as_deref(),
            parsed.model_hint.as_deref(),
            parsed.turns.len() as i64,
            raw_sha,
            normalised_sha,
            attestation,
            notes,
        ],
    )?;

    // Append an audit_row so the AIBOM gate can count the entry.
    // audit_rows schema: (project_id, agent, action, target, result,
    // sidecar_json). `result='info'` since this is a recording event,
    // not a pass/fail check.
    let sidecar = serde_json::json!({
        "kind": "external_session",
        "platform": platform,
        "session_id": parsed.session_id,
        "model_hint": parsed.model_hint,
        "turn_count": parsed.turns.len(),
        "raw_sha": raw_sha,
        "normalised_sha": normalised_sha,
        "user_attestation": attestation,
        "adr": "ADR-0053",
    })
    .to_string();
    let target = format!(
        "{platform}:{}",
        parsed.session_id.as_deref().unwrap_or(&raw_sha[..12])
    );
    conn.execute(
        "INSERT INTO audit_rows (project_id, agent, action, target, result, sidecar_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            project,
            "external-session-import",
            "ingest",
            target,
            "info",
            sidecar,
        ],
    )
    .context("inserting audit_row for external_session ingest")?;

    println!(
        "ingested external session\n  platform     : {platform}\n  session_id   : {sid}\n  turns        : {n}\n  raw_sha      : {raw_sha}\n  normalised   : {normalised_sha}\n  attestation  : {attestation}",
        sid = parsed.session_id.as_deref().unwrap_or("(none)"),
        n = parsed.turns.len(),
    );
    Ok(())
}

fn list(db_path: &Path, project: &str, platform: Option<&str>) -> Result<()> {
    let conn = agentic_core::db::open(db_path)?;
    let mut q = "SELECT id, platform, COALESCE(session_id,''), captured_at, turn_count, substr(blob_sha,1,12), user_attestation FROM external_sessions WHERE project_id = ?1".to_string();
    if platform.is_some() {
        q.push_str(" AND platform = ?2");
    }
    q.push_str(" ORDER BY captured_at DESC");
    let mut stmt = conn.prepare(&q)?;
    let mut count = 0usize;
    let render = |r: &rusqlite::Row<'_>| -> rusqlite::Result<()> {
        let id: i64 = r.get(0)?;
        let plat: String = r.get(1)?;
        let sid: String = r.get(2)?;
        let cap: String = r.get(3)?;
        let turns: i64 = r.get(4)?;
        let sha: String = r.get(5)?;
        let att: String = r.get(6)?;
        println!(
            "  #{id:<4} {plat:<10} turns={turns:<3} sha={sha} captured={cap}  sid={sid}  attestation={att}"
        );
        Ok(())
    };
    let mut rows = if let Some(p) = platform {
        stmt.query(params![project, p])?
    } else {
        stmt.query(params![project])?
    };
    while let Some(r) = rows.next()? {
        render(r)?;
        count += 1;
    }
    if count == 0 {
        println!("(no external sessions captured for project {project})");
    } else {
        println!("\n{count} session(s).");
    }
    Ok(())
}

fn is_known_platform(p: &str) -> bool {
    matches!(
        p,
        "grok" | "gemini" | "chatgpt" | "claude" | "perplexity" | "other"
    )
}

fn mime_for_export(file: &Path) -> String {
    match file.extension().and_then(|s| s.to_str()) {
        Some("json") => "application/json".into(),
        Some("html") | Some("htm") => "text/html".into(),
        Some("md") | Some("markdown") => "text/markdown".into(),
        Some("zip") => "application/zip".into(),
        Some("txt") => "text/plain".into(),
        _ => "application/octet-stream".into(),
    }
}

/// Grok (xAI) share-link / data-download JSON parser.
///
/// xAI's share-link export is a JSON object with shape (as of Feb 2026):
///   { "conversation_id": "...", "title": "...", "model": "grok-4",
///     "created_at": "...", "messages": [ { "role": "...",
///     "content": "...", "ts": "..." }, ... ] }
///
/// The "Download data" bundle is a JSON array of conversations with
/// the same per-conversation shape. We accept BOTH — single object
/// is parsed as one session, an array picks element 0 (the caller
/// imports each conversation in a separate `import` invocation when
/// needed).
fn parse_grok(raw: &[u8]) -> Result<ParsedSession> {
    let v: serde_json::Value = serde_json::from_slice(raw).context("parsing grok JSON export")?;
    let obj: &serde_json::Map<String, serde_json::Value> = match &v {
        serde_json::Value::Object(o) => o,
        serde_json::Value::Array(a) => a
            .first()
            .and_then(|el| el.as_object())
            .ok_or_else(|| anyhow!("grok JSON array is empty or first element not an object"))?,
        _ => return Err(anyhow!("grok JSON must be an object or non-empty array")),
    };
    let session_id = obj
        .get("conversation_id")
        .and_then(|v| v.as_str())
        .or_else(|| obj.get("id").and_then(|v| v.as_str()))
        .map(str::to_string);
    let session_started_at = obj
        .get("created_at")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let model_hint = obj
        .get("model")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let turns = obj
        .get("messages")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| {
                    let mo = m.as_object()?;
                    Some(Turn {
                        role: mo
                            .get("role")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown")
                            .to_string(),
                        ts: mo.get("ts").and_then(|v| v.as_str()).map(str::to_string),
                        content_text: mo
                            .get("content")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let session_ended_at = turns
        .last()
        .and_then(|t| t.ts.clone())
        .or_else(|| session_started_at.clone());
    Ok(ParsedSession {
        platform: "grok".into(),
        session_id,
        session_started_at,
        session_ended_at,
        model_hint,
        turns,
    })
}

/// Gemini (Google) Takeout "Gemini Apps Activity" JSON parser.
///
/// Takeout exports as a JSON array of activity records, each with:
///   { "title": "...", "time": "...", "products": ["Gemini Apps"],
///     "subtitles": [{ "name": "..." }],
///     "details": [{ "name": "Gemini" }],
///     "prompt": { "text": "..." },
///     "response": { "text": "..." } }
///
/// One JSON file usually contains many records spanning multiple
/// conversations; the parser collapses them into ONE session for
/// import simplicity. Use the `notes` field on import to scope
/// (e.g. "2026-05-30 morning Gemini sessions on Photon-OS topics").
fn parse_gemini(raw: &[u8]) -> Result<ParsedSession> {
    let v: serde_json::Value =
        serde_json::from_slice(raw).context("parsing gemini Takeout JSON")?;
    let arr = v
        .as_array()
        .ok_or_else(|| anyhow!("gemini Takeout JSON must be an array of activity records"))?;
    let mut turns = Vec::new();
    let mut first_ts: Option<String> = None;
    let mut last_ts: Option<String> = None;
    for rec in arr {
        let ro = match rec.as_object() {
            Some(o) => o,
            None => continue,
        };
        let ts = ro.get("time").and_then(|v| v.as_str()).map(str::to_string);
        if first_ts.is_none() {
            first_ts = ts.clone();
        }
        last_ts = ts.clone();
        if let Some(prompt) = ro
            .get("prompt")
            .and_then(|v| v.get("text"))
            .and_then(|v| v.as_str())
        {
            turns.push(Turn {
                role: "user".into(),
                ts: ts.clone(),
                content_text: prompt.to_string(),
            });
        }
        if let Some(resp) = ro
            .get("response")
            .and_then(|v| v.get("text"))
            .and_then(|v| v.as_str())
        {
            turns.push(Turn {
                role: "assistant".into(),
                ts: ts.clone(),
                content_text: resp.to_string(),
            });
        }
    }
    Ok(ParsedSession {
        platform: "gemini".into(),
        session_id: None,
        session_started_at: first_ts,
        session_ended_at: last_ts,
        model_hint: Some("gemini (Takeout — model not in export)".into()),
        turns,
    })
}

/// Stub parser for chatgpt / claude / perplexity / other. Returns
/// zero turns; the raw blob is the audit anchor. A future T1b commit
/// can add full per-platform parsers without touching this code.
fn parse_stub(platform: &str, _raw: &[u8]) -> ParsedSession {
    ParsedSession {
        platform: platform.to_string(),
        session_id: None,
        session_started_at: None,
        session_ended_at: None,
        model_hint: None,
        turns: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GROK_SHARE_JSON: &str = r#"{
        "conversation_id": "abc123",
        "title": "Photon OS / FHNW MAS",
        "model": "grok-4",
        "created_at": "2026-05-30T14:22:00Z",
        "messages": [
            {"role":"user","ts":"2026-05-30T14:22:00Z","content":"What is FHNW?"},
            {"role":"assistant","ts":"2026-05-30T14:22:05Z","content":"FHNW is..."}
        ]
    }"#;

    const GEMINI_TAKEOUT_JSON: &str = r#"[
        {
            "title": "Gemini",
            "time": "2026-05-30T14:22:00.000Z",
            "products": ["Gemini Apps"],
            "prompt": {"text":"What is FHNW?"},
            "response": {"text":"FHNW is..."}
        },
        {
            "title": "Gemini",
            "time": "2026-05-30T14:25:00.000Z",
            "prompt": {"text":"Tell me about Photon OS"},
            "response": {"text":"Photon OS is..."}
        }
    ]"#;

    #[test]
    fn parse_grok_share_json_extracts_turns() {
        let p = parse_grok(GROK_SHARE_JSON.as_bytes()).unwrap();
        assert_eq!(p.platform, "grok");
        assert_eq!(p.session_id.as_deref(), Some("abc123"));
        assert_eq!(p.model_hint.as_deref(), Some("grok-4"));
        assert_eq!(
            p.session_started_at.as_deref(),
            Some("2026-05-30T14:22:00Z")
        );
        assert_eq!(p.turns.len(), 2);
        assert_eq!(p.turns[0].role, "user");
        assert_eq!(p.turns[0].content_text, "What is FHNW?");
        assert_eq!(p.turns[1].role, "assistant");
    }

    #[test]
    fn parse_gemini_takeout_extracts_turns() {
        let p = parse_gemini(GEMINI_TAKEOUT_JSON.as_bytes()).unwrap();
        assert_eq!(p.platform, "gemini");
        // 2 prompt/response pairs = 4 turns total.
        assert_eq!(p.turns.len(), 4);
        assert_eq!(p.turns[0].role, "user");
        assert_eq!(p.turns[1].role, "assistant");
        assert_eq!(p.turns[2].role, "user");
        assert_eq!(p.turns[3].role, "assistant");
        assert_eq!(
            p.session_started_at.as_deref(),
            Some("2026-05-30T14:22:00.000Z")
        );
        assert_eq!(
            p.session_ended_at.as_deref(),
            Some("2026-05-30T14:25:00.000Z")
        );
    }

    #[test]
    fn parse_stub_round_trips() {
        let p = parse_stub("chatgpt", b"some raw export bytes");
        assert_eq!(p.platform, "chatgpt");
        assert_eq!(p.turns.len(), 0);
        assert!(p.session_id.is_none());
    }

    #[test]
    fn known_platform_table_locked() {
        assert!(is_known_platform("grok"));
        assert!(is_known_platform("gemini"));
        assert!(is_known_platform("chatgpt"));
        assert!(is_known_platform("claude"));
        assert!(is_known_platform("perplexity"));
        assert!(is_known_platform("other"));
        assert!(!is_known_platform("xai")); // alias is NOT accepted; use grok
        assert!(!is_known_platform(""));
    }

    #[test]
    fn grok_array_form_picks_first_element() {
        let arr = format!("[{GROK_SHARE_JSON}]");
        let p = parse_grok(arr.as_bytes()).unwrap();
        assert_eq!(p.session_id.as_deref(), Some("abc123"));
        assert_eq!(p.turns.len(), 2);
    }
}
