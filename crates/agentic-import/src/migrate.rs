//! Migrate a legacy FACTORYAI / interim-presentation directory into a fresh
//! `thesis.db`.
//!
//! The mapping is intentionally simple: each well-known top-level directory in
//! the source repo is mirrored under a stable prefix inside the project's
//! working tree. Files that don't match a known bucket land under
//! `proposal/`. Hidden / build / vendor directories are skipped entirely.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Result, anyhow};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use agentic_core::project::{ProjectKind, create as create_project};

use crate::detect::{self, Format};
use crate::import::{ImportOutcome, import_file};

/// Top-level mapping rules.
///
/// `prefix` is the working-tree prefix where files from `src_dir` will land.
/// Order matters: the first match wins.
const MAPPINGS: &[(&str, &str)] = &[
    ("thesis-draft", "thesis-draft"),
    ("specs", "proposal/specs"),
    ("docs", "references"),
    ("iterations", "iterations"),
    ("code", "code"),
    ("code/notebooks", "code/notebooks"),
];

/// Directories we never descend into.
const SKIP_DIRS: &[&str] = &[
    ".git",
    ".github",
    ".claude",
    ".cursor",
    ".factory",
    ".gemini",
    ".codex",
    ".vscode",
    ".idea",
    "node_modules",
    "target",
    "build",
    "dist",
    "venv",
    ".venv",
    "__pycache__",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationReport {
    pub project_id: String,
    pub project_name: String,
    pub source: String,
    pub imported: Vec<ImportOutcome>,
    pub skipped: Vec<SkippedEntry>,
    pub bucket_counts: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkippedEntry {
    pub path: String,
    pub reason: String,
}

/// Drive a full migration from `src_dir` into the project. Creates the
/// project row first; subsequent imports land on its default branch.
///
/// `working_lang` is forwarded to `agentic_core::project::create`.
/// `institution` and `track` are stashed in `projects.metadata_json` if
/// either is non-empty.
pub fn migrate_legacy_repo(
    conn: &Connection,
    src_dir: &Path,
    project_name: &str,
    working_lang: &str,
    institution: Option<&str>,
    track: Option<&str>,
) -> Result<MigrationReport> {
    if !src_dir.is_dir() {
        return Err(anyhow!("not a directory: {}", src_dir.display()));
    }

    let project_id = create_project(conn, project_name, ProjectKind::Thesis, working_lang, None)
        .map_err(|e| anyhow!("create project: {e}"))?;

    if institution.is_some() || track.is_some() {
        let meta = serde_json::json!({
            "institution": institution.unwrap_or(""),
            "track": track.unwrap_or(""),
            "migrated_from": src_dir.display().to_string(),
        });
        conn.execute(
            "UPDATE projects SET metadata_json = ?1 WHERE id = ?2",
            rusqlite::params![meta.to_string(), project_id],
        )?;
    }

    let mut imported = Vec::new();
    let mut skipped = Vec::new();
    let mut bucket_counts: BTreeMap<String, usize> = BTreeMap::new();

    for entry in walkdir::WalkDir::new(src_dir)
        .into_iter()
        .filter_entry(|e| !is_skipped_dir(e.path()))
    {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                skipped.push(SkippedEntry {
                    path: e
                        .path()
                        .map(Path::display)
                        .map(|d| d.to_string())
                        .unwrap_or_default(),
                    reason: format!("walkdir error: {e}"),
                });
                continue;
            }
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let abs = entry.path();
        let rel = match abs.strip_prefix(src_dir) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let Some(format) = detect::from_path(abs) else {
            // Not an importable type — silently skipped for the report.
            skipped.push(SkippedEntry {
                path: rel.display().to_string(),
                reason: "unsupported extension".into(),
            });
            continue;
        };

        let target = map_target(rel, format);
        let bucket = bucket_for(rel);
        *bucket_counts.entry(bucket.clone()).or_insert(0) += 1;

        match import_file(
            conn,
            &project_id,
            abs,
            &target,
            "migrate",
            &format!("migrate from {}", src_dir.display()),
            Some(working_lang),
        ) {
            Ok(o) => imported.push(o),
            Err(e) => skipped.push(SkippedEntry {
                path: rel.display().to_string(),
                reason: format!("import failed: {e}"),
            }),
        }
    }

    // Journal what we did so the first thing the user sees is a record of
    // the migration.
    let _ = conn.execute(
        "INSERT INTO journal_entries
            (project_id, entry_no, actor, action_type, description, reasoning)
         VALUES (?1, 1, 'migrate', 'Migrate', ?2, ?3)",
        rusqlite::params![
            project_id,
            format!(
                "Migrated from {} ({} files)",
                src_dir.display(),
                imported.len()
            ),
            format!(
                "buckets: {}",
                bucket_counts
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ],
    );

    Ok(MigrationReport {
        project_id,
        project_name: project_name.to_owned(),
        source: src_dir.display().to_string(),
        imported,
        skipped,
        bucket_counts,
    })
}

/// Return the working-tree prefix for a relative source path, plus the
/// final filename. DOCX/PDF are normalised to `.md` (since they always
/// land as markdown after extraction).
fn map_target(rel: &Path, format: Format) -> String {
    let mut s = rel.to_string_lossy().replace('\\', "/");

    // Apply the longest matching prefix.
    let mut prefix_replacement: Option<(&str, &str)> = None;
    for (src_prefix, tgt_prefix) in MAPPINGS {
        let with_slash = format!("{src_prefix}/");
        if s.starts_with(&with_slash) || s == *src_prefix {
            if prefix_replacement.map_or(true, |(p, _)| src_prefix.len() > p.len()) {
                prefix_replacement = Some((src_prefix, tgt_prefix));
            }
        }
    }

    if let Some((src_prefix, tgt_prefix)) = prefix_replacement {
        if let Some(rest) = s.strip_prefix(&format!("{src_prefix}/")) {
            s = format!("{tgt_prefix}/{rest}");
        } else if s == src_prefix {
            s = tgt_prefix.to_string();
        }
    } else if !s.contains('/') {
        // Root-level file (e.g. README.md, proposal.docx). Stash under proposal/.
        s = format!("proposal/{s}");
    }

    if !matches!(format, Format::Markdown) {
        if let Some((stem, _ext)) = s.rsplit_once('.') {
            s = format!("{stem}.md");
        }
    }

    s
}

/// Bucket label for the report — the first matching MAPPING source prefix,
/// or `"root"` for top-level files, or `"other"` for unrecognised paths.
fn bucket_for(rel: &Path) -> String {
    let s = rel.to_string_lossy().replace('\\', "/");
    for (src_prefix, _) in MAPPINGS {
        if s.starts_with(&format!("{src_prefix}/")) || s == *src_prefix {
            return (*src_prefix).to_string();
        }
    }
    if s.contains('/') {
        "other".into()
    } else {
        "root".into()
    }
}

fn is_skipped_dir(path: &Path) -> bool {
    if !path.is_dir() {
        return false;
    }
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    SKIP_DIRS.contains(&name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentic_core::db::open_in_memory;
    use agentic_core::worktree;
    use std::io::Write;
    use tempfile::tempdir;

    fn make(dir: &Path, rel: &str, body: &str) -> std::path::PathBuf {
        let p = dir.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let mut f = std::fs::File::create(&p).unwrap();
        writeln!(f, "{body}").unwrap();
        p
    }

    #[test]
    fn map_target_routes_known_top_level_dirs() {
        assert_eq!(
            map_target(Path::new("thesis-draft/ch-01.md"), Format::Markdown),
            "thesis-draft/ch-01.md"
        );
        assert_eq!(
            map_target(Path::new("specs/proposal.docx"), Format::Docx),
            "proposal/specs/proposal.md"
        );
        assert_eq!(
            map_target(Path::new("docs/refs/paper.pdf"), Format::Pdf),
            "references/refs/paper.md"
        );
        assert_eq!(
            map_target(Path::new("iterations/legacy/v1/notes.md"), Format::Markdown),
            "iterations/legacy/v1/notes.md"
        );
    }

    #[test]
    fn map_target_stashes_root_files_under_proposal() {
        assert_eq!(
            map_target(Path::new("README.md"), Format::Markdown),
            "proposal/README.md"
        );
        assert_eq!(
            map_target(Path::new("draft.docx"), Format::Docx),
            "proposal/draft.md"
        );
    }

    #[test]
    fn map_target_preserves_unknown_subtrees() {
        assert_eq!(
            map_target(Path::new("misc/foo.md"), Format::Markdown),
            "misc/foo.md"
        );
    }

    #[test]
    fn migrates_fixture_repo() {
        let conn = open_in_memory().unwrap();
        let src = tempdir().unwrap();
        make(src.path(), "thesis-draft/ch-01.md", "# Intro");
        make(src.path(), "thesis-draft/ch-02.md", "# Theory");
        make(src.path(), "specs/notes.md", "# Spec");
        make(src.path(), "docs/refs/r1.md", "# Ref");
        make(src.path(), "README.md", "# top");
        make(src.path(), "unsupported.bin", "binary");
        // Hidden + vendored dirs must be skipped entirely.
        make(src.path(), ".git/HEAD", "ref: refs/heads/main");
        make(src.path(), "node_modules/foo/index.js", "module");

        let report = migrate_legacy_repo(
            &conn,
            src.path(),
            "MAS Thesis",
            "de",
            Some("fhnw-mas"),
            None,
        )
        .unwrap();

        // 5 importable files (the 4 .md + README.md). Unsupported and
        // skipped-dir files must NOT appear.
        assert_eq!(report.imported.len(), 5);
        assert!(
            !report
                .imported
                .iter()
                .any(|o| o.target_path.starts_with(".git/"))
        );
        assert!(
            !report
                .imported
                .iter()
                .any(|o| o.target_path.contains("node_modules"))
        );

        // Bucket accounting.
        assert_eq!(report.bucket_counts.get("thesis-draft").copied(), Some(2));
        assert_eq!(report.bucket_counts.get("specs").copied(), Some(1));
        assert_eq!(report.bucket_counts.get("docs").copied(), Some(1));
        assert_eq!(report.bucket_counts.get("root").copied(), Some(1));

        // Working tree contains the rewritten paths.
        let entries = worktree::list(&conn, &report.project_id, "").unwrap();
        let paths: Vec<&str> = entries.iter().map(|(p, _)| p.as_str()).collect();
        assert!(paths.contains(&"thesis-draft/ch-01.md"));
        assert!(paths.contains(&"proposal/specs/notes.md"));
        assert!(paths.contains(&"references/refs/r1.md"));
        assert!(paths.contains(&"proposal/README.md"));
    }

    #[test]
    fn errors_on_non_directory() {
        let conn = open_in_memory().unwrap();
        let dir = tempdir().unwrap();
        let f = make(dir.path(), "x.md", "x");
        let err = migrate_legacy_repo(&conn, &f, "x", "en", None, None).unwrap_err();
        assert!(err.to_string().contains("not a directory"));
    }
}
