//! `agentic-import` — proposal import (markdown / DOCX / PDF) into a project's
//! working tree.
//!
//! Two entry points: [`import::import_file`] for a single file, and
//! [`walk::import_dir`] for recursive directory import. Both convert the input
//! to markdown, then call [`agentic_core::worktree::put_at`] so the import
//! lands as a normal blob + commit on the project's `main` branch.

#![warn(missing_debug_implementations)]
#![warn(rust_2018_idioms)]

pub mod detect;
pub mod docx;
pub mod import;
pub mod markdown;
pub mod pdf;
pub mod walk;

pub use import::{ImportOutcome, import_file};
pub use walk::import_dir;
