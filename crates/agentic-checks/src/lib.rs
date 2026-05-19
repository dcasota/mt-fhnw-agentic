//! agentic-checks — integrity checkers.
//!
//! P0 ships only the public surface. Real implementations come in P2.

#![warn(clippy::pedantic)]
#![allow(dead_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Verdict {
    Pass,
    Warn,
    Fail,
}
