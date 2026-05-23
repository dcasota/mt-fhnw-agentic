//! `agentic-checks` — integrity checkers.
//!
//! Public surface:
//!   * [`self_check`]          — structural integrity (DB-level, expanded in P1)
//!   * [`writing_quality`]     — 46 AI-typical patterns + FHNW style rules
//!   * [`citation_tracker`]    — APA7 in-text vs. reference-list cross-check
//!   * [`contamination`]       — Crossref / OpenAlex / Semantic Scholar verification

#![warn(clippy::pedantic)]
#![allow(clippy::missing_errors_doc, clippy::module_name_repetitions, dead_code)]

pub mod citation_tracker;
pub mod contamination;
pub mod self_check;
pub mod tree_integrity;
pub mod writing_quality;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Severity {
    Info,
    Warn,
    Error,
    Blocking,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Verdict {
    Pass,
    Warn,
    Fail,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub category: String,
    pub severity: Severity,
    pub message: String,
    /// Optional location: `<path>:<line>` or `<table>.<column>`.
    pub location: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckReport {
    pub checker: String,
    pub verdict: Verdict,
    pub findings: Vec<Finding>,
}

impl CheckReport {
    #[must_use]
    pub fn new(checker: impl Into<String>, findings: Vec<Finding>) -> Self {
        let verdict = verdict_from(&findings);
        Self {
            checker: checker.into(),
            verdict,
            findings,
        }
    }
}

#[must_use]
pub fn verdict_from(findings: &[Finding]) -> Verdict {
    if findings
        .iter()
        .any(|f| matches!(f.severity, Severity::Blocking | Severity::Error))
    {
        Verdict::Fail
    } else if findings
        .iter()
        .any(|f| matches!(f.severity, Severity::Warn))
    {
        Verdict::Warn
    } else {
        Verdict::Pass
    }
}
