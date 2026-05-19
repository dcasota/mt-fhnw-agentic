//! M0 onboarding wizard. P0 ships a non-interactive placeholder; the full
//! ratatui TUI lands in a later phase.

use anyhow::Result;
use rusqlite::Connection;

/// Minimum field surface needed by the wizard launcher.
pub trait WizardArgs {
    fn mode(&self) -> &str;
    fn working_lang(&self) -> &str;
    fn institution(&self) -> Option<&str>;
}

/// Launch the wizard. P0 placeholder.
pub async fn launch<A>(_conn: &Connection, _args: &A) -> Result<()>
where
    A: WizardArgs + ?Sized,
{
    println!();
    println!("M0 Onboarding Wizard (P0 placeholder)");
    println!();
    println!("Full ratatui TUI ships in a later phase. For now, run with");
    println!("--no-wizard plus flags to scaffold:");
    println!();
    println!("    agentic init --no-wizard --mode single --lang de \\");
    println!("           --institution fhnw-mas");
    println!();
    println!("Then iterate:");
    println!("    agentic project status --id <ID>");
    println!("    agentic content put thesis/ch1.md");
    println!("    agentic journal append --project <id> --actor me \\");
    println!("           --action-type Writing --description \"first edit\"");
    println!();
    Ok(())
}
