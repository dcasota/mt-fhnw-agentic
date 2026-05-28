//! Subcommand dispatchers. Each `mod` here owns one top-level verb.

mod audit;
mod authorize;
mod bibliography;
mod book;
mod cascade;
mod check;
mod config;
mod content;
mod doctor;
mod embed;
mod export;
pub(crate) mod facts;
#[path = "gen.rs"]
mod generate;
mod import;
mod inbox;
mod init;
mod journal;
mod merge;
mod migrate;
mod normalize;
mod orchestrate;
mod passport;
mod prisma;
mod project;
mod promote;
mod provider;
mod rank;
mod review;
mod risk;
mod verify;

use anyhow::Result;

use crate::cli::{Cli, Command};

pub async fn dispatch(args: Cli) -> Result<()> {
    match args.command {
        Command::Init(a) => init::run(&args.db, a, args.json),
        Command::Project { action } => project::run(&args.db, action, args.json),
        Command::Passport { action } => passport::run(&args.db, action, args.json),
        Command::Journal { action } => journal::run(&args.db, action, args.json),
        Command::Content { action } => content::run(&args.db, action, args.json),
        Command::Authorize { action } => authorize::run(&args.db, action, args.json),
        Command::Promote {
            project,
            issue,
            snapshot,
        } => promote::run(&args.db, &project, &issue, snapshot.as_deref(), args.json),
        Command::Audit { action } => audit::run(&args.db, action, args.json),
        Command::Inbox { action } => inbox::run(&args.db, action, args.json),
        Command::Normalize { project, prefix } => {
            normalize::run(&args.db, &project, &prefix, args.json)
        }
        Command::Book { action } => book::run(&args.db, action, &args.lang, args.json),
        Command::Gen { action } => generate::run(action, args.json),
        Command::Risk { action } => risk::run(action, args.json),
        Command::Rank { action } => rank::run(&args.db, action, args.json),
        Command::Orchestrate { action } => orchestrate::run(&args.db, action, args.json),
        Command::Merge { action } => merge::run(&args.db, action, args.json),
        Command::Cascade { action } => cascade::run(&args.db, action, args.json),
        Command::Check { action } => check::run(&args.db, action, &args.lang, args.json).await,
        Command::Bibliography { action } => bibliography::run(&args.db, action, args.json),
        Command::Facts { action } => facts::run(&args.db, action, args.json),
        Command::Prisma { action } => prisma::run(&args.db, action, args.json),
        Command::Verify { action } => verify::run(&args.db, action, args.json).await,
        Command::Review { action } => review::run(&args.db, action, args.json).await,
        Command::Doctor => doctor::run(args.json),
        Command::Provider { action } => provider::run(action, args.json).await,
        Command::Config { action } => config::run(action, args.json),
        Command::Import { action } => import::run(&args.db, action, args.json),
        Command::Migrate {
            src,
            name,
            working_lang,
            institution,
            track,
            embed,
            provider,
            model,
        } => {
            migrate::from_args(
                &args.db,
                src,
                name,
                working_lang,
                institution,
                track,
                embed,
                provider,
                model,
                args.json,
            )
            .await
        }
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
            strategy,
            provider,
            model,
        } => {
            embed::run_classify(
                &args.db,
                &project,
                &prefix,
                slots.as_deref(),
                strategy.as_deref(),
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
