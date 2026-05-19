use std::path::Path;

use anyhow::Result;

use crate::cli::InitArgs;

pub fn run(db_path: &Path, args: InitArgs, _json: bool) -> Result<()> {
    // P0 stub: just create the database. The wizard (ratatui TUI) is implemented in P1+.
    let conn = agentic_core::db::open(db_path)?;
    let version = agentic_core::db::current_version(&conn)?;
    println!("Initialised database at {} (schema v{})", db_path.display(), version);
    println!("mode={} institution={:?} track={:?} working_lang={}",
        args.mode, args.institution, args.track, args.working_lang);
    if !args.no_wizard {
        println!("\n(Wizard TUI not implemented yet — P0 ships storage only. Run with --no-wizard for now.)");
    }
    Ok(())
}
