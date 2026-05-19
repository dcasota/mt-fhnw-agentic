//! Single-file import: extract → wrap as markdown → `put_at` in the working tree.

use std::path::Path;

use anyhow::{Context, Result, anyhow};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use agentic_core::worktree;

use crate::detect::{self, Format};
use crate::{docx, markdown, pdf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportOutcome {
    pub source: String,
    pub target_path: String,
    pub format: String,
    pub bytes_in: usize,
    pub bytes_out: usize,
    pub commit_sha: String,
}

/// Import a single file into the project's working tree.
///
/// `target_path` is the path inside the working tree (e.g.
/// `proposal/skizze.md`). Non-markdown formats are extracted to plain text
/// and wrapped with an H1 heading from the file stem so the resulting blob
/// is valid markdown.
pub fn import_file(
    conn: &Connection,
    project_id: &str,
    src: &Path,
    target_path: &str,
    author: &str,
    message: &str,
    lang: Option<&str>,
) -> Result<ImportOutcome> {
    let format = detect::from_path(src)
        .ok_or_else(|| anyhow!("unsupported file extension: {}", src.display()))?;

    let raw_meta = std::fs::metadata(src).with_context(|| format!("stat {}", src.display()))?;
    let bytes_in = raw_meta.len() as usize;

    let markdown_text = match format {
        Format::Markdown => markdown::extract_text(src)?,
        Format::Docx => to_markdown(src, &docx::extract_text(src)?),
        Format::Pdf => to_markdown(src, &pdf::extract_text(src)?),
    };

    let bytes_out = markdown_text.len();

    let commit_sha = worktree::put_at(
        conn,
        project_id,
        target_path,
        markdown_text.as_bytes(),
        "text/markdown",
        lang,
        author,
        message,
    )
    .map_err(|e| anyhow!("put_at: {e}"))?;

    Ok(ImportOutcome {
        source: src.display().to_string(),
        target_path: target_path.to_owned(),
        format: format.as_str().to_owned(),
        bytes_in,
        bytes_out,
        commit_sha,
    })
}

/// Wrap extracted plain text with a markdown H1 (file stem) and normalised
/// paragraph spacing.
fn to_markdown(src: &Path, text: &str) -> String {
    let stem = src
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Imported document");
    let mut out = String::with_capacity(text.len() + stem.len() + 16);
    out.push_str("# ");
    out.push_str(stem);
    out.push_str("\n\n");
    // Collapse 3+ newlines down to 2 (standard md paragraph break).
    let mut blank_streak = 0;
    for line in text.split('\n') {
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            blank_streak += 1;
            if blank_streak <= 1 {
                out.push('\n');
            }
        } else {
            blank_streak = 0;
            out.push_str(trimmed);
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentic_core::db::open_in_memory;
    use agentic_core::project::{ProjectKind, create as create_project};
    use std::io::Write;
    use tempfile::tempdir;

    fn fixture() -> (Connection, String, tempfile::TempDir) {
        let conn = open_in_memory().unwrap();
        let pid = create_project(&conn, "P", ProjectKind::Thesis, "en", None).unwrap();
        let dir = tempdir().unwrap();
        (conn, pid, dir)
    }

    #[test]
    fn import_markdown_passthrough() {
        let (conn, pid, dir) = fixture();
        let path = dir.path().join("intro.md");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "# Intro\n\nHello.").unwrap();
        drop(f);

        let outcome = import_file(
            &conn,
            &pid,
            &path,
            "proposal/intro.md",
            "tester",
            "import md",
            Some("en"),
        )
        .unwrap();
        assert_eq!(outcome.format, "markdown");
        assert!(outcome.bytes_out > 0);

        let blob = worktree::read_at(&conn, &pid, "proposal/intro.md").unwrap();
        assert_eq!(blob.mime, "text/markdown");
        assert!(String::from_utf8(blob.content).unwrap().contains("Intro"));
    }

    #[test]
    fn unsupported_extension_errors() {
        let (conn, pid, dir) = fixture();
        let path = dir.path().join("nope.xyz");
        std::fs::write(&path, b"x").unwrap();
        let err = import_file(&conn, &pid, &path, "x.md", "u", "m", None).unwrap_err();
        assert!(err.to_string().contains("unsupported"));
    }

    #[test]
    fn to_markdown_inserts_heading_and_normalises_blank_lines() {
        let path = Path::new("/tmp/skizze.txt");
        let raw = "line one\n\n\n\nline two\n";
        let md = to_markdown(path, raw);
        assert!(md.starts_with("# skizze\n\n"));
        // No run of 3+ blank lines.
        assert!(!md.contains("\n\n\n"));
        assert!(md.contains("line one"));
        assert!(md.contains("line two"));
    }
}
