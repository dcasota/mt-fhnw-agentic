//! Working tree: maps path strings to blob SHAs, snapshotted per commit.
//!
//! Each project has a HEAD ref (branch `main` by default) whose commit's
//! tree holds the current path→blob mapping. [`put_at`] stages a blob at
//! a path and creates a new commit; [`list`] walks the HEAD tree.
//!
//! For P1 we use a single flat tree where entry `name` IS the full path
//! (e.g. `thesis-draft/chapter-02.md`). Nested sub-trees can be added in
//! a later phase without breaking this API — the path-based lookup
//! remains identical.

use rusqlite::Connection;

use crate::{
    Error, Result,
    content::{
        blob::{self, Blob, put_blob},
        commit::{self, Commit, put_commit},
        refs::{self, RefKind, set_ref},
        tree::{EntryKind, Tree, TreeEntry, get_tree, put_tree},
    },
    project,
};

const DEFAULT_BRANCH: &str = "main";

fn project_default_branch(project_id: &str) -> String {
    format!("{project_id}/{DEFAULT_BRANCH}")
}

/// Stage `content` at `path` in the project's working tree, creating a new
/// commit on the project's default branch. Returns the new commit SHA.
pub fn put_at(
    conn: &Connection,
    project_id: &str,
    path: &str,
    content: &[u8],
    mime: &str,
    lang: Option<&str>,
    author: &str,
    message: &str,
) -> Result<String> {
    let project = project::get(conn, project_id)?;

    let blob_sha = put_blob(conn, content, mime, lang)?;
    let branch_name = project_default_branch(project_id);

    // Resolve the parent commit + its tree (if any).
    let parent_commit_sha = refs::get_ref(conn, &branch_name).ok().map(|r| r.commit_sha);
    let mut entries: Vec<TreeEntry> = if let Some(parent_sha) = &parent_commit_sha {
        let parent_commit = commit::get_commit(conn, parent_sha)?;
        get_tree(conn, &parent_commit.tree)?.entries
    } else {
        Vec::new()
    };

    // Insert-or-replace by name.
    if let Some(existing) = entries.iter_mut().find(|e| e.name == path) {
        existing.target = blob_sha.clone();
    } else {
        entries.push(TreeEntry {
            name: path.to_owned(),
            kind: EntryKind::Blob,
            target: blob_sha.clone(),
            mode: "100644".into(),
        });
    }

    let tree_sha = put_tree(conn, entries)?;
    let new_commit = put_commit(
        conn,
        &tree_sha,
        parent_commit_sha.as_deref(),
        None,
        author,
        "human",
        None,
        None,
        message,
    )?;
    set_ref(conn, &branch_name, RefKind::Branch, &new_commit)?;

    // Keep project metadata in sync.
    conn.execute(
        "UPDATE projects SET head_ref = ?1 WHERE id = ?2",
        rusqlite::params![branch_name, project.id],
    )?;

    Ok(new_commit)
}

/// Stage many `(path, content, mime, lang)` blobs in a single commit. Existing
/// entries with the same path are replaced; all other parent entries are
/// preserved. This is the bulk equivalent of [`put_at`] and is what makes the
/// content store a faithful, one-commit mirror of an on-disk source tree.
/// Returns the new commit SHA.
pub fn put_many(
    conn: &Connection,
    project_id: &str,
    files: &[(String, Vec<u8>, String, Option<String>)],
    author: &str,
    message: &str,
    replace: bool,
) -> Result<String> {
    let project = project::get(conn, project_id)?;
    let branch_name = project_default_branch(project_id);

    let parent_commit_sha = refs::get_ref(conn, &branch_name).ok().map(|r| r.commit_sha);
    // When `replace` is set, HEAD's tree becomes EXACTLY the staged set (a clean
    // snapshot); the parent commit is still linked so history is preserved.
    // Otherwise the staged files are merged onto the parent tree.
    let mut entries: Vec<TreeEntry> = if replace {
        Vec::new()
    } else if let Some(parent_sha) = &parent_commit_sha {
        let parent_commit = commit::get_commit(conn, parent_sha)?;
        get_tree(conn, &parent_commit.tree)?.entries
    } else {
        Vec::new()
    };

    // Index existing entries by name so each stage is a cheap replace-or-append
    // instead of the O(n) scan `put_at` does per file.
    let mut idx: std::collections::HashMap<String, usize> = entries
        .iter()
        .enumerate()
        .map(|(i, e)| (e.name.clone(), i))
        .collect();

    for (path, content, mime, lang) in files {
        let blob_sha = put_blob(conn, content, mime, lang.as_deref())?;
        if let Some(&i) = idx.get(path) {
            entries[i].target = blob_sha;
        } else {
            idx.insert(path.clone(), entries.len());
            entries.push(TreeEntry {
                name: path.clone(),
                kind: EntryKind::Blob,
                target: blob_sha,
                mode: "100644".into(),
            });
        }
    }

    let tree_sha = put_tree(conn, entries)?;
    let new_commit = put_commit(
        conn,
        &tree_sha,
        parent_commit_sha.as_deref(),
        None,
        author,
        "human",
        None,
        None,
        message,
    )?;
    set_ref(conn, &branch_name, RefKind::Branch, &new_commit)?;
    conn.execute(
        "UPDATE projects SET head_ref = ?1 WHERE id = ?2",
        rusqlite::params![branch_name, project.id],
    )?;
    Ok(new_commit)
}

