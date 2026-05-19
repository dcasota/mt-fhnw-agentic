//! Subcommand dispatchers. Each `mod` here owns one top-level verb.

mod check;
mod config;
mod content;
mod doctor;
mod export;
mod import;
mod init;
mod journal;
mod passport;
mod project;
mod provider;

use anyhow::Result;

use crate::cli::{Cli, Command};

pub async fn dispatch(args: Cli) -> Result<()> {
    match args.command {
        Command::Init(a) => init::run(&args.db, a, args.json),
        Command::Project { action } => project::run(&args.db, action, args.json),
        Command::Passport { action } => passport::run(&args.db, action, args.json),
        Command::Journal { action } => journal::run(&args.db, action, args.json),
        Command::Content { action } => content::run(&args.db, action, args.json),
        Command::Check { action } => check::run(&args.db, action, args.json).await,
        Command::Doctor => doctor::run(args.json),
        Command::Provider { action } => provider::run(action, args.json).await,
        Command::Config { action } => config::run(action, args.json),
        Command::Import { action } => import::run(&args.db, action, args.json),
        Command::Export {
            project,
            format,
            to,
            prefix,
            title,
        } => export::from_args(&args.db, project, format, to, prefix, title, args.json),
    }
}
