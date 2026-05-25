//! Orchestration sessions — in-tool replacement for the external PowerShell
//! launcher (`launch_session.ps1` + `run_*_all.ps1` + `sessions/manifest.json`).
//!
//! This module holds the *pure* logic (insert / list / status-update, the
//! 429/rate-limit detector, the reset-time parser, and the `claude` argument
//! builder) so it is unit-testable without spawning a real `claude` process.
//! The CLI command (`agentic orchestrate`) owns the I/O and process spawning.

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::Result;

/// The bounded-sub-session guard appended to every `claude` invocation.
pub const GUARD: &str = "You are a bounded sub-session. Work ONLY inside the project dir. Write/edit files only; do not run destructive commands. Produce exactly the requested deliverable, then stop. English is the source-of-truth language; no foreign-language text in an English document.";

/// Default per-session budget in USD.
pub const DEFAULT_BUDGET_USD: f64 = 3.0;

/// Default rate-limit backoff (seconds) when no reset time can be parsed.
pub const DEFAULT_BACKOFF_SECS: u64 = 1800;

/// Cap on any computed backoff (seconds) — never sleep longer than 6h.
pub const MAX_BACKOFF_SECS: u64 = 6 * 60 * 60;

/// Lifecycle state of an orchestration session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pending,
    Running,
    Done,
    Failed,
    Ratelimited,
}

impl Status {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Ratelimited => "ratelimited",
        }
    }
}

/// One orchestration session row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub project_id: String,
    pub task: String,
    pub budget_usd: f64,
    pub model: Option<String>,
    pub status: String,
    pub transcript_path: Option<String>,
    pub exit_summary: Option<String>,
    pub created_at: String,
    pub closed_at: Option<String>,
}

/// Build the timestamp-prefixed stored id from a short id (`"dim01"` →
/// `"20260525-141200-dim01"`). Kept separate so the timestamp source is
/// injectable in tests.
#[must_use]
pub fn make_session_id(prefix_ts: &str, short_id: &str) -> String {
    format!("{prefix_ts}-{short_id}")
}

/// Current UTC timestamp formatted for an id prefix (`yyyyMMdd-HHmmss`).
#[must_use]
pub fn now_id_prefix() -> String {
    chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string()
}

/// Insert a new `pending` session and return its stored (timestamp-prefixed) id.
pub fn add(
    conn: &Connection,
    stored_id: &str,
    project_id: &str,
    task: &str,
    budget_usd: f64,
    model: Option<&str>,
) -> Result<String> {
    conn.execute(
        "INSERT INTO orchestration_sessions (id, project_id, task, budget_usd, model, status) \
         VALUES (?1, ?2, ?3, ?4, ?5, 'pending')",
        params![stored_id, project_id, task, budget_usd, model],
    )?;
    Ok(stored_id.to_string())
}

