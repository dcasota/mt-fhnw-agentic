//! `agentic migrate <src>` — bootstrap a project from a legacy directory.

use std::path::{Path, PathBuf};

use anyhow::Result;

use agentic_import::{EmbedOutcome, MigrationReport, embed_project_blobs, migrate_legacy_repo};

pub async fn run(
    db_path: &Path,
    src: &Path,
    name: Option<&str>,
    working_lang: &str,
    institution: Option<&str>,
    track: Option<&str>,
    embed: bool,
    provider: Option<&str>,
    model: Option<&str>,
    json: bool,
) -> Result<()> {
    let conn = agentic_core::db::open(db_path)?;
    let project_name = name.map(str::to_owned).unwrap_or_else(|| derive_name(src));
    let report = migrate_legacy_repo(&conn, src, &project_name, working_lang, institution, track)?;

    let embed_outcomes = if embed {
        Some(embed_project_blobs(&conn, &report.project_id, "", provider, model, true).await?)
    } else {
        None
    };

    report_out(&report, embed_outcomes.as_deref(), json)
}

fn derive_name(src: &Path) -> String {
    src.file_name()
        .and_then(|n| n.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("Migrated thesis")
        .to_owned()
}

pub fn from_args(
    db: &Path,
    src: PathBuf,
    name: Option<String>,
    working_lang: String,
    institution: Option<String>,
    track: Option<String>,
    embed: bool,
    provider: Option<String>,
    model: Option<String>,
    json: bool,
) -> impl std::future::Future<Output = Result<()>> {
    let db_path = db.to_path_buf();
    async move {
        run(
            &db_path,
            &src,
            name.as_deref(),
            &working_lang,
            institution.as_deref(),
            track.as_deref(),
            embed,
            provider.as_deref(),
            model.as_deref(),
            json,
        )
        .await
    }
}

fn report_out(r: &MigrationReport, embed: Option<&[EmbedOutcome]>, json: bool) -> Result<()> {
    if json {
        let payload = serde_json::json!({
            "report": r,
            "embed": embed,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }
    println!("Project {} created ({}).", r.project_id, r.project_name);
    println!("Source: {}", r.source);
    println!("Imported {} file(s):", r.imported.len());
    for (bucket, count) in &r.bucket_counts {
        println!("  {bucket:<14} {count}");
    }
    if !r.skipped.is_empty() {
        println!("\nSkipped {} entry/entries:", r.skipped.len());
        for s in r.skipped.iter().take(10) {
            println!("  - {} ({})", s.path, s.reason);
        }
        if r.skipped.len() > 10 {
            println!("  … and {} more.", r.skipped.len() - 10);
        }
    }
    if let Some(emb) = embed {
        let (done, skipped) =
            emb.iter().fold(
                (0, 0),
                |(d, s), o| if o.skipped { (d, s + 1) } else { (d + 1, s) },
            );
        println!("\nEmbeddings: {done} new, {skipped} reused/skipped.");
    }
    println!("\nNext: agentic project status --id {}", r.project_id);
    Ok(())
}
