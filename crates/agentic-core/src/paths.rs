//! Canonical working-tree paths — the single source of truth for where content
//! lives in the project tree.
//!
//! Historically these were string literals (`"out/sources/"`, …) scattered
//! across the CLI, the gate suite and the cascade. Centralising them here is the
//! groundwork for the **`out/` deprecation** (see the deprecation ADR): the
//! `out/` working-tree prefix is being retired, and concentrating every
//! reference in one place means the retirement is a change *here*, not a hunt
//! across ~20 call sites. **Phase 0 keeps the values byte-identical** (no folder
//! is renamed, no behaviour changes); later phases flip them at the marked
//! deprecation point while history keeps its `out/` paths intact.

/// Prefix under which authored source content (dimensions, campaigns, project
/// sheets, norms chapters, frontmatter, the merged doc, bibliography) is stored
/// in the working tree. Used as the gate-suite scan prefix and by the cascade.
pub const SOURCES_PREFIX: &str = "out/sources/";

/// The book manifest consumed by `book build` / `cascade`.
pub const MANIFEST_PATH: &str = "out/book_manifest.json";

/// The merged eleven-dimension compendium written by `merge dimensions`.
pub const MERGED_DOC: &str = "out/sources/Dimensions_merged_EN.md";

/// The reviewer-calibration gold set read by the `calibration` gate (ADR-0044).
pub const CALIBRATION_GOLD: &str = "out/calibration_gold.json";
