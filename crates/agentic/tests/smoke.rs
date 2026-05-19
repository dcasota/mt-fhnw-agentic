//! End-to-end smoke test: opens a file-backed DB, creates a project, fetches it back.

use agentic_core::project;
use tempfile::TempDir;

#[test]
fn baseline_lifecycle_through_file_path() {
    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("thesis.db");
    let conn = agentic_core::db::open(&db).unwrap();
    let id = project::create(&conn, "Untitled", project::ProjectKind::Thesis, "de", None).unwrap();
    let fetched = project::get(&conn, &id).unwrap();
    assert_eq!(fetched.name, "Untitled");
    assert_eq!(fetched.working_lang, "de");
}