/// List every session for a project, newest first.
pub fn list(conn: &Connection, project_id: &str) -> Result<Vec<Session>> {
    let mut stmt = conn.prepare(
        "SELECT id, project_id, task, budget_usd, model, status, transcript_path, \
                exit_summary, created_at, closed_at \
         FROM orchestration_sessions WHERE project_id = ?1 \
         ORDER BY created_at DESC, id DESC",
    )?;
    let rows = stmt
        .query_map(params![project_id], row_to_session)?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Fetch a single session by id.
pub fn get(conn: &Connection, id: &str) -> Result<Option<Session>> {
    let mut stmt = conn.prepare(
        "SELECT id, project_id, task, budget_usd, model, status, transcript_path, \
                exit_summary, created_at, closed_at \
         FROM orchestration_sessions WHERE id = ?1",
    )?;
    let mut rows = stmt.query_map(params![id], row_to_session)?;
    match rows.next() {
        Some(r) => Ok(Some(r?)),
        None => Ok(None),
    }
}

/// Every still-pending session for a project, oldest first (FIFO execution).
pub fn pending(conn: &Connection, project_id: &str) -> Result<Vec<Session>> {
    let mut stmt = conn.prepare(
        "SELECT id, project_id, task, budget_usd, model, status, transcript_path, \
                exit_summary, created_at, closed_at \
         FROM orchestration_sessions WHERE project_id = ?1 AND status = 'pending' \
         ORDER BY created_at ASC, id ASC",
    )?;
    let rows = stmt
        .query_map(params![project_id], row_to_session)?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Set a session's status (used for the `running`/`ratelimited` transitions).
pub fn set_status(conn: &Connection, id: &str, status: Status) -> Result<()> {
    conn.execute(
        "UPDATE orchestration_sessions SET status = ?2 WHERE id = ?1",
        params![id, status.as_str()],
    )?;
    Ok(())
}

/// Record a session's transcript path.
pub fn set_transcript(conn: &Connection, id: &str, path: &str) -> Result<()> {
    conn.execute(
        "UPDATE orchestration_sessions SET transcript_path = ?2 WHERE id = ?1",
        params![id, path],
    )?;
    Ok(())
}

/// Mark a session terminal (`done`/`failed`), stamping `closed_at` and an
/// optional exit summary. Uses the same UTC format as the DB defaults.
pub fn close(
    conn: &Connection,
    id: &str,
    status: Status,
    exit_summary: Option<&str>,
) -> Result<()> {
    let closed_at = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    conn.execute(
        "UPDATE orchestration_sessions \
         SET status = ?2, exit_summary = ?3, closed_at = ?4 WHERE id = ?1",
        params![id, status.as_str(), exit_summary, closed_at],
    )?;
    Ok(())
}

fn row_to_session(row: &rusqlite::Row) -> rusqlite::Result<Session> {
    Ok(Session {
        id: row.get(0)?,
        project_id: row.get(1)?,
        task: row.get(2)?,
        budget_usd: row.get(3)?,
        model: row.get(4)?,
        status: row.get(5)?,
        transcript_path: row.get(6)?,
        exit_summary: row.get(7)?,
        created_at: row.get(8)?,
        closed_at: row.get(9)?,
    })
}

/// True when the transcript's last JSON line indicates a rate-limit / 429 error:
/// it must carry `"is_error":true` AND mention either `429` or "hit your limit".
#[must_use]
pub fn detect_ratelimit(last_line: &str) -> bool {
    let lower = last_line.to_ascii_lowercase();
    let is_error = lower.contains("\"is_error\":true") || lower.contains("\"is_error\": true");
    if !is_error {
        return false;
    }
    lower.contains("429") || lower.contains("hit your limit")
}

/// True when the transcript's last JSON line is a clean (non-error) result.
#[must_use]
pub fn is_clean_result(last_line: &str) -> bool {
    let lower = last_line.to_ascii_lowercase();
    let is_result = lower.contains("\"type\":\"result\"") || lower.contains("\"type\": \"result\"");
    let is_error = lower.contains("\"is_error\":true") || lower.contains("\"is_error\": true");
    is_result && !is_error
}

/// Parse a `resets <h>:<mm> <am|pm>` hint into a backoff.
///
/// Returns the number of seconds to sleep until the reset time, capped at
/// [`MAX_BACKOFF_SECS`]. `now_minutes_of_day` is "now" expressed as minutes
/// since local midnight so the computation is deterministic (and testable).
/// Returns `None` if no reset time is present, in which case the caller falls
/// back to [`DEFAULT_BACKOFF_SECS`].
#[must_use]
pub fn parse_reset_backoff(line: &str, now_minutes_of_day: u32) -> Option<u64> {
    let target = parse_reset_minutes_of_day(line)?;
    // Minutes until the target time today; if it already passed, it's tomorrow.
    let day = 24 * 60;
    let delta_min = if target >= now_minutes_of_day {
        target - now_minutes_of_day
    } else {
        day - now_minutes_of_day + target
    };
    let secs = u64::from(delta_min) * 60;
    Some(secs.min(MAX_BACKOFF_SECS))
}

/// Parse the `resets <h>:<mm> <am|pm>` clock time into minutes since midnight.
fn parse_reset_minutes_of_day(line: &str) -> Option<u32> {
    let lower = line.to_ascii_lowercase();
    let idx = lower.find("resets")?;
    let rest = &lower[idx + "resets".len()..];
    // Find the first H:MM token.
    let bytes = rest.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            break;
        }
        i += 1;
    }
    let rest = &rest[i..];
    let colon = rest.find(':')?;
    let hour: u32 = rest[..colon].trim().parse().ok()?;
    let after = &rest[colon + 1..];
    let min_str: String = after.chars().take_while(char::is_ascii_digit).collect();
    if min_str.len() < 2 {
        return None;
    }
    let minute: u32 = min_str.parse().ok()?;
    if hour > 12 || minute > 59 {
        return None;
    }
    // am/pm comes after the digits (possibly after a space).
    let tail = &after[min_str.len()..];
    let pm = tail.contains("pm");
    let am = tail.contains("am");
    let hour24 = if pm && hour != 12 {
        hour + 12
    } else if am && hour == 12 {
        0
    } else {
        hour
    };
    Some(hour24 * 60 + minute)
}

