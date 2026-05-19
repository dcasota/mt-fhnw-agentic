//! `agentic-import` — proposal import (markdown / DOCX / PDF) into a project's
//! working tree.
//!
//! Two entry points: [`import::import_file`] for a single file, and
//! [`walk::import_dir`] for recursive directory import. Both convert the input
//! to markdown, then call [`agentic_core::worktree::put_at`] so the import
//! lands as a normal blob + commit on the project's `main` branch.

#![warn(missing_debug_implementations)]
#![warn(rust_2018_idioms)]

pub mod classify;
pub mod detect;
pub mod docx;
pub mod embed;
pub mod import;
pub mod markdown;
pub mod migrate;
pub mod pdf;
pub mod walk;

pub use classify::{ChapterAssignment, Slot, SlotMatch, classify_project, default_slots};
pub use embed::{EmbedOutcome, embed_project_blobs};
pub use import::{ImportOutcome, import_file};
pub use migrate::{MigrationReport, SkippedEntry, migrate_legacy_repo};
pub use walk::import_dir;
