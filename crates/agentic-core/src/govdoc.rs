//! Shared constants for generated governance docs (ADR-0047 R10).
//!
//! Every mission-control / subagent role has one canonical body under
//! `specs/agents/<role>.canonical.md` (a `name:`/`description:` header, then
//! `---`, then the tool-agnostic body). The generator (`agentic gen agent-defs`)
//! emits each into `.claude/agents/<role>.md` and `.factory/droids/<role>.md`;
//! the drift gate (`check docs`) asserts each generated body equals the
//! canonical. Generator and gate share these constants so they stay in lock-step.

/// Directory of canonical agent bodies (`<role>.canonical.md`).
pub const AGENTS_CANONICAL_DIR: &str = "specs/agents";

/// Marker separating a generated file's per-tool front-matter from the verbatim
/// canonical body. Everything after it must equal the role's canonical body.
pub const GENERATED_MARKER: &str = "<!-- GENERATED from specs/agents/<role>.canonical.md — do not edit; run `agentic gen agent-defs` -->";

/// The two per-tool generated paths for a role. (The Gemini runtime uses the
/// AGENTS.md pointer model and has no generated droid file.)
#[must_use]
pub fn generated_paths(role: &str) -> [String; 2] {
    [
        format!(".claude/agents/{role}.md"),
        format!(".factory/droids/{role}.md"),
    ]
}
