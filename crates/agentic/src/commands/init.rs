use std::path::Path;
use std::str::FromStr;

use anyhow::{Context, Result, anyhow};

use agentic_providers::{ProviderKind, keychain};
use agentic_tui::wizard::{self, WizardOutcome, WizardState};

use crate::cli::InitArgs;

pub fn run(db_path: &Path, args: InitArgs, json: bool) -> Result<()> {
    let conn = agentic_core::db::open(db_path)?;
    let version = agentic_core::db::current_version(&conn)?;
    if !json {
        println!(
            "Initialised database at {} (schema v{})",
            db_path.display(),
            version
        );
    }

    if args.no_wizard {
        scaffold_non_interactive(&conn, &args, json)?;
        return Ok(());
    }

    match wizard::launch(&conn, &args, args.resume)? {
        WizardOutcome::Confirmed(state) => {
            let project_id = materialise(&conn, &state)?;
            wizard::discard_draft(&conn).ok();
            report_success(&state, &project_id, json);
        }
        WizardOutcome::Cancelled => {
            // No TTY, or user pressed Esc. Either way, give them the
            // non-interactive recipe.
            if !std::io::IsTerminal::is_terminal(&std::io::stdout()) {
                eprintln!("(no interactive TTY detected — re-run with --no-wizard plus flags)");
            } else {
                eprintln!("(wizard cancelled — draft preserved; re-run with --resume to continue)");
            }
        }
    }
    Ok(())
}

/// Non-interactive scaffold (the historical `--no-wizard` path).
fn scaffold_non_interactive(
    conn: &rusqlite::Connection,
    args: &InitArgs,
    json: bool,
) -> Result<()> {
    use agentic_core::project::{ProjectKind, create};
    let kind = ProjectKind::from_str("standalone").map_err(|e| anyhow!("internal: {e}"))?;
    let name = args
        .institution
        .as_deref()
        .map(|i| format!("{i} project"))
        .unwrap_or_else(|| "agentic project".into());
    let id =
        create(conn, &name, kind, &args.working_lang, None).context("create initial project")?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "project_id": id,
                "name": name,
                "mode": args.mode,
                "wizard": "skipped",
            }))?
        );
    } else {
        println!("Created project {id} ({name}).");
    }
    Ok(())
}

/// Wizard finished: create the project row, write keys, journal it.
fn materialise(conn: &rusqlite::Connection, state: &WizardState) -> Result<String> {
    use agentic_core::project::{ProjectKind, create};

    let kind = ProjectKind::from_str(&state.kind).map_err(|e| anyhow!("invalid kind: {e}"))?;
    let project_id = create(conn, &state.project_name, kind, &state.working_lang, None)
        .context("create project")?;

    // Stash institution + track in the project's metadata_json column.
    if !state.institution.is_empty() || !state.track.is_empty() {
        let metadata = serde_json::json!({
            "institution": state.institution,
            "track": state.track,
        });
        conn.execute(
            "UPDATE projects SET metadata_json = ?1 WHERE id = ?2",
            rusqlite::params![metadata.to_string(), project_id],
        )?;
    }

    // Write each captured provider key to the OS keychain. These are NOT in
    // the draft — they only exist in memory during the wizard run.
    for (name, key) in &state.provider_keys {
        let kind: ProviderKind = name
            .parse()
            .map_err(|e| anyhow!("internal: bad provider name {name}: {e}"))?;
        keychain::set_key(kind.as_str(), key)
            .with_context(|| format!("store {name} key in OS keychain"))?;
    }

    // Drop a journal entry so the first thing the user sees in
    // `agentic journal show` is the wizard run.
    let entry_no: i64 = 1;
    conn.execute(
        "INSERT INTO journal_entries
            (project_id, entry_no, actor, action_type, description, reasoning)
         VALUES (?1, ?2, 'wizard', 'Init', ?3, ?4)",
        rusqlite::params![
            project_id,
            entry_no,
            format!(
                "Project created via wizard: name=\"{}\" kind={} lang={}",
                state.project_name, state.kind, state.working_lang
            ),
            format!(
                "institution={}; track={}; providers_keyed={}",
                state.institution,
                state.track,
                state.providers_keyed.len()
            ),
        ],
    )?;
    Ok(project_id)
}

fn report_success(state: &WizardState, project_id: &str, json: bool) {
    if json {
        let providers: Vec<&str> = state
            .providers_keyed
            .iter()
            .filter_map(|i| wizard::PROVIDERS.get(*i).copied())
            .collect();
        let payload = serde_json::json!({
            "project_id": project_id,
            "name": state.project_name,
            "kind": state.kind,
            "working_lang": state.working_lang,
            "institution": state.institution,
            "track": state.track,
            "providers_keyed": providers,
        });
        // Best-effort: if JSON fails, fall back to text.
        match serde_json::to_string_pretty(&payload) {
            Ok(s) => println!("{s}"),
            Err(_) => {
                println!("Created project {project_id} ({}).", state.project_name);
            }
        }
    } else {
        println!();
        println!("✓ Project created.");
        println!("    id:        {project_id}");
        println!("    name:      {}", state.project_name);
        println!("    kind:      {}", state.kind);
        println!("    language:  {}", state.working_lang);
        if !state.institution.is_empty() {
            println!("    institution: {}", state.institution);
        }
        if !state.track.is_empty() {
            println!("    track:     {}", state.track);
        }
        if !state.providers_keyed.is_empty() {
            let names: Vec<&str> = state
                .providers_keyed
                .iter()
                .filter_map(|i| wizard::PROVIDERS.get(*i).copied())
                .collect();
            println!("    keys set:  {}", names.join(", "));
        }
        println!();
        println!("Next: agentic project status --id {project_id}");
    }
}
