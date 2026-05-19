//! `agentic-import` — PDF/DOCX proposal import + recursive folder classification.
//!
//! P0 ships text-extraction helpers; the LLM-assisted FHNW Projektskizze
//! mapping and clustering live in P5/P6.

#![warn(missing_debug_implementations)]
#![warn(rust_2018_idioms)]

pub mod docx;
pub mod pdf;
