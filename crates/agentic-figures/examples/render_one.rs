//! `render_one` — render one figspec JSON file to one PNG.
//!
//! Usage: `cargo run --release --example render_one -- <figspec.json> <out.png>`
//!
//! Thin wrapper around `agentic_figures::render_figspec`. We add this binary
//! because the `agentic figures` CLI subcommand only exposes the high-level
//! `regulation-timeline` and `extract-from-docx` flows; the bulk i18n work
//! for the thesis figures (Wave-A-style per-language renders) needs a way
//! to call the figspec dispatcher from outside the crate without going
//! through `resolve_markdown`'s markdown shell.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: render_one <figspec.json> <out.png>");
        return ExitCode::from(64);
    }
    let spec_path = PathBuf::from(&args[1]);
    let out_path = PathBuf::from(&args[2]);
    let json = match fs::read_to_string(&spec_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cannot read {}: {e}", spec_path.display());
            return ExitCode::from(66);
        }
    };
    match agentic_figures::render_figspec(&json, &out_path) {
        Ok(()) => {
            let size = fs::metadata(&out_path).map(|m| m.len()).unwrap_or(0);
            println!("wrote {} ({size} bytes)", out_path.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("render failed: {e:#}");
            ExitCode::from(1)
        }
    }
}
