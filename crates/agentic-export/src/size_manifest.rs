//! Per-figure size hint manifest (Round V iter-10, 2026-06-03).
//!
//! The book renderer's `DocxBlock::Image` arm decides every raster's `<wp:extent
//! cx="..."/>` by a path-based heuristic: figspec-emitter stems (`gov_*`,
//! `reg_*`, `iso*`, `pop_*`, …) render at 6 in (FIGURE bucket), everything else
//! falls back to the 4-in `DEFAULT_EMBED_W_EMU` default (OTHER bucket). That
//! catches 41 of the 78 reference FIGURE drawings but leaves the remaining 32
//! `image*.png` FIGURE entries / 52 `image*.png` OTHER entries indistinguishable
//! by path bytes — the FIGURE / OTHER split between them is editorial in the
//! reference book.
//!
//! This manifest closes that gap. A `sizes.toml` file alongside the rasters
//! (`<figdir>/sizes.toml`) lists per-filename width hints (inches) lifted from
//! the reference book's `<wp:extent>` values. The renderer reads it once at
//! `Ctx` construction, then for every image it tries `sizes[basename(path)]`
//! before falling through to the path heuristic.
//!
//! ## Format
//!
//! Minimal TOML subset:
//!
//! ```toml
//! [sizes]
//! "image6.png" = 5.315   # OTHER
//! "image13.png" = 5.9055 # FIGURE
//! "gov_eu.png" = 5.9055  # FIGURE
//! ```
//!
//! Only `[sizes]` lines of the form `"<key>" = <float>` are recognised. Comments
//! (after `#`), blank lines, and other table headers are ignored. The renderer
//! pulls in no new dependency to parse this; the format is intentionally narrow.
//!
//! ## Missing entries
//!
//! If `basename(path)` is absent from the manifest (or no manifest file
//! exists), the iter-9 path-prefix heuristic decides — preserving behaviour
//! for every other book that does not ship a manifest.

use std::collections::HashMap;
use std::path::Path;

/// A loaded per-filename width hint table. The empty manifest is the "no
/// override anywhere" state and is what every book except AI-Norms uses.
#[derive(Debug, Default, Clone)]
pub struct SizeManifest {
    sizes: HashMap<String, f32>,
}

impl SizeManifest {
    /// Empty manifest — every lookup misses.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            sizes: HashMap::new(),
        }
    }

    /// Try to load `sizes.toml` from `<figdir>/` first, then walk a small
    /// fallback chain of project-relative locations. Returns an empty manifest
    /// when no candidate is found — the renderer treats that case the same as
    /// "no manifest, use path heuristic".
    ///
    /// Round V iter-10b (2026-06-03): the cascade materialises rasters into
    /// `figdir` (a scratch tmp dir) but does NOT copy `sizes.toml` alongside.
    /// The manifest lives at `specs/figures/raster/ai_norms/sizes.toml` in the
    /// project worktree. Search that path next so the override actually fires
    /// at cascade time.
    #[must_use]
    pub fn load_from_figdir(figdir: &Path) -> Self {
        // 1. The original lookup — `<figdir>/sizes.toml` (still authoritative
        //    when the cascade pre-copies the manifest into the scratch dir).
        let primary = figdir.join("sizes.toml");
        if let Ok(text) = std::fs::read_to_string(&primary) {
            let m = Self::parse(&text);
            if !m.sizes.is_empty() {
                return m;
            }
        }
        // 2. Project-relative fallback: walk up from cwd / figdir looking for
        //    `specs/figures/raster/ai_norms/sizes.toml`. The book/book.rs caller
        //    constructs Ctx before changing cwd, so cwd is still the project root.
        let candidates = [
            std::path::PathBuf::from("specs/figures/raster/ai_norms/sizes.toml"),
            std::env::current_dir()
                .ok()
                .map(|d| d.join("specs/figures/raster/ai_norms/sizes.toml"))
                .unwrap_or_default(),
        ];
        for cand in &candidates {
            if let Ok(text) = std::fs::read_to_string(cand) {
                let m = Self::parse(&text);
                if !m.sizes.is_empty() {
                    return m;
                }
            }
        }
        Self::empty()
    }

    /// Parse the minimal `[sizes]` table format described in the module docs.
    /// Lines outside the `[sizes]` table are ignored, as are comments and
    /// blanks. Malformed entries are silently dropped — the manifest is a
    /// best-effort optimisation, never a correctness gate.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        let mut sizes: HashMap<String, f32> = HashMap::new();
        let mut in_sizes_table = false;
        for raw_line in text.lines() {
            // Strip trailing comments (a `#` outside a quoted string starts
            // a comment). The keys are quoted, so any `#` inside the quotes
            // is part of the filename — but the manifest only ever stores
            // filenames, which never contain `#`. A naive split is therefore
            // safe.
            let line = raw_line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            if let Some(table) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                in_sizes_table = table.trim() == "sizes";
                continue;
            }
            if !in_sizes_table {
                continue;
            }
            // Expect: "<key>" = <float>
            let Some((lhs, rhs)) = line.split_once('=') else {
                continue;
            };
            let key = lhs.trim().trim_matches('"');
            if key.is_empty() {
                continue;
            }
            let Ok(value) = rhs.trim().parse::<f32>() else {
                continue;
            };
            if !value.is_finite() || value <= 0.0 {
                continue;
            }
            sizes.insert(key.to_string(), value);
        }
        Self { sizes }
    }

    /// Look up the width hint for an image path. The lookup key is the
    /// **basename** (the trailing segment after the last `/` or `\`), so the
    /// manifest does not need to track the figdir layout. Returns `None` when
    /// the basename is absent from the manifest.
    #[must_use]
    pub fn lookup(&self, image_path: &str) -> Option<f32> {
        let basename = image_path.rsplit(['/', '\\']).next().unwrap_or(image_path);
        self.sizes.get(basename).copied()
    }

    /// Total entry count — useful for tests and for the boot-gate journal
    /// entry that records "manifest loaded with N entries".
    #[must_use]
    pub fn len(&self) -> usize {
        self.sizes.len()
    }

    /// Convenience alias for `len() == 0`.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.sizes.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_extracts_quoted_keys_and_float_values() {
        let text = r#"
