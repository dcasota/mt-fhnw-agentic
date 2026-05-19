//! Subcommand dispatchers. Each `mod` here owns one top-level verb.

mod content;
mod doctor;
mod init;
mod journal;
mod passport;
mod project;

use anyhow::Result;

use crate::cli::{Cli, Command};

pub async fn dispatch(args: Cli) -> Result<()> {
    match args.command {
        Command::Init(a) => init::run(&args.db, a, args.json),
        Command::Project { action } => project::run(&args.db, action, args.json),
        Command::Passport { action } => passport::run(&args.db, action, args.json),
        Command::Journal { action } => journal::run(&args.db, action, args.json),
        Command::Content { action } => content::run(&args.db, action, args.json),
        Command::Doctor => doctor::run(args.json),
    }
}