/// Build the argument vector for one `claude` invocation. The program name
/// (`claude`) is NOT included — only the args. The task text is piped to stdin
/// by the caller, not passed as an argument.
#[must_use]
pub fn claude_args(
    root: &str,
    transcript_dir_known: bool,
    model: Option<&str>,
    budget_usd: f64,
) -> Vec<String> {
    // `transcript_dir_known` is accepted for symmetry with the spawn site but is
    // not itself an argument; kept so the signature documents the contract.
    let _ = transcript_dir_known;
    let mut args: Vec<String> = vec![
        "-p".into(),
        "--add-dir".into(),
        root.into(),
        "--permission-mode".into(),
        "acceptEdits".into(),
    ];
    if let Some(m) = model {
        args.push("--model".into());
        args.push(m.into());
    }
    args.push("--append-system-prompt".into());
    args.push(GUARD.into());
    args.push("--output-format".into());
    args.push("stream-json".into());
    args.push("--verbose".into());
    args.push("--max-budget-usd".into());
    args.push(format!("{budget_usd}"));
    args
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use pretty_assertions::assert_eq;

    #[test]
    fn add_then_list_round_trips() {
        let conn = open_in_memory().unwrap();
        let id = make_session_id("20260525-141200", "dim01");
        let stored = add(&conn, &id, "PROJ", "write chapter 1", 3.0, Some("opus")).unwrap();
        assert_eq!(stored, id);

        let rows = list(&conn, "PROJ").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, id);
        assert_eq!(rows[0].project_id, "PROJ");
        assert_eq!(rows[0].task, "write chapter 1");
        assert_eq!(rows[0].budget_usd, 3.0);
        assert_eq!(rows[0].model.as_deref(), Some("opus"));
        assert_eq!(rows[0].status, "pending");

        // A different project sees nothing.
        assert!(list(&conn, "OTHER").unwrap().is_empty());
    }

    #[test]
    fn status_update_and_close() {
        let conn = open_in_memory().unwrap();
        let id = make_session_id("20260525-141200", "dim02");
        add(&conn, &id, "PROJ", "task", 3.0, None).unwrap();

        set_status(&conn, &id, Status::Running).unwrap();
        assert_eq!(get(&conn, &id).unwrap().unwrap().status, "running");

        set_status(&conn, &id, Status::Ratelimited).unwrap();
        assert_eq!(get(&conn, &id).unwrap().unwrap().status, "ratelimited");

        close(&conn, &id, Status::Done, Some("tail summary")).unwrap();
        let s = get(&conn, &id).unwrap().unwrap();
        assert_eq!(s.status, "done");
        assert_eq!(s.exit_summary.as_deref(), Some("tail summary"));
        assert!(s.closed_at.is_some());

        // pending() no longer returns a closed session.
        assert!(pending(&conn, "PROJ").unwrap().is_empty());
    }

    #[test]
    fn pending_is_fifo_and_filters_done() {
        let conn = open_in_memory().unwrap();
        add(&conn, "20260525-100000-a", "P", "first", 3.0, None).unwrap();
        add(&conn, "20260525-100001-b", "P", "second", 3.0, None).unwrap();
        close(&conn, "20260525-100000-a", Status::Done, None).unwrap();
        let p = pending(&conn, "P").unwrap();
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].id, "20260525-100001-b");
    }

    #[test]
    fn detect_ratelimit_true_for_429_error_line() {
        let line = r#"{"type":"result","is_error":true,"error":{"message":"Request failed: 429 Too Many Requests"}}"#;
        assert!(detect_ratelimit(line));
    }

    #[test]
    fn detect_ratelimit_true_for_hit_your_limit() {
        let line = r#"{"type":"result","is_error":true,"result":"You have hit your limit. resets 3:30 pm"}"#;
        assert!(detect_ratelimit(line));
    }

    #[test]
    fn detect_ratelimit_false_for_clean_result() {
        let line = r#"{"type":"result","is_error":false,"result":"Done","total_cost_usd":1.23}"#;
        assert!(!detect_ratelimit(line));
        assert!(is_clean_result(line));
    }

    #[test]
    fn detect_ratelimit_false_for_unrelated_error() {
        // is_error but no 429 / limit signal -> not a rate-limit (caller treats as failed).
        let line = r#"{"type":"result","is_error":true,"result":"syntax error in tool input"}"#;
        assert!(!detect_ratelimit(line));
    }

    #[test]
    fn parse_reset_backoff_computes_seconds() {
        // "resets 3:30 pm" == 15:30 == 930 minutes of day.
        let line = "You have hit your limit. resets 3:30 pm.";
        // now = 15:00 (900). delta = 30 min = 1800 s.
        let secs = parse_reset_backoff(line, 900).unwrap();
        assert_eq!(secs, 1800);
    }

    #[test]
    fn parse_reset_backoff_wraps_to_next_day() {
        // resets 1:00 am == 60 min; now 23:00 == 1380 min -> 120 min until = 7200 s.
        let line = "limit. resets 1:00 am";
        assert_eq!(parse_reset_backoff(line, 1380).unwrap(), 7200);
    }

    #[test]
    fn parse_reset_backoff_caps_at_six_hours() {
        // now 0:00, resets 11:00 pm == 23:00 -> 23h, capped at 6h.
        let line = "resets 11:00 pm";
        assert_eq!(parse_reset_backoff(line, 0).unwrap(), MAX_BACKOFF_SECS);
    }

    #[test]
    fn parse_reset_backoff_none_when_absent() {
        assert!(parse_reset_backoff("just a 429 with no reset hint", 600).is_none());
    }

    #[test]
    fn claude_args_has_required_flags() {
        let args = claude_args(".", false, Some("claude-opus-4-7"), 3.0);
        assert!(args.contains(&"acceptEdits".to_string()));
        assert!(args.contains(&"stream-json".to_string()));
        assert!(args.contains(&"--max-budget-usd".to_string()));
        assert!(args.contains(&"3".to_string()));
        assert!(args.contains(&"--append-system-prompt".to_string()));
        assert!(args.contains(&GUARD.to_string()));
        assert!(args.contains(&"--model".to_string()));
        assert!(args.contains(&"claude-opus-4-7".to_string()));
        assert!(args.contains(&"--add-dir".to_string()));
        assert!(args.contains(&".".to_string()));
    }

    #[test]
    fn claude_args_omits_model_when_none() {
        let args = claude_args(".", false, None, 5.0);
        assert!(!args.contains(&"--model".to_string()));
        assert!(args.contains(&"5".to_string()));
    }
}