/// Read the blob currently staged at `path` in the project's working tree.
pub fn read_at(conn: &Connection, project_id: &str, path: &str) -> Result<Blob> {
    let tree = head_tree(conn, project_id)?
        .ok_or_else(|| Error::InvalidInput(format!("project {project_id} has no commits yet")))?;
    let entry = tree
        .entries
        .into_iter()
        .find(|e| e.name == path)
        .ok_or_else(|| {
            Error::InvalidInput(format!(
                "path {path} not in project {project_id} working tree"
            ))
        })?;
    blob::get_blob(conn, &entry.target)
}

/// List paths in the project's working tree under `prefix`. Empty prefix lists all.
pub fn list(conn: &Connection, project_id: &str, prefix: &str) -> Result<Vec<(String, String)>> {
    let Some(tree) = head_tree(conn, project_id)? else {
        return Ok(Vec::new());
    };
    let mut out: Vec<(String, String)> = tree
        .entries
        .into_iter()
        .filter(|e| matches!(e.kind, EntryKind::Blob) && e.name.starts_with(prefix))
        .map(|e| (e.name, e.target))
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// Result of comparing the DB working tree against the on-disk files.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ReconcileReport {
    /// On-disk files present in the DB with identical bytes.
    pub matched: usize,
    /// On-disk files present in the DB but with DIFFERENT bytes (integrity drift).
    pub modified: Vec<String>,
    /// On-disk files (under prefix) NOT present in the DB (un-ingested).
    pub extra_on_disk: Vec<String>,
    /// DB paths NOT materialised on disk (expected after a checkout-only setup).
    pub missing_on_disk: Vec<String>,
}

/// Compare the project's DB working tree against the files on disk under `root`
/// (filtered by `prefix`). Dot-directories, `target/`, `node_modules/`,
/// `__pycache__/`, and the database files themselves are skipped. This is the
/// boot-time integrity check: any on-disk file that differs from its DB blob is
/// `modified` (drift); a file in the DB but not on disk is merely
/// `missing_on_disk` (not materialised).
pub fn reconcile(
    conn: &Connection,
    project_id: &str,
    prefix: &str,
    root: &std::path::Path,
) -> Result<ReconcileReport> {
    use std::collections::{HashMap, HashSet};
    let db: HashMap<String, String> = list(conn, project_id, prefix)?.into_iter().collect();

    // Walk the on-disk tree under root, collecting forward-slash relative paths.
    let mut disk_paths: Vec<String> = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_dir() {
                if name.starts_with('.')
                    || name == "target"
                    || name == "node_modules"
                    || name == "__pycache__"
                {
                    continue;
                }
                stack.push(path);
            } else if let Ok(rel) = path.strip_prefix(root) {
                let rel = rel.to_string_lossy().replace('\\', "/");
                if rel == ".project-id" || rel.starts_with("thesis.db") {
                    continue;
                }
                if prefix.is_empty() || rel.starts_with(prefix) {
                    disk_paths.push(rel);
                }
            }
        }
    }

    let disk_set: HashSet<&String> = disk_paths.iter().collect();
    let (mut modified, mut extra_on_disk) = (Vec::new(), Vec::new());
    let mut matched = 0usize;
    for rel in &disk_paths {
        match db.get(rel) {
            Some(sha) => {
                let blob = blob::get_blob(conn, sha)?;
                let native = rel.replace('/', std::path::MAIN_SEPARATOR_STR);
                let disk_bytes = std::fs::read(root.join(native)).unwrap_or_default();
                if disk_bytes == blob.content {
                    matched += 1;
                } else {
                    modified.push(rel.clone());
                }
            }
            None => extra_on_disk.push(rel.clone()),
        }
    }
    let mut missing_on_disk: Vec<String> = db
        .keys()
        .filter(|p| !disk_set.contains(*p))
        .cloned()
        .collect();
    modified.sort();
    extra_on_disk.sort();
    missing_on_disk.sort();
    Ok(ReconcileReport {
        matched,
        modified,
        extra_on_disk,
        missing_on_disk,
    })
}

