//! Recursive directory import.

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::Connection;

use crate::detect;
use crate::import::{ImportOutcome, import_file};

/// Walk `src_dir` recursively and import every supported file. The relative
/// path under `src_dir` is mirrored under `target_prefix` inside the project's
/// working tree, with non-markdown extensions rewritten to `.md`.
///
/// Returns one [`ImportOutcome`] **per file attempted**, including failures
/// (with `success = false` and `error = Some(...)`). Files with unsupported
/// extensions are silently skipped. The function only returns an outer
/// `Err` when the walk itself cannot proceed (e.g. `src_dir` isn't a
/// directory). Callers should iterate the result and decide what to do
/// with failures.
pub fn import_dir(
    conn: &Connection,
    project_id: &str,
    src_dir: &Path,
    target_prefix: &str,
    author: &str,
    message: &str,
    lang: Option<&str>,
) -> Result<Vec<ImportOutcome>> {
    if !src_dir.is_dir() {
        return Err(anyhow::anyhow!("not a directory: {}", src_dir.display()));
    }
    let mut out = Vec::new();
    for entry in walkdir::WalkDir::new(src_dir)
        .into_iter()
        .filter_map(std::result::Result::ok)
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let Some(format) = detect::from_path(path) else {
            tracing::debug!(file = %path.display(), "skipping unsupported file");
            continue;
        };
        let rel = path
            .strip_prefix(src_dir)
            .with_context(|| format!("strip prefix from {}", path.display()))?;
        let target = mirror_target(rel, target_prefix, format);
        match import_file(conn, project_id, path, &target, author, message, lang) {
            Ok(o) => out.push(o),
            Err(e) => {
                tracing::warn!(file = %path.display(), error = %e, "import failed");
                out.push(ImportOutcome {
                    source: path.display().to_string(),
                    target_path: target,
                    format: format.as_str().to_owned(),
                    bytes_in: std::fs::metadata(path)
                        .map(|m| m.len() as usize)
                        .unwrap_or(0),
                    bytes_out: 0,
                    commit_sha: String::new(),
                    success: false,
                    error: Some(e.to_string()),
                });
            }
        }
    }
    Ok(out)
}

/// Map a relative source path to a working-tree path, replacing `.docx`/`.pdf`
/// with `.md` since the import always stores markdown.
fn mirror_target(rel: &Path, prefix: &str, format: crate::detect::Format) -> String {
    use crate::detect::Format;
    let mut s = rel.to_string_lossy().replace('\\', "/");
    if !matches!(format, Format::Markdown) {
        if let Some((stem, _ext)) = s.rsplit_once('.') {
            s = format!("{stem}.md");
        }
    }
    if prefix.is_empty() {
        s
    } else {
        let prefix = prefix.trim_end_matches('/');
        format!("{prefix}/{s}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentic_core::db::open_in_memory;
    use agentic_core::project::{ProjectKind, create as create_project};
    use agentic_core::worktree;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn imports_nested_markdown_files() {
        let conn = open_in_memory().unwrap();
        let pid = create_project(&conn, "P", ProjectKind::Thesis, "en", None).unwrap();

        let dir = tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let mut a = std::fs::File::create(dir.path().join("intro.md")).unwrap();
        writeln!(a, "# A").unwrap();
        let mut b = std::fs::File::create(dir.path().join("sub/ch2.md")).unwrap();
        writeln!(b, "# B").unwrap();

        let outcomes = import_dir(
            &conn,
            &pid,
            dir.path(),
            "proposal",
            "tester",
            "bulk",
            Some("en"),
        )
        .unwrap();
        assert_eq!(outcomes.len(), 2);
        assert!(outcomes.iter().all(|o| o.success));

        let entries = worktree::list(&conn, &pid, "").unwrap();
        let paths: Vec<&str> = entries.iter().map(|(p, _)| p.as_str()).collect();
        assert!(paths.contains(&"proposal/intro.md"));
        assert!(paths.contains(&"proposal/sub/ch2.md"));
    }

    #[test]
    fn surfaces_failures_in_outcomes() {
        // Project id "ID" does not exist; every import will fail with
        // "project not found: ID". The walk must still return one
        // outcome per file, all with success=false.
        let conn = open_in_memory().unwrap();
        let dir = tempdir().unwrap();
        std::fs::File::create(dir.path().join("a.md")).unwrap();
        std::fs::File::create(dir.path().join("b.md")).unwrap();

        let outcomes = import_dir(&conn, "ID", dir.path(), "p", "t", "m", Some("en")).unwrap();
        assert_eq!(outcomes.len(), 2);
        assert!(outcomes.iter().all(|o| !o.success));
        assert!(outcomes.iter().all(|o| {
            o.error
                .as_deref()
                .unwrap_or("")
                .contains("project not found")
        }));
    }

    #[test]
    fn target_extension_rewrites_to_md_for_docx_and_pdf() {
        use crate::detect::Format;
        assert_eq!(
            mirror_target(Path::new("a.docx"), "proposal", Format::Docx),
            "proposal/a.md"
        );
        assert_eq!(
            mirror_target(Path::new("sub/b.pdf"), "", Format::Pdf),
            "sub/b.md"
        );
        assert_eq!(
            mirror_target(Path::new("c.md"), "proposal", Format::Markdown),
            "proposal/c.md"
        );
    }
}
