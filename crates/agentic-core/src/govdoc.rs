//! Shared constants for generated governance docs (ADR-0047 R10).
//!
//! The per-tool mission-control agent-defs are generated from one canonical
//! body. The generator (`agentic gen agent-defs`) and the drift gate
//! (`check docs`) share these constants so the body comparison stays in lock-step.

/// The single canonical mission-control body (tool-agnostic, no front-matter).
pub const CANONICAL_MISSION_CONTROL: &str = "specs/mission-control.canonical.md";

/// Marker separating a generated file's per-tool front-matter from the verbatim
/// canonical body. Everything after it must equal the canonical body.
pub const GENERATED_MARKER: &str = "<!-- GENERATED from specs/mission-control.canonical.md — do not edit; run `agentic gen agent-defs` -->";

/// The per-tool agent-defs generated from the canonical body. (The Gemini runtime
/// uses the AGENTS.md pointer model and has no generated droid file.)
pub const GENERATED_AGENT_DEFS: &[&str] = &[
    ".claude/agents/mission-control.md",
    ".factory/droids/mission-control.md",
];
