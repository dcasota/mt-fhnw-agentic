//! `agentic merge` — source-merge utilities.
//!
//! Codifies the previously-manual dimension merge: read every
//! `Dimension_NN_*.md` source (NN = 01..11) under a prefix, normalise each so
//! the merged document has exactly one top-level `#` heading per dimension
//! (stray secondary `# ` lines inside a dimension are demoted to `## `), and
//! write the concatenated compendium back to the content store in one commit.

use std::path::Path;

use anyhow::Result;
use serde_json::json;

use agentic_core::worktree;

use crate::cli::MergeAction;

/// Author recorded on the merge commit.
const AUTHOR: &str = "agentic merge dimensions";

pub fn run(db_path: &Path, action: MergeAction, json_out: bool) -> Result<()> {
    let conn = agentic_core::db::open(db_path)?;
    match action {
        MergeAction::Dimensions {
            project,
            prefix,
            out,
        } => merge_dimensions(&conn, &project, &prefix, &out, json_out),
    }
}

/// `Dimension_NN_*.md` basename → its numeric index (01..11), else None.
fn dimension_index(path: &str) -> Option<u32> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let rest = name.strip_prefix("Dimension_")?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    let n: u32 = digits.parse().ok()?;
    (1..=11).contains(&n).then_some(n)
}

/// Normalise one dimension's markdown so it contributes exactly one top-level
/// `# ` heading: the FIRST `# ` line is kept as the dimension H1; any later
/// `# ` line is a stray secondary H1 and is demoted to `## `. Deeper headings
/// (`##`, `###`, …) and fenced code are left untouched.
#[must_use]
fn normalise_h1(text: &str) -> String {
    let mut seen_h1 = false;
    let mut in_fence = false;
    let mut out: Vec<String> = Vec::new();
    for ln in text.lines() {
        if ln.trim_start().starts_with("```") {
            in_fence = !in_fence;
            out.push(ln.to_string());
            continue;
        }
        // An H1 is exactly one `#` then a space (not `## `).
        let is_h1 = !in_fence && ln.starts_with("# ");
        if is_h1 {
            if seen_h1 {
                // Stray secondary H1 inside a dimension → demote to H2.
                out.push(format!("#{ln}"));
            } else {
                seen_h1 = true;
                out.push(ln.to_string());
            }
        } else {
            out.push(ln.to_string());
        }
    }
    out.join("\n")
}

fn merge_dimensions(
    conn: &rusqlite::Connection,
    project: &str,
    prefix: &str,
    out: &str,
    json_out: bool,
) -> Result<()> {
    // Collect dimension sources keyed by their numeric index, in numeric order.
    let mut dims: Vec<(u32, String)> = worktree::list(conn, project, prefix)?
        .into_iter()
        .filter_map(|(path, _sha)| dimension_index(&path).map(|n| (n, path)))
        .collect();
    dims.sort_by_key(|(n, _)| *n);

    if dims.is_empty() {
        anyhow::bail!("no Dimension_NN_*.md sources found under prefix '{prefix}'");
    }

    let intro = "*Merged compendium — the eleven dimension sources assembled into one \
document; the English source-of-truth.*";
    let mut doc = String::new();
    doc.push_str(intro);
    doc.push('\n');

    let mut chapters = 0usize;
    for (_n, path) in &dims {
        let blob = worktree::read_at(conn, project, path)?;
        let text = String::from_utf8_lossy(&blob.content);
        let normalised = normalise_h1(text.trim_end());
        doc.push('\n');
        doc.push_str(&normalised);
        doc.push('\n');
        chapters += 1;
    }

    let msg = format!("merge {chapters} dimension sources into {out}");
    let commit_sha = worktree::put_at(
        conn,
        project,
        out,
        doc.as_bytes(),
        "text/markdown",
        Some("en"),
        AUTHOR,
        &msg,
    )?;

    if json_out {
        println!(
            "{}",
            json!({ "commit": commit_sha, "out": out, "chapters": chapters })
        );
    } else {
        println!("{commit_sha}  merged {chapters} dimension(s) into {out}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{dimension_index, normalise_h1};

    #[test]
    fn first_h1_kept_secondary_demoted() {
        // The first `# ` is the chapter H1 (kept); a later `# ` becomes `## `.
        let md = "# Dimension 03\n\nProse.\n\n# Stray\n\nMore.\n";
        let out = normalise_h1(md);
        assert!(out.starts_with("# Dimension 03\n"));
        assert!(out.contains("\n## Stray\n"));
        // Exactly one top-level `# ` line survives.
        assert_eq!(out.lines().filter(|l| l.starts_with("# ")).count(), 1);
    }

    #[test]
    fn existing_subheadings_untouched() {
        // `##`/`###` are not H1s and must be left exactly as-is.
        let md = "# Title\n\n## Section\n\n### Sub\n";
        assert_eq!(normalise_h1(md), "# Title\n\n## Section\n\n### Sub");
    }

    #[test]
    fn h1_in_fence_ignored() {
        // A `# ` line inside a fence is code, not a heading.
        let md = "# Title\n\n```\n# not a heading\n```\n";
        let out = normalise_h1(md);
        assert!(out.contains("\n# not a heading\n"));
        assert_eq!(out.lines().filter(|l| l.starts_with("# ")).count(), 2);
    }

    #[test]
    fn dimension_index_parses_range() {
        assert_eq!(
            dimension_index("out/sources/Dimension_01_Intro.md"),
            Some(1)
        );
        assert_eq!(dimension_index("Dimension_11_Last.md"), Some(11));
        assert_eq!(dimension_index("Dimension_12_OutOfRange.md"), None);
        assert_eq!(dimension_index("Dimensions_merged_EN.md"), None);
    }
}