/// Resolve the project's HEAD commit's tree (None if no commits yet).
pub fn head_tree(conn: &Connection, project_id: &str) -> Result<Option<Tree>> {
    let branch_name = project_default_branch(project_id);
    match refs::get_ref(conn, &branch_name) {
        Ok(r) => {
            let c = commit::get_commit(conn, &r.commit_sha)?;
            Ok(Some(get_tree(conn, &c.tree)?))
        }
        Err(Error::RefNotFound(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Return the HEAD commit object for a project (None if no commits yet).
pub fn head_commit(conn: &Connection, project_id: &str) -> Result<Option<Commit>> {
    let branch_name = project_default_branch(project_id);
    match refs::get_ref(conn, &branch_name) {
        Ok(r) => Ok(Some(commit::get_commit(conn, &r.commit_sha)?)),
        Err(Error::RefNotFound(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        db::open_in_memory,
        project::{ProjectKind, create as create_project},
    };
    use pretty_assertions::assert_eq;

    fn fixture_project(conn: &Connection) -> String {
        create_project(conn, "P", ProjectKind::Thesis, "en", None).unwrap()
    }

    #[test]
    fn put_at_creates_commit_and_advances_head() {
        let conn = open_in_memory().unwrap();
        let pid = fixture_project(&conn);
        assert!(head_commit(&conn, &pid).unwrap().is_none());

        let c1 = put_at(
            &conn,
            &pid,
            "thesis-draft/ch-01.md",
            b"# Intro\n",
            "text/markdown",
            Some("de"),
            "u",
            "init",
        )
        .unwrap();
        let head = head_commit(&conn, &pid).unwrap().unwrap();
        assert_eq!(head.sha256, c1);
        assert_eq!(head.parent, None);

        let c2 = put_at(
            &conn,
            &pid,
            "thesis-draft/ch-02.md",
            b"# Theory\n",
            "text/markdown",
            Some("de"),
            "u",
            "add ch2",
        )
        .unwrap();
        let head2 = head_commit(&conn, &pid).unwrap().unwrap();
        assert_eq!(head2.sha256, c2);
        assert_eq!(head2.parent.as_deref(), Some(c1.as_str()));
    }

    #[test]
    fn read_at_returns_latest_content() {
        let conn = open_in_memory().unwrap();
        let pid = fixture_project(&conn);
        put_at(&conn, &pid, "x.md", b"v1", "text/markdown", None, "u", "v1").unwrap();
        put_at(&conn, &pid, "x.md", b"v2", "text/markdown", None, "u", "v2").unwrap();
        let blob = read_at(&conn, &pid, "x.md").unwrap();
        assert_eq!(blob.content, b"v2");
    }

    #[test]
    fn list_filters_by_prefix() {
        let conn = open_in_memory().unwrap();
        let pid = fixture_project(&conn);
        put_at(
            &conn,
            &pid,
            "thesis-draft/ch-01.md",
            b"a",
            "text/markdown",
            None,
            "u",
            "1",
        )
        .unwrap();
        put_at(
            &conn,
            &pid,
            "thesis-draft/ch-02.md",
            b"b",
            "text/markdown",
            None,
            "u",
            "2",
        )
        .unwrap();
        put_at(
            &conn,
            &pid,
            "studentnotes/note-01.md",
            b"c",
            "text/markdown",
            None,
            "u",
            "3",
        )
        .unwrap();

        let drafts = list(&conn, &pid, "thesis-draft/").unwrap();
        assert_eq!(drafts.len(), 2);
        assert_eq!(drafts[0].0, "thesis-draft/ch-01.md");
        assert_eq!(drafts[1].0, "thesis-draft/ch-02.md");

        let all = list(&conn, &pid, "").unwrap();
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn read_at_missing_path_errors() {
        let conn = open_in_memory().unwrap();
        let pid = fixture_project(&conn);
        put_at(
            &conn,
            &pid,
            "a.md",
            b"x",
            "text/markdown",
            None,
            "u",
            "init",
        )
        .unwrap();
        let err = read_at(&conn, &pid, "missing.md").unwrap_err();
        assert!(matches!(err, Error::InvalidInput(_)));
    }

    #[test]
    fn head_tree_empty_for_fresh_project() {
        let conn = open_in_memory().unwrap();
        let pid = fixture_project(&conn);
        assert!(head_tree(&conn, &pid).unwrap().is_none());
    }
}
