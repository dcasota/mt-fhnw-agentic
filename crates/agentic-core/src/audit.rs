//! Audit compiler — assembles a complete, non-repudiation audit for a project
//! (or a single item) from the append-only surfaces already in the database:
//! the journal (what the user did), the commit DAG (every change, with
//! human/AI authorship), the material passport (source origins → APA7, and the
//! AI ranking decisions), `audit_rows` (the per-item LLM-decision index),
//! `audit_verdicts` (gate verdicts), and `signatures` (the ML-DSA-87 seal).
//!
//! See ADR-0039 (PQC-only signing) and the AUDIT guide.

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Result, passport, project, signing, worktree};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserAction {
    pub entry_no: i64,
    pub actor: String,
    pub triggered_by: Option<String>,
    pub action_type: String,
    pub description: String,
    pub reasoning: Option<String>,
    pub hallucination_risk: Option<String>,
    pub approval_required: bool,
    pub approval_given: Option<String>,
    pub ts: String,
    pub commit_sha: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangeRecord {
    pub sha256: String,
    pub author: String,
    pub actor_kind: String,
    pub iteration: Option<i64>,
    pub message: String,
    pub timestamp: String,
    pub signed: bool,
    pub key_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceOrigin {
    pub citation_key: String,
    pub apa7: String,
    pub kind: String,
    pub ingest_source: Option<String>,
    pub dimension: Option<i64>,
    pub embedded_into: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmDecision {
    pub ts: String,
    pub agent: String,
    pub action: String,
    pub target: Option<String>,
    pub result: String,
    pub model: Option<String>,
    pub tokens_used: Option<i64>,
    pub iteration: Option<i64>,
    pub detail: Option<String>,
    pub reconstructed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Verdict {
    pub checkpoint: String,
    pub verdict: String,
    pub iteration: Option<i64>,
    pub ts: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Seal {
    pub head_commit: Option<String>,
    pub head_signed: bool,
    pub alg: String,
    pub key_id: Option<String>,
    pub public_key: Option<String>,
    pub signed_commits: i64,
    pub total_commits: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditReport {
    pub project_id: String,
    pub project_name: String,
    pub item_filter: Option<String>,
    pub generated_at: String,
    pub user_actions: Vec<UserAction>,
    pub changes: Vec<ChangeRecord>,
    pub source_origins: Vec<SourceOrigin>,
    pub llm_decisions: Vec<LlmDecision>,
    pub verdicts: Vec<Verdict>,
    pub seal: Seal,
}

fn matches_item(filter: &Option<String>, haystacks: &[&str]) -> bool {
    match filter {
        None => true,
        Some(f) => {
            let f = f.to_lowercase();
            haystacks.iter().any(|h| h.to_lowercase().contains(&f))
        }
    }
}

/// Render a literature-corpus payload as an APA7 reference (best-effort).
pub fn apa7(v: &Value) -> String {
    let authors = v.get("authors").and_then(|a| a.as_array());
    let author_str = match authors {
        Some(list) if !list.is_empty() => {
            let names: Vec<String> = list
                .iter()
                .map(|a| {
                    let family = a.get("family").and_then(Value::as_str).unwrap_or("");
                    let given = a.get("given").and_then(Value::as_str).unwrap_or("");
                    let initials: String = given
                        .split_whitespace()
                        .filter_map(|w| w.chars().next())
                        .map(|c| format!("{c}."))
                        .collect::<Vec<_>>()
                        .join(" ");
                    if initials.is_empty() {
                        family.to_owned()
                    } else {
                        format!("{family}, {initials}")
                    }
                })
                .collect();
            match names.len() {
                1 => names[0].clone(),
                _ => {
                    let (last, rest) = names.split_last().unwrap();
                    format!("{}, & {}", rest.join(", "), last)
                }
            }
        }
        _ => v
            .get("organization")
            .and_then(Value::as_str)
            .unwrap_or("Anonymous")
            .to_owned(),
    };
    // Render year cleanly whether stored as a JSON number or string (a string
    // "n.d." must not come out quoted); empty/absent → "n.d.".
    let year = v
        .get("year")
        .map(|y| match y {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        })
        .filter(|s| !s.is_empty() && s != "null")
        .unwrap_or_else(|| "n.d.".into());
    let title = v.get("title").and_then(Value::as_str).unwrap_or("Untitled");
    let venue = v.get("venue").and_then(Value::as_str).unwrap_or("");
    let url = v.get("url").and_then(Value::as_str).unwrap_or("");
    let mut s = format!("{author_str} ({year}). {title}.");
    if !venue.is_empty() {
        s.push_str(&format!(" {venue}."));
    }
    if !url.is_empty() {
        s.push_str(&format!(" {url}"));
    }
    s
}

/// Compile a complete audit report for `project_id`, optionally filtered to an
/// `item` (substring matched against commit messages, journal descriptions,
/// passport ids/placements).
pub fn compile(conn: &Connection, project_id: &str, item: Option<&str>) -> Result<AuditReport> {
    let proj = project::get(conn, project_id)?;
    let filter = item.map(std::string::ToString::to_string);

    // --- 1. User actions (journal) -------------------------------------------
    let mut stmt = conn.prepare(
        "SELECT entry_no, actor, triggered_by, action_type, description, reasoning, \
                hallucination_risk, user_approval_required, user_approval_given, ts, commit_sha \
         FROM journal_entries WHERE project_id = ?1 ORDER BY entry_no",
    )?;
    let all_actions: Vec<UserAction> = stmt
        .query_map(params![project_id], |r| {
            Ok(UserAction {
                entry_no: r.get(0)?,
                actor: r.get(1)?,
                triggered_by: r.get(2)?,
                action_type: r.get(3)?,
                description: r.get(4)?,
                reasoning: r.get(5)?,
                hallucination_risk: r.get(6)?,
                approval_required: r.get::<_, i64>(7)? != 0,
                approval_given: r.get(8)?,
                ts: r.get(9)?,
                commit_sha: r.get(10)?,
            })
        })?
        .collect::<std::result::Result<_, _>>()?;
    let user_actions: Vec<UserAction> = all_actions
        .into_iter()
        .filter(|a| matches_item(&filter, &[&a.description, &a.action_type]))
        .collect();

    // --- 2. Change records (commit DAG + signature status) -------------------
    let mut cstmt = conn.prepare(
        "SELECT sha256, author, actor_kind, iteration, message, timestamp \
         FROM commits ORDER BY timestamp",
    )?;
    let raw_commits: Vec<(String, String, String, Option<i64>, String, String)> = cstmt
        .query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
            ))
        })?
        .collect::<std::result::Result<_, _>>()?;
    let mut changes = Vec::new();
    for (sha, author, actor_kind, iteration, message, timestamp) in raw_commits {
        if !matches_item(&filter, &[&message]) {
            continue;
        }
        let sigs = signing::signatures_for(conn, "commit", &sha)?;
        changes.push(ChangeRecord {
            signed: !sigs.is_empty(),
            key_id: sigs.first().map(|s| s.key_id.clone()),
            sha256: sha,
            author,
            actor_kind,
            iteration,
            message,
            timestamp,
        });
    }

    // --- 3. Source origins → APA7 (literature_corpus) + who cited them -------
    // Build item→sources from claim_audit_results so each source lists the
    // thesis items it was embedded into.
    let cars = passport::current(conn, project_id, passport::Section::ClaimAuditResults)?;
    let mut cited_by: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for e in &cars {
        if let Ok(v) = serde_json::from_str::<Value>(&e.payload_json) {
            let placement = v
                .get("item")
                .and_then(Value::as_str)
                .or_else(|| v.get("id").and_then(Value::as_str))
                .unwrap_or("(unattributed)")
                .to_owned();
            if let Some(srcs) = v
                .get("provenance")
                .and_then(|p| p.get("sources"))
                .and_then(Value::as_array)
            {
                for s in srcs {
                    if let Some(ss) = s.as_str() {
                        cited_by
                            .entry(ss.to_owned())
                            .or_default()
                            .push(placement.clone());
                    }
                }
            }
        }
    }

    let lit = passport::current(conn, project_id, passport::Section::LiteratureCorpus)?;
    let mut source_origins = Vec::new();
    for e in &lit {
        let Ok(v) = serde_json::from_str::<Value>(&e.payload_json) else {
            continue;
        };
        let citation_key = v
            .get("citation_key")
            .and_then(Value::as_str)
            .unwrap_or("(no key)")
            .to_owned();
        let title = v.get("title").and_then(Value::as_str).unwrap_or("");
        if !matches_item(&filter, &[&citation_key, title]) {
            continue;
        }
        // Match this corpus entry to any claim that cited it (by key or title).
        let mut embedded_into: Vec<String> = cited_by
            .iter()
            .filter(|(src, _)| {
                src.contains(&citation_key) || (!title.is_empty() && src.contains(title))
            })
            .flat_map(|(_, items)| items.clone())
            .collect();
        embedded_into.sort();
        embedded_into.dedup();
        source_origins.push(SourceOrigin {
            apa7: apa7(&v),
            kind: v
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("source")
                .to_owned(),
            ingest_source: v
                .get("ingest_source")
                .and_then(Value::as_str)
                .map(str::to_owned),
            dimension: v.get("dimension").and_then(Value::as_i64),
            citation_key,
            embedded_into,
        });
    }

    // --- 4. LLM-decision index (audit_rows + reconstructed from claims) ------
    let mut llm_decisions = Vec::new();
    let mut astmt = conn.prepare(
        "SELECT ts, agent, action, target, result, model, tokens_used, iteration, sidecar_json \
         FROM audit_rows WHERE project_id = ?1 ORDER BY ts",
    )?;
    let rows: Vec<LlmDecision> = astmt
        .query_map(params![project_id], |r| {
            Ok(LlmDecision {
                ts: r.get(0)?,
                agent: r.get(1)?,
                action: r.get(2)?,
                target: r.get(3)?,
                result: r.get(4)?,
                model: r.get(5)?,
                tokens_used: r.get(6)?,
                iteration: r.get(7)?,
                detail: r.get(8)?,
                reconstructed: false,
            })
        })?
        .collect::<std::result::Result<_, _>>()?;
    for d in rows {
        if matches_item(&filter, &[d.target.as_deref().unwrap_or(""), &d.action]) {
            llm_decisions.push(d);
        }
    }
    // Reconstruct AI ranking decisions from claim-audit-results when not already
    // present as recorded rows (best-effort historical backfill view).
    for e in &cars {
        if let Ok(v) = serde_json::from_str::<Value>(&e.payload_json) {
            let target = v
                .get("id")
                .and_then(Value::as_str)
                .or_else(|| v.get("item").and_then(Value::as_str))
                .unwrap_or("(item)")
                .to_owned();
            if !matches_item(&filter, &[&target]) {
                continue;
            }
            let placement = v.get("placement").and_then(Value::as_str).unwrap_or("");
            llm_decisions.push(LlmDecision {
                ts: e.added_at.clone(),
                agent: "claim-audit (reconstructed)".into(),
                action: format!("rank→{placement}"),
                target: Some(target),
                result: "info".into(),
                model: None,
                tokens_used: None,
                iteration: None,
                detail: v
                    .get("justification")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                reconstructed: true,
            });
        }
    }

    // --- 5. Gate verdicts ----------------------------------------------------
    let mut vstmt = conn.prepare(
        "SELECT checkpoint, verdict, iteration, ts FROM audit_verdicts \
         WHERE project_id = ?1 ORDER BY ts",
    )?;
    let verdicts: Vec<Verdict> = vstmt
        .query_map(params![project_id], |r| {
            Ok(Verdict {
                checkpoint: r.get(0)?,
                verdict: r.get(1)?,
                iteration: r.get(2)?,
                ts: r.get(3)?,
            })
        })?
        .collect::<std::result::Result<_, _>>()?;

    // --- 6. Integrity seal ---------------------------------------------------
    let head = worktree::head_commit(conn, project_id)?.map(|c| c.sha256);
    let head_signed = match &head {
        Some(h) => !signing::signatures_for(conn, "commit", h)?.is_empty(),
        None => false,
    };
    let active = signing::active_key(conn)?;
    let total_commits: i64 = conn.query_row("SELECT COUNT(*) FROM commits", [], |r| r.get(0))?;
    let signed_commits = signing::count_by_kind(conn, "commit")?;
    let seal = Seal {
        head_commit: head,
        head_signed,
        alg: signing::ALG.to_owned(),
        key_id: active.as_ref().map(|k| k.key_id.clone()),
        public_key: active.as_ref().map(|k| k.public_key.clone()),
        signed_commits,
        total_commits,
    };

    Ok(AuditReport {
        project_id: project_id.to_owned(),
        project_name: proj.name,
        item_filter: filter,
        generated_at: chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        user_actions,
        changes,
        source_origins,
        llm_decisions,
        verdicts,
        seal,
    })
}

/// Render the report as Markdown (the cryptographic seal/signature block is
/// appended separately by the caller after signing the rendered body).
pub fn render_markdown(rep: &AuditReport) -> String {
    let mut o = String::new();
    let scope = rep.item_filter.as_deref().map_or_else(
        || "whole project".to_owned(),
        |f| format!("item filter: `{f}`"),
    );
    o.push_str(&format!(
        "# Audit report — {} ({})\n\n_Project `{}` · generated {} · {scope}_\n\n",
        rep.project_name, rep.project_id, rep.project_id, rep.generated_at
    ));

    o.push_str("## 1 Summary\n\n");
    o.push_str(&format!(
        "- User/journal actions: {}\n- Change records (commits): {} ({} signed of {} total)\n- Source origins (APA7): {}\n- LLM decisions indexed: {}\n- Gate verdicts: {}\n- Signing algorithm: {} (ADR-0039, PQC-only)\n\n",
        rep.user_actions.len(),
        rep.changes.len(),
        rep.seal.signed_commits,
        rep.seal.total_commits,
        rep.source_origins.len(),
        rep.llm_decisions.len(),
        rep.verdicts.len(),
        rep.seal.alg,
    ));

    o.push_str("## 2 What the user did (journal)\n\n");
    o.push_str(
        "| # | When | Actor | Action | Approval | Description |\n|---|---|---|---|---|---|\n",
    );
    for a in &rep.user_actions {
        let appr = if a.approval_required {
            a.approval_given.as_deref().map_or("required", |_| "given")
        } else {
            "-"
        };
        o.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} |\n",
            a.entry_no,
            a.ts,
            a.actor,
            a.action_type,
            appr,
            a.description
                .replace('|', "\\|")
                .chars()
                .take(140)
                .collect::<String>()
        ));
    }
    o.push('\n');

    o.push_str("## 3 Change records (commit DAG, with authorship)\n\n");
    o.push_str(
        "| Commit | When | Actor kind | Iter | Signed | Message |\n|---|---|---|---|---|---|\n",
    );
    for c in &rep.changes {
        o.push_str(&format!(
            "| `{}` | {} | {} | {} | {} | {} |\n",
            &c.sha256[..c.sha256.len().min(12)],
            c.timestamp,
            c.actor_kind,
            c.iteration
                .map(|i| i.to_string())
                .unwrap_or_else(|| "-".into()),
            if c.signed { "yes" } else { "NO" },
            c.message
                .replace('|', "\\|")
                .chars()
                .take(100)
                .collect::<String>()
        ));
    }
    o.push('\n');

    o.push_str("## 4 Source origins (APA7) and the items they were embedded into\n\n");
    for s in &rep.source_origins {
        let by = if s.embedded_into.is_empty() {
            String::new()
        } else {
            format!(" — embedded into: {}", s.embedded_into.join("; "))
        };
        let src = s
            .ingest_source
            .as_deref()
            .map_or(String::new(), |x| format!(" [{x}]"));
        o.push_str(&format!("- {}{}{}\n", s.apa7, src, by));
    }
    o.push('\n');

    o.push_str("## 5 AI (LLM) decision index, per item\n\n");
    o.push_str("| When | Agent | Action | Target | Result | Model | Tokens | Source |\n|---|---|---|---|---|---|---|---|\n");
    for d in &rep.llm_decisions {
        o.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} |\n",
            d.ts,
            d.agent,
            d.action.replace('|', "\\|"),
            d.target
                .as_deref()
                .unwrap_or("-")
                .replace('|', "\\|")
                .chars()
                .take(60)
                .collect::<String>(),
            d.result,
            d.model.as_deref().unwrap_or("-"),
            d.tokens_used
                .map(|t| t.to_string())
                .unwrap_or_else(|| "-".into()),
            if d.reconstructed {
                "reconstructed"
            } else {
                "recorded"
            },
        ));
    }
    o.push('\n');

    o.push_str("## 6 Gate verdicts\n\n");
    if rep.verdicts.is_empty() {
        o.push_str("_No gate verdicts recorded in the database._\n\n");
    } else {
        o.push_str("| When | Checkpoint | Verdict | Iter |\n|---|---|---|---|\n");
        for v in &rep.verdicts {
            o.push_str(&format!(
                "| {} | {} | {} | {} |\n",
                v.ts,
                v.checkpoint,
                v.verdict,
                v.iteration
                    .map(|i| i.to_string())
                    .unwrap_or_else(|| "-".into())
            ));
        }
        o.push('\n');
    }

    // --- 7. AIBOM — single chronological ledger of every auditable event ----
    o.push_str("## 7 AIBOM — chronological decision-and-change ledger\n\n");
    o.push_str(
        "A merged, strictly time-ordered record of every journal action, content \
commit, AI/LLM decision and gate verdict — the AI Bill of Materials chronology. \
Sealed by this report's signature.\n\n",
    );
    let trunc = |s: &str, n: usize| s.replace('|', "\\|").chars().take(n).collect::<String>();
    let mut events: Vec<(String, &'static str, String)> = Vec::new();
    for a in &rep.user_actions {
        events.push((
            a.ts.clone(),
            "journal",
            format!("[{}] {}", a.action_type, trunc(&a.description, 90)),
        ));
    }
    for c in &rep.changes {
        events.push((
            c.timestamp.clone(),
            if c.signed {
                "commit (signed)"
            } else {
                "commit (UNSIGNED)"
            },
            format!(
                "{} {}",
                &c.sha256[..c.sha256.len().min(12)],
                trunc(&c.message, 72)
            ),
        ));
    }
    for d in &rep.llm_decisions {
        events.push((
            d.ts.clone(),
            "ai-decision",
            format!("{} -> {}", trunc(&d.action, 60), d.result),
        ));
    }
    for v in &rep.verdicts {
        events.push((
            v.ts.clone(),
            "gate",
            format!("{}: {}", v.checkpoint, v.verdict),
        ));
    }
    events.sort_by(|x, y| x.0.cmp(&y.0));
    o.push_str(&format!(
        "_{} events, {} -> {}_\n\n",
        events.len(),
        events.first().map_or("-", |e| e.0.as_str()),
        events.last().map_or("-", |e| e.0.as_str()),
    ));
    o.push_str("| When | Kind | Event |\n|---|---|---|\n");
    for (ts, kind, detail) in &events {
        o.push_str(&format!("| {ts} | {kind} | {detail} |\n"));
    }
    o.push('\n');

    o.push_str("## 8 Integrity seal (ML-DSA-87, FIPS 204)\n\n");
    o.push_str(&format!(
        "- HEAD commit: `{}`\n- HEAD signed: {}\n- Signed commits: {} of {}\n- Signing key id: {}\n",
        rep.seal.head_commit.as_deref().unwrap_or("(none)"),
        if rep.seal.head_signed { "yes" } else { "NO" },
        rep.seal.signed_commits,
        rep.seal.total_commits,
        rep.seal.key_id.as_deref().unwrap_or("(no active key)"),
    ));
    o
}
