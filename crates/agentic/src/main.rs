//! `agentic` — monolithic CLI entry point.
//!
//! The full command tree is defined in [`cli`]; each top-level subcommand
//! delegates to a module that does the actual work.

#![warn(clippy::pedantic)]

mod cli;
mod commands;

use anyhow::Result;
use clap::Parser;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    let args = cli::Cli::parse();
    commands::dispatch(args).await
}

fn init_tracing() {
    let filter = EnvFilter::try_from_env("AGENTIC_LOG")
        .unwrap_or_else(|_| EnvFilter::new("warn,agentic=info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .compact()
        .init();
}
