//! agentic-core — storage layer.
//!
//! See [`db`] for migration handling, [`content`] for blob/tree/commit/refs,
//! [`project`] for project metadata, [`journal`] / [`passport`] for the
//! append-only history surfaces.

#![warn(clippy::pedantic)]
#![allow(clippy::missing_errors_doc, clippy::module_name_repetitions)]

pub mod audit;
pub mod audit_profile;
pub mod authz;
pub mod content;
pub mod db;
pub mod embeddings;
pub mod error;
pub mod govdoc;
pub mod i18n;
pub mod inbox;
pub mod journal;
pub mod orchestrate;
pub mod passport;
pub mod paths;
pub mod profile;
pub mod profiles;
pub mod project;
pub mod rank_summary;
pub mod review;
pub mod signing;
pub mod tombstone;
pub mod translation_cache;
pub mod worktree;

pub use error::{Error, Result};

/// Re-export `rusqlite::Connection` so callers don't need a direct dep.
pub use rusqlite::Connection;