[sizes]
"image1.png" = 5.9055   # FIGURE
"image2.png" = 4.0      # OTHER
"icon_tip.png" = 0.1654 # ICON
"#;
        let m = SizeManifest::parse(text);
        assert_eq!(m.len(), 3);
        assert!((m.lookup("image1.png").unwrap() - 5.9055).abs() < 1e-4);
        assert!((m.lookup("image2.png").unwrap() - 4.0).abs() < 1e-4);
        assert!((m.lookup("icon_tip.png").unwrap() - 0.1654).abs() < 1e-4);
    }

    #[test]
    fn parse_ignores_non_sizes_tables() {
        let text = r#"
[other]
"image1.png" = 99.0

[sizes]
"image1.png" = 5.0
"#;
        let m = SizeManifest::parse(text);
        assert_eq!(m.lookup("image1.png"), Some(5.0));
    }

    #[test]
    fn parse_ignores_blank_and_comment_lines() {
        let text = r#"
# top-level comment

[sizes]

# blank above; comment-only line
"image1.png" = 6.0
"#;
        let m = SizeManifest::parse(text);
        assert_eq!(m.len(), 1);
    }

    #[test]
    fn lookup_strips_path_to_basename() {
        let text = r#"
[sizes]
"gov_eu.png" = 6.0
"#;
        let m = SizeManifest::parse(text);
        assert_eq!(m.lookup("gov_eu.png"), Some(6.0));
        assert_eq!(m.lookup("figures/governance/gov_eu.png"), Some(6.0));
        assert_eq!(m.lookup(r"figures\governance\gov_eu.png"), Some(6.0));
    }

    #[test]
    fn empty_manifest_misses_everything() {
        let m = SizeManifest::empty();
        assert!(m.is_empty());
        assert_eq!(m.lookup("anything.png"), None);
    }

    #[test]
    fn parse_skips_malformed_lines() {
        let text = r#"
[sizes]
"image1.png" = not-a-number
"image2.png" = -3.0
"image3.png" =
malformed_no_quotes = 5.0
"image4.png" = 6.0
"#;
        let m = SizeManifest::parse(text);
        // image4.png is well-formed; malformed_no_quotes is accepted too
        // (we don't require quotes — the trim_matches just strips them when
        // present). image1/2/3 are dropped (non-numeric / negative / empty).
        assert_eq!(m.lookup("image4.png"), Some(6.0));
        assert_eq!(m.lookup("image1.png"), None);
        assert_eq!(m.lookup("image2.png"), None);
        assert_eq!(m.lookup("image3.png"), None);
        assert_eq!(m.lookup("malformed_no_quotes"), Some(5.0));
    }
}
