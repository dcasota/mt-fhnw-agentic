//! `agentic book` — render professional DOCX books (Rust book engine) and audit
//! render quality against the previous iteration. Replaces the Python
//! bookkit/build_book. Figures via `agentic-figures`, layout via
//! `agentic-export::book`.

use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

use agentic_core::worktree;
use agentic_export::book::{BookMeta, render_book};

use crate::cli::BookAction;

#[derive(Debug, Deserialize)]
struct Manifest {
    books: Vec<BookSpec>,
}
#[derive(Debug, Deserialize)]
struct BookSpec {
    key: String,
    title: String,
    #[serde(default)]
    subtitle: String,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    context: Option<String>,
    #[serde(default)]
    description: String,
    #[serde(default)]
    dedication: Option<String>,
    #[serde(default)]
    epigraph: Option<String>,
    #[serde(default)]
    epigraph_by: Option<String>,
    #[serde(default)]
    disclaimer: Option<String>,
    /// Imprint lines on the title page (version, place + date); one per line.
    #[serde(default)]
    imprint: Option<String>,
    /// Master-thesis numbering profile (ADR-0045): number body chapters,
    /// keep only true front/back-matter unnumbered.
    #[serde(default)]
    thesis_profile: bool,
    /// Companion-paper profile (ADR-0045, bookkit B): skip the book chrome
    /// (title page / disclaimer / inscription); plain title + contents only.
    #[serde(default)]
    companion: bool,
    #[serde(default)]
    index_terms: Vec<String>,
    /// Optional per-book chrome language (en|de|fr|it|rm|hi). Overrides the
    /// global `--lang`; absent → fall back to `--lang` then "en".
    #[serde(default)]
    lang: Option<String>,
    /// Optional content-image asset prefix in the working tree (e.g.
    /// `out/sources/norms/media`). When set, every blob under it is materialised
    /// (by basename) into the per-book scratch dir so `![caption](name.png)`
    /// image references resolve. Opt-in: books without it are unaffected.
    #[serde(default)]
    assets: Option<String>,
    chapters: Vec<String>,
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

pub fn run(db_path: &Path, action: BookAction, lang: &str, json_out: bool) -> Result<()> {
    match action {
        BookAction::Build {
            project,
            manifest,
            out,
            only,
        } => build(
            db_path,
            &project,
            &manifest,
            &out,
            only.as_deref(),
            lang,
            json_out,
        ),
        BookAction::Audit { current, previous } => audit(&current, previous.as_deref(), json_out),
        BookAction::Finalize { path, pdf } => finalize(&path, pdf, json_out),
    }
}

/// Collect `.docx` targets: a single file, or every `.docx` in a directory.
fn collect_docx(path: &Path) -> Result<Vec<PathBuf>> {
    if path.is_dir() {
        let mut v: Vec<PathBuf> = std::fs::read_dir(path)
            .with_context(|| format!("read dir {}", path.display()))?
            .filter_map(std::result::Result::ok)
            .map(|e| e.path())
            .filter(|p| {
                p.extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("docx"))
            })
            .collect();
        v.sort();
        Ok(v)
    } else {
        Ok(vec![path.to_path_buf()])
    }
}

/// `book finalize` command: finalize the target(s) through Word. Explicit
/// invocation, so a missing Word is reported as an error.
fn finalize(path: &Path, pdf: bool, json_out: bool) -> Result<()> {
    let docs = collect_docx(path)?;
    if docs.is_empty() {
        anyhow::bail!("no .docx found at {}", path.display());
    }
    let results = finalize_docs(&docs, pdf)?;
    if json_out {
        let arr: Vec<_> = results
            .iter()
            .map(|(p, r)| serde_json::json!({"docx": p, "result": r}))
            .collect();
        println!("{}", serde_json::json!({"finalized": arr}));
    } else {
        for (p, r) in &results {
            println!("  + finalized {p} ({r})");
        }
        println!(
            "Finalized {} document(s) via Microsoft Word.",
            results.len()
        );
    }
    Ok(())
}

