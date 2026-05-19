//! Walk a project's HEAD tree and pull every markdown chapter into memory.

use anyhow::{Result, anyhow};
use rusqlite::Connection;

use agentic_core::content::blob;
use agentic_core::worktree;

#[derive(Debug, Clone)]
pub struct Chapter {
    /// Path inside the project (e.g. `thesis-draft/ch-02.md`).
    pub path: String,
    /// Decoded UTF-8 body.
    pub body: String,
    /// Lowercase language tag (`en|de|fr|it|rm|hi`) if the blob declares one.
    pub lang: Option<String>,
}

/// Collect all markdown chapters under the given prefix, sorted by path.
///
/// `prefix` is matched against the working-tree path. Empty string means
/// "everything". Non-markdown entries are silently skipped.
pub fn collect_chapters(conn: &Connection, project_id: &str, prefix: &str) -> Result<Vec<Chapter>> {
    let entries =
        worktree::list(conn, project_id, prefix).map_err(|e| anyhow!("list working tree: {e}"))?;
    let mut out = Vec::with_capacity(entries.len());
    for (path, sha) in entries {
        if !is_markdown_path(&path) {
            continue;
        }
        let b = blob::get_blob(conn, &sha).map_err(|e| anyhow!("load blob {sha}: {e}"))?;
        if b.encoding != "utf-8" {
            // Skip non-text blobs — DOCX/PDF can't include them inline yet.
            continue;
        }
        let body = String::from_utf8(b.content)
            .map_err(|_| anyhow!("blob {sha} declares utf-8 but isn't"))?;
        out.push(Chapter {
            path,
            body,
            lang: b.lang,
        });
    }
    Ok(out)
}

fn is_markdown_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".md") || lower.ends_with(".markdown")
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentic_core::db::open_in_memory;
    use agentic_core::project::{ProjectKind, create as create_project};
    use agentic_core::worktree::put_at;

    fn fixture() -> (Connection, String) {
        let conn = open_in_memory().unwrap();
        let pid = create_project(&conn, "P", ProjectKind::Thesis, "de", None).unwrap();
        put_at(
            &conn,
            &pid,
            "thesis-draft/ch-01.md",
            b"# Intro\nBody A.",
            "text/markdown",
            Some("de"),
            "u",
            "ch1",
        )
        .unwrap();
        put_at(
            &conn,
            &pid,
            "thesis-draft/ch-02.md",
            b"# Theory\nBody B.",
            "text/markdown",
            Some("de"),
            "u",
            "ch2",
        )
        .unwrap();
        put_at(
            &conn,
            &pid,
            "notes/scratch.txt",
            b"not markdown",
            "text/plain",
            None,
            "u",
            "scratch",
        )
        .unwrap();
        (conn, pid)
    }

    #[test]
    fn collects_markdown_only_in_path_order() {
        let (conn, pid) = fixture();
        let chapters = collect_chapters(&conn, &pid, "").unwrap();
        assert_eq!(chapters.len(), 2);
        assert_eq!(chapters[0].path, "thesis-draft/ch-01.md");
        assert_eq!(chapters[1].path, "thesis-draft/ch-02.md");
        assert!(chapters[0].body.contains("Intro"));
    }

    #[test]
    fn prefix_filters_to_subtree() {
        let (conn, pid) = fixture();
        let chapters = collect_chapters(&conn, &pid, "thesis-draft/").unwrap();
        assert_eq!(chapters.len(), 2);
        let other = collect_chapters(&conn, &pid, "notes/").unwrap();
        assert_eq!(other.len(), 0);
    }
}
