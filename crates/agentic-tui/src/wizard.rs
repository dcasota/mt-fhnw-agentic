//! M0 onboarding wizard — public surface.
//!
//! Provides:
//!   * the [`WizardArgs`] trait that the CLI `init` command passes in,
//!   * [`launch`] which materialises the ratatui app and returns its outcome,
//!   * state types via the [`state`] submodule (used by tests + caller).

pub mod app;
pub mod state;

use std::io::IsTerminal;

use anyhow::Result;
use rusqlite::Connection;

pub use app::WizardOutcome;
pub use state::{LANGS, PROJECT_KINDS, PROVIDERS, Step, WizardState};

/// Minimum field surface needed by the wizard launcher.
pub trait WizardArgs {
    fn mode(&self) -> &str;
    fn working_lang(&self) -> &str;
    fn institution(&self) -> Option<&str>;
}

/// Run the wizard. Returns the outcome.
///
/// Behaviour:
///   * Without a TTY (CI, piped stdin/stdout) returns
///     [`WizardOutcome::Cancelled`] immediately — the caller is expected to
///     fall back to `--no-wizard` and print guidance.
///   * If `resume` is true, an existing draft is loaded; otherwise a fresh
///     state seeded from `args` is used.
pub fn launch<A>(conn: &Connection, args: &A, resume: bool) -> Result<WizardOutcome>
where
    A: WizardArgs + ?Sized,
{
    if !std::io::stdout().is_terminal() || !std::io::stdin().is_terminal() {
        return Ok(WizardOutcome::Cancelled);
    }

    let state = if resume {
        state::load_draft(conn)?.unwrap_or_else(|| {
            WizardState::new_with_defaults(args.working_lang(), args.institution())
        })
    } else {
        WizardState::new_with_defaults(args.working_lang(), args.institution())
    };

    let app = app::App::new(state);
    app.run(conn)
}

/// Convenience: drop any saved draft (e.g. after a successful confirm).
pub fn discard_draft(conn: &Connection) -> Result<()> {
    state::delete_draft(conn)
}
