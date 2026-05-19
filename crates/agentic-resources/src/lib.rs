//! `agentic-resources` — embedded resources (FHNW thesis template,
//! Typst stylesheet, i18n seed strings, ADR/protocol seed text).
//! P0 ships placeholders; real resources land in P4/P5.

#![warn(missing_debug_implementations)]
#![warn(rust_2018_idioms)]

/// Bytes of the FHNW MAS thesis Word template (placeholder for now).
pub const FHNW_THESIS_TEMPLATE_DOCX: &[u8] = &[];

/// Typst stylesheet for thesis PDF output (placeholder).
pub const TYPST_THESIS_TEMPLATE_TYP: &str = "";
