//! Subcommand dispatchers. Each `mod` here owns one top-level verb.

mod check;
mod config;
mod content;
mod doctor;
mod embed;
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
        Command::Embed {
            project,
            prefix,
            provider,
            model,
            force,
        } => {
            embed::run_embed(
                &args.db,
                &project,
                &prefix,
                provider.as_deref(),
                model.as_deref(),
                force,
                args.json,
            )
            .await
        }
        Command::Classify {
            project,
            prefix,
            slots,
            provider,
            model,
        } => {
            embed::run_classify(
                &args.db,
                &project,
                &prefix,
                slots.as_deref(),
                provider.as_deref(),
                model.as_deref(),
                args.json,
            )
            .await
        }
        Command::Export {
            project,
            format,
            to,
            prefix,
            title,
        } => export::from_args(&args.db, project, format, to, prefix, title, args.json),
    }
}