/// Drive Microsoft Word (COM, via PowerShell) over all `docs` in ONE Word
/// session — update every field class, repaginate (real page numbers), hide
/// field codes, save — so each opens with no refresh prompt. Per-document
/// errors are reported, not aborted. Mirrors `book_build/finalize.ps1`.
#[cfg(windows)]
fn finalize_docs(docs: &[PathBuf], pdf: bool) -> Result<Vec<(String, String)>> {
    use std::process::Command;
    let mut items: Vec<String> = Vec::new();
    for d in docs {
        let abs = std::fs::canonicalize(d)
            .with_context(|| format!("resolve {}", d.display()))?
            .to_string_lossy()
            .replace(r"\\?\", ""); // strip the verbatim prefix Word rejects
        items.push(format!("'{}'", abs.replace('\'', "''"))); // single-quote escaped
    }
    let arr = items.join(","); // no trailing comma — PowerShell rejects @('x',)
    let pdf_block = if pdf {
        "if($pdf){ $d.SaveAs([ref]([System.IO.Path]::ChangeExtension($pth,'pdf')),[ref]17) }"
    } else {
        ""
    };
    let script = format!(
        r#"$ErrorActionPreference='Stop'
$paths=@({arr})
$pdf=${pdf}
$w = New-Object -ComObject Word.Application
$w.Visible=$false; $w.DisplayAlerts=0
$w.Options.ConfirmConversions=$false; $w.Options.UpdateLinksAtOpen=$false
try {{
  foreach ($pth in $paths) {{
    try {{
      $d = $w.Documents.Open($pth, $false, $false, $false)
      $d.Fields.Update() | Out-Null
      foreach ($tof in $d.TablesOfFigures) {{ $tof.Update() }}
      foreach ($toc in $d.TablesOfContents) {{ $toc.Update() }}
      foreach ($ix in $d.Indexes) {{ $ix.Update() }}
      try {{ $d.ActiveWindow.View.ShowFieldCodes=$false }} catch {{}}
      $d.Repaginate()
      $pages=$d.ComputeStatistics(2)
      $d.Save()
      {pdf_block}
      $d.Close($false)
      Write-Output ("{{0}}`tpages={{1}}" -f $pth, $pages)
    }} catch {{
      Write-Output ("{{0}}`tERROR {{1}}" -f $pth, $_.Exception.Message)
    }}
  }}
}} finally {{
  $w.Quit()
  [System.Runtime.InteropServices.Marshal]::ReleaseComObject($w) | Out-Null
}}"#,
        pdf = if pdf { "true" } else { "false" }
    );
    let out = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .context("launch Word via powershell (is Microsoft Word installed?)")?;
    if !out.status.success() {
        anyhow::bail!(
            "Word finalize failed (is Microsoft Word installed?): {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    Ok(stdout
        .lines()
        .filter_map(|l| {
            l.split_once('\t')
                .map(|(p, r)| (p.to_string(), r.to_string()))
        })
        .collect())
}

#[cfg(not(windows))]
fn finalize_docs(_docs: &[PathBuf], _pdf: bool) -> Result<Vec<(String, String)>> {
    anyhow::bail!(
        "Word finalize requires Microsoft Word on Windows; elsewhere the rendered \
         .docx carries updateFields so Word refreshes on open"
    )
}

fn build(
    db_path: &Path,
    project: &str,
    manifest_path: &Path,
    out: &Path,
    only: Option<&str>,
    lang: &str,
    json_out: bool,
) -> Result<()> {
    let conn = agentic_core::db::open(db_path)?;
    let text = std::fs::read_to_string(manifest_path)
        .with_context(|| format!("reading manifest {}", manifest_path.display()))?;
    let manifest: Manifest = serde_json::from_str(&text).context("parsing manifest JSON")?;
    std::fs::create_dir_all(out)?;

    let mut report = Vec::new();
    let mut built = 0usize;
    let mut built_docs: Vec<PathBuf> = Vec::new();
    for spec in &manifest.books {
        if let Some(k) = only {
            if spec.key != k {
                continue;
            }
        }
        // Per-book scratch dir in the system temp — created and DELETED within
        // this processing step so the output dir never accumulates intermediates
        // and a crash leaves at most one book's scratch (not a global wipe).
        let work = std::env::temp_dir().join(format!(
            "agentic_book_{}_{}",
            sanitize(&spec.key),
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&work);
        std::fs::create_dir_all(&work)?;

        let result = build_one(&conn, project, spec, &work, out, lang);
        let _ = std::fs::remove_dir_all(&work); // always clean this step's scratch
        match result {
            Ok((figs, bytes)) => {
                built += 1;
                built_docs.push(out.join(format!("{}.docx", spec.key)));
                report.push(serde_json::json!({
                    "key": spec.key, "chapters": spec.chapters.len(), "figures": figs, "docx_bytes": bytes
                }));
                if !json_out {
                    println!(
                        "  + {}.docx  ({} figures, {} chapters)",
                        spec.key,
                        figs,
                        spec.chapters.len()
                    );
                }
            }
            Err(e) => eprintln!("  ! {} FAILED: {e}", spec.key),
        }
    }
    // Render report (used by `book audit` to compare iterations).
    std::fs::write(
        out.join("_render_report.json"),
        serde_json::to_string_pretty(&serde_json::json!({ "books": report }))?,
    )?;
    // Always finalize through Microsoft Word so the books open with no "update
    // fields" prompt (TOC / List of Figures/Tables / Index populated, real page
    // numbers). Generation itself never fails on this, but an unmet prerequisite
    // (Word absent / non-Windows) is WARNED loudly to stderr — never silently
    // skipped — because the resulting docx will then need a field refresh on open.
    if !built_docs.is_empty() {
        match finalize_docs(&built_docs, false) {
            Ok(res) => {
                let failed: Vec<_> = res.iter().filter(|(_, r)| r.starts_with("ERROR")).collect();
                if !json_out {
                    println!(
                        "Finalized {}/{} book(s) via Microsoft Word.",
                        res.len() - failed.len(),
                        res.len()
                    );
                }
                for (p, r) in failed {
                    eprintln!(
                        "WARNING: Word finalize failed for {p}: {r} — this book will need a field refresh on open."
                    );
                }
            }
            Err(e) => eprintln!(
                "WARNING: prerequisite not met — Microsoft Word unavailable, so the {} \
                 rendered book(s) were NOT finalized and WILL prompt to update fields \
                 (TOC / lists / index) on open. Finalize on Windows with Word installed \
                 (e.g. `agentic book finalize --path {}`). Cause: {e}",
                built_docs.len(),
                out.display()
            ),
        }
    }
    if json_out {
        println!(
            "{}",
            serde_json::json!({ "built": built, "out": out.display().to_string() })
        );
    } else {
        println!(
            "Built {built} book(s) into {} (no intermediates left)",
            out.display()
        );
    }
    Ok(())
}

fn build_one(
    conn: &rusqlite::Connection,
    project: &str,
    spec: &BookSpec,
    work: &Path,
    out: &Path,
    lang: &str,
) -> Result<(usize, u64)> {
    // Pre-render the three admonition icons (gen_icons port) into the work dir
    // so the book renderer can embed icon_{tip,note,warning}.png in callouts.
    for kind in ["tip", "note", "warning"] {
        let json = format!(
            "{{\"id\":\"icon_{kind}\",\"type\":\"icon\",\"data\":{{\"variant\":\"{kind}\"}}}}"
        );
        let _ = agentic_figures::render_figspec(&json, &work.join(format!("icon_{kind}.png")));
    }
    // Materialise content-image assets (if any) into the scratch dir, by
    // basename, so `![caption](name.png)` references resolve at render time.
    if let Some(prefix) = &spec.assets {
        if let Ok(items) = worktree::list(conn, project, prefix) {
            for (path, _sha) in items {
                if let Ok(blob) = worktree::read_at(conn, project, &path) {
                    let base = path.rsplit('/').next().unwrap_or(&path);
                    let _ = std::fs::write(work.join(base), &blob.content);
                }
            }
        }
    }
    let mut chapters: Vec<(String, String)> = Vec::new();
    let mut figs = 0usize;
    for ch in &spec.chapters {
        let Ok(blob) = worktree::read_at(conn, project, ch) else {
            eprintln!("    ! missing chapter {ch}");
            continue;
        };
        let md = String::from_utf8_lossy(&blob.content).to_string();
        let subdir = sanitize(ch.rsplit('/').next().unwrap_or(ch));
        // Surface (don't swallow) a figure-resolution failure: a dropped figspec
        // must not vanish silently from a deliverable (non-repudiation).
        let (resolved, n) = agentic_figures::resolve_markdown(&md, work, &subdir)
            .with_context(|| format!("resolving figspecs in chapter {ch}"))?;
        figs += n;
        chapters.push((ch.clone(), resolved));
    }
    let meta = BookMeta {
        title: spec.title.clone(),
        subtitle: spec.subtitle.clone(),
        author: spec
            .author
            .clone()
            .unwrap_or_else(|| "Daniel Casota".into()),
        context: spec
            .context
            .clone()
            .unwrap_or_else(|| "MAS Cybersecurity, IWI, FHNW — May 2026".into()),
        description: spec.description.clone(),
        dedication: spec.dedication.clone(),
        epigraph: spec.epigraph.clone(),
        epigraph_by: spec.epigraph_by.clone(),
        disclaimer: spec.disclaimer.clone(),
        imprint: spec.imprint.clone(),
        thesis_profile: spec.thesis_profile,
        companion: spec.companion,
        index_terms: spec.index_terms.clone(),
        // Chrome language: per-book `lang` wins; else the global `--lang`; else
        // "en". The i18n layer normalises/falls back, but resolve a non-empty
        // default here so an empty global value still renders English.
        lang: spec.lang.clone().unwrap_or_else(|| {
            if lang.is_empty() {
                "en".to_string()
            } else {
                lang.to_string()
            }
        }),
    };
    let bytes = render_book(&meta, &chapters, work)?;
    let path = out.join(format!("{}.docx", spec.key));
    std::fs::write(&path, &bytes).with_context(|| format!("writing {}", path.display()))?;
    Ok((figs, bytes.len() as u64))
}

// ---- render-quality audit (compare current vs previous iteration) ----
struct DocxFacts {
    media: usize,
    has_heading_styles: bool,
    has_page_size: bool,
    /// Count of the figure-spacing sentinel (ADR-0030 relaxed placement around
    /// figures: `w:after="220"`). Each figure should contribute at least one.
    fig_spacers: usize,
    bytes: u64,
}

fn inspect_docx(path: &Path) -> Result<DocxFacts> {
    let file = std::fs::File::open(path)?;
    let bytes = file.metadata()?.len();
    let mut zip = zip::ZipArchive::new(file)?;
    let mut media = 0usize;
    let mut styles = String::new();
    let mut document = String::new();
    for i in 0..zip.len() {
        let mut f = zip.by_index(i)?;
        let name = f.name().to_string();
        if name.starts_with("word/media/") {
            media += 1;
        } else if name == "word/styles.xml" {
            f.read_to_string(&mut styles).ok();
        } else if name == "word/document.xml" {
            f.read_to_string(&mut document).ok();
        }
    }
    Ok(DocxFacts {
        media,
        has_heading_styles: styles.contains("w:styleId=\"Heading1\""),
        has_page_size: document.contains("w:pgSz"),
        fig_spacers: document.matches("w:after=\"220\"").count(),
        bytes,
    })
}

fn audit(current: &Path, previous: Option<&Path>, json_out: bool) -> Result<()> {
    let books: Vec<PathBuf> = std::fs::read_dir(current)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "docx"))
        .collect();
    let mut fail = false;
    let mut rows = Vec::new();
    println!(
        "{:<28} {:>6} {:>5} {:>5} {:>5} {:>9}  {}",
        "BOOK", "FIGS", "HEAD", "PAGE", "SPC", "KB", "VS PREVIOUS"
    );
    for b in &books {
        let name = b.file_name().unwrap().to_string_lossy().to_string();
        let f = match inspect_docx(b) {
            Ok(f) => f,
            Err(e) => {
                println!("{name:<28}  inspect error: {e}");
                fail = true;
                continue;
            }
        };
        // Intra-book quality invariants.
        let mut notes = Vec::new();
        if !f.has_heading_styles {
            notes.push("NO Heading styles (TOC broken)".to_string());
            fail = true;
        }
        if !f.has_page_size {
            notes.push("no page size".to_string());
            fail = true;
        }
        // ADR-0030 relaxed placement: every figure must carry breathing-room
        // spacing. A book with figures but no figure-spacing sentinels means the
        // figures are cramped against surrounding text (the defect this gate now
        // enforces — previously unaudited).
        let figs_spaced = f.media == 0 || f.fig_spacers >= f.media;
        if !figs_spaced {
            notes.push(format!(
                "figures lack relaxed spacing (ADR-0030): {} figs, {} spacers",
                f.media, f.fig_spacers
            ));
            fail = true;
        }
        // Cross-iteration comparison.
        if let Some(prev) = previous {
            let pp = prev.join(&name);
            if pp.exists() {
                if let Ok(pf) = inspect_docx(&pp) {
                    if f.media + 1 < pf.media {
                        notes.push(format!("FIGURES regressed {}->{}", pf.media, f.media));
                        fail = true;
                    }
                    if f.bytes * 2 < pf.bytes {
                        notes.push(format!(
                            "size collapsed {}KB->{}KB",
                            pf.bytes / 1024,
                            f.bytes / 1024
                        ));
                        fail = true;
                    }
                    if !pf.has_heading_styles && f.has_heading_styles {
                        notes.push("heading styles RESTORED vs previous".to_string());
                    }
                }
            } else {
                notes.push("new (no previous)".to_string());
            }
        }
        println!(
            "{:<28} {:>6} {:>5} {:>5} {:>5} {:>9}  {}",
            name,
            f.media,
            if f.has_heading_styles { "yes" } else { "NO" },
            if f.has_page_size { "yes" } else { "NO" },
            if figs_spaced { "yes" } else { "NO" },
            f.bytes / 1024,
            notes.join("; ")
        );
        rows.push(serde_json::json!({ "book": name, "figures": f.media, "heading_styles": f.has_heading_styles, "figs_spaced": figs_spaced, "kb": f.bytes/1024, "notes": notes }));
    }
    if json_out {
        println!(
            "{}",
            serde_json::json!({ "books": rows, "verdict": if fail {"FAIL"} else {"PASS"} })
        );
    } else {
        println!(
            "--- render audit: {} ---",
            if fail {
                "FAIL (regressions found)"
            } else {
                "PASS"
            }
        );
    }
    if fail {
        std::process::exit(1);
    }
    Ok(())
}
