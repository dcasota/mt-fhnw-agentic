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
    /// Typography profile (ADR-0050). "designer" (default) keeps the
    /// Georgia/Calibri/navy aesthetic for every non-thesis book.
    /// "fhnw-proposal-parity" switches body/headings/captions to
    /// Arial/Arial/Times-New-Roman black for FHNW master-thesis parity.
    #[serde(default)]
    thesis_typography: Option<String>,
    /// Caption format (ADR-0050 §1). "period" (default) → "Figure 1.";
    /// "colon" → "Figure 1:" (English) / "Abbildung 1:" (German).
    #[serde(default)]
    caption_format: Option<String>,
    /// Optional DB path of a PNG logo to render in the FHNW running
    /// header (ADR-0050 §1 item 1). When set with `thesis_typography =
    /// "fhnw-proposal-parity"`, the engine renders the logo + the
    /// `header_lines` on every page header.
    #[serde(default)]
    header_logo: Option<String>,
    /// Optional header text lines (rendered right-aligned beneath the
    /// logo). e.g. `["Master of Advanced Studies", "Leadership in
    /// Cybersecurity"]`.
    #[serde(default)]
    header_lines: Vec<String>,
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
      # ADR-0050 §1 item 1 (v0.1.15-engine, 2026-05-29): inject the FHNW
      # running header via Word's own API when a sidecar exists. The
      # docx-rs Pic-in-header path emits XML Word silently rejects on
      # open (verified 2026-05-29: header XML on disk is well-formed but
      # InlineShapes.Count == 0 after Documents.Open). Using
      # InlineShapes.AddPicture lets Word build XML Word itself will
      # accept.
      $sidePath = $pth + '.fhnw_header.json'
      if (Test-Path $sidePath) {{
        try {{
          # IMPORTANT: -Encoding UTF8. Windows PowerShell 5.1's Get-Content
          # defaults to the system code page (Windows-1252 in DE/CH locales),
          # which corrupts non-ASCII path characters (`ö` → `Ã¶`) when reading
          # the UTF-8 JSON sidecar written by Rust. Verified 2026-05-29:
          # without -Encoding UTF8 the OneDrive path `Persönlich` becomes
          # `PersÃ¶nlich` and Test-Path fails, silently skipping the logo
          # injection.
          $side = Get-Content $sidePath -Raw -Encoding UTF8 | ConvertFrom-Json
          $cr = [char]13  # Word paragraph mark = CR
          # 2026-05-29 fix: write to Section 1's primary header ONLY.
          # The docx-rs builder emits multiple sections (we saw 5 in the
          # rebuilt master_thesis.docx), all marked LinkToPrevious=True.
          # Setting `.Range.Text = ''` inside a `foreach ($sec in $d.Sections)`
          # loop wipes section 1's header on the second iteration (because
          # section 2 IS section 1 when linked) — and the wipe also drops
          # any InlineShape that was just added. Edit section 1 only; the
          # other sections inherit via LinkToPrevious.
          $sec1Hdrs = $d.Sections.Item(1).Headers
          # Suppress different-first-page so the same header renders on page 1.
          $d.Sections.Item(1).PageSetup.DifferentFirstPageHeaderFooter = 0
          $hdr = $sec1Hdrs.Item(1)
          # Wipe and right-align the whole header.
          $hdr.Range.Text = ''
          $hdr.Range.ParagraphFormat.Alignment = 2  # wdAlignParagraphRight
          # 2026-05-29 debug: surface the parsed logo path + Test-Path result
          # so we can see if non-ASCII characters survive the Rust→PowerShell
          # round-trip.
          $logoPath = $side.logo_path_abs
          $logoExists = if ($logoPath) {{ Test-Path -LiteralPath $logoPath }} else {{ $false }}
          Write-Output ("{{0}}`tHEADER_PROBE  logo=[{{1}}]  exists={{2}}" -f $pth, $logoPath, $logoExists)
            # Layout: FLOATING-anchor logo + text-line paragraphs.
            # The FHNW MAS proposal docx places the logo as a Shape (NOT
            # an InlineShape) anchored to the page at (-49.3, -59.8) pt
            # with wrap=BehindText, so the header text flows in its own
            # right-aligned paragraph while the logo overlays the top-left
            # corner (bleeding slightly off-page). This was the Fix-A
            # target deferred from v0.1.16-engine; the previous attempt
            # failed because it tried `InlineShape.ConvertToShape()` on
            # an already-injected inline picture (Word: "Parameter value
            # out of acceptable range" on header-anchored inlines).
            # `Headers.Shapes.AddPicture(...)` builds the floating shape
            # DIRECTLY — no convert step. Verified live 2026-05-29 against
            # the proposal coordinate dump: bit-for-bit match.
            if ($logoExists) {{
              # Delete any pre-existing inline shape in the header (left
              # over from the v0.1.15..16-engine inline path; defensive
              # so this code path is idempotent on re-runs).
              while ($hdr.Range.InlineShapes.Count -gt 0) {{
                $hdr.Range.InlineShapes.Item(1).Delete()
              }}
              # Add as floating Shape via the Header's Shapes collection
              # — anchor is implicit (the header range). 1 cm = 28.346 pt.
              $W_pt = [double]$side.logo_width_cm * 28.346
              $H_pt = [double]$side.logo_height_cm * 28.346
              $L_pt = [double]$side.logo_left_pt
              $T_pt = [double]$side.logo_top_pt
              $shape = $hdr.Shapes.AddPicture($logoPath, $false, $true, $L_pt, $T_pt, $W_pt, $H_pt)
              # Re-assert L/T after Word's auto-adjust (it sometimes
              # re-anchors to the paragraph on first add; setting the
              # absolute values + relH/relV pins it page-relative).
              $shape.WrapFormat.Type           = [int]$side.logo_wrap_type
              $shape.RelativeHorizontalPosition = [int]$side.logo_relh
              $shape.RelativeVerticalPosition   = [int]$side.logo_relv
              $shape.Left = $L_pt
              $shape.Top  = $T_pt
              Write-Output ("{{0}}`tHEADER_PIC_ADDED  floating-count={{1}}  L={{2}} T={{3}} W={{4}} H={{5}} wrap={{6}}" -f $pth, $hdr.Shapes.Count, [math]::Round($shape.Left,1), [math]::Round($shape.Top,1), [math]::Round($shape.Width,1), [math]::Round($shape.Height,1), $shape.WrapFormat.Type)
            }}
            # 2) Append each non-empty text line, terminated by a paragraph
            # mark. After all text is in place we style the whole header
            # uniformly (font/size/bold).
            foreach ($line in $side.lines) {{
              if ([string]::IsNullOrWhiteSpace($line)) {{ continue }}
              $hdr.Range.InsertAfter($line + $cr)
            }}
            # Apply uniform character formatting to every run in the header
            # (the picture is unaffected; this only touches text runs).
            $hdr.Range.Font.Name = $side.line_font
            $hdr.Range.Font.Size = $side.line_size_pt
            $hdr.Range.Font.Bold = [int]$side.line_bold
            $hdr.Range.ParagraphFormat.Alignment = 2  # right (re-assert)
            Write-Output ("{{0}}`tHEADER_DONE  floating-count={{1}}  inline-count={{2}}  paras={{3}}" -f $pth, $hdr.Shapes.Count, $hdr.Range.InlineShapes.Count, $hdr.Range.Paragraphs.Count)
            # ADR-0050 §17 / ADR-0030 §37 — centred page-number footer.
            # docx-rs 0.4.20 attaches only ONE Footer per Document
            # (verified 2026-05-29 by unzipping the rendered docx: 4
            # footer*.xml files where only 1 has the PAGE field; the
            # others are Word-generated empty stubs for each section).
            # Result: most pages render without a footer. Fix: walk
            # every section, write the centred PAGE field into Sec1's
            # primary footer, then set LinkToPrevious=True on Sec2+.
            if ([bool]$side.footer_pagenum_enabled) {{
              try {{
                $sec1Ftr = $d.Sections.Item(1).Footers.Item(1)
                $sec1Ftr.Range.Text = ''
                # Use Word's NATIVE Footer.PageNumbers.Add — handles the
                # PAGE field, paragraph alignment, AND auto-computes the
                # display value on every page. Fields.Add with the raw
                # PAGE code created the field but Word didn't auto-
                # compute it (cached '0'); PageNumbers.Add does the
                # complete job. The alignment param is a
                # WdPageNumberAlignment: 0=Left 1=Center 2=Right
                # 3=Inside 4=Outside. We map the sidecar
                # footer_pagenum_alignment (paragraph wdAlignParagraph*,
                # 0=L 1=C 2=R 3=J) onto the page-number variant by
                # taking 0/1/2 directly; J (3) falls back to Center.
                $pnAlign = [int]$side.footer_pagenum_alignment
                if ($pnAlign -lt 0 -or $pnAlign -gt 2) {{ $pnAlign = 1 }}
                $sec1Ftr.PageNumbers.Add($pnAlign, $true) | Out-Null
                # Style the inserted page number (Arial 11pt black,
                # proposal-parity).
                $sec1Ftr.Range.Font.Name = $side.footer_pagenum_font
                $sec1Ftr.Range.Font.Size = [int]$side.footer_pagenum_size_pt
                $sec1Ftr.Range.Font.Bold = $false
                # Sections 2+ inherit from Sec1 via LinkToPrevious. This
                # replaces the Word-generated empty footers with the
                # Sec1 page-number content.
                for ($s = 2; $s -le $d.Sections.Count; $s++) {{
                  $d.Sections.Item($s).Footers.Item(1).LinkToPrevious = $true
                }}
                Write-Output ("{{0}}`tFOOTER_PAGENUM_ADDED  sections={{1}}  field-count={{2}}" -f $pth, $d.Sections.Count, $sec1Ftr.Range.Fields.Count)
              }} catch {{
                $fmsg = "FOOTER_INJECT_ERR {{0}} | trace: {{1}}" -f $_.Exception.Message, ($_.ScriptStackTrace -replace "`r?`n", " >> ")
                Write-Output ("{{0}}`t{{1}}" -f $pth, $fmsg)
                [Console]::Error.WriteLine(("FHNW-FTR-FAIL [{{0}}]: {{1}}" -f $pth, $fmsg))
              }}
            }}
        }} catch {{
          # Write to BOTH stdout (so the Rust filter sees it) AND stderr
          # (so the user sees it raw in case something else filters
          # stdout). The full ScriptStackTrace is included so we can find
          # which line failed without re-running the script manually.
          $msg = "HEADER_INJECT_ERR {{0}} | trace: {{1}}" -f $_.Exception.Message, ($_.ScriptStackTrace -replace "`r?`n", " >> ")
          Write-Output ("{{0}}`t{{1}}" -f $pth, $msg)
          [Console]::Error.WriteLine(("FHNW-HDR-FAIL [{{0}}]: {{1}}" -f $pth, $msg))
        }}
      }}
      $d.Fields.Update() | Out-Null
      # 2026-05-29 Fix-G1: $d.Fields.Update() only refreshes the
      # MAIN BODY story. Header/footer fields (specifically the
      # PAGE field newly injected by FOOTER_PAGENUM_ADDED) live in
      # separate StoryRanges and stay at their cached value (often
      # "0") unless explicitly updated. Walk every story and refresh
      # its fields so the page-number footer shows real numbers.
      try {{
        foreach ($story in $d.StoryRanges) {{
          $story.Fields.Update() | Out-Null
          $nxt = $story.NextStoryRange
          while ($nxt -ne $null) {{
            $nxt.Fields.Update() | Out-Null
            $nxt = $nxt.NextStoryRange
          }}
        }}
      }} catch {{
        # StoryRanges iteration can fail on docs with no linked
        # stories — non-fatal; the main update above already covered
        # the body fields, headers/footers may still show "0" but
        # Word will refresh them on user open via updateFields.
        [Console]::Error.WriteLine(("STORY_UPDATE_WARN [{{0}}]: {{1}}" -f $pth, $_.Exception.Message))
      }}
      foreach ($tof in $d.TablesOfFigures) {{ $tof.Update() }}
      foreach ($toc in $d.TablesOfContents) {{ $toc.Update() }}
      foreach ($ix in $d.Indexes) {{ $ix.Update() }}
      try {{ $d.ActiveWindow.View.ShowFieldCodes=$false }} catch {{}}
      $d.Repaginate()
      $pages=$d.ComputeStatistics(2)
      $finalHdr = $d.Sections.Item(1).Headers.Item(1)
      Write-Output ("{{0}}`tHEADER_PRE_SAVE  floating-count={{1}}  inline-count={{2}}" -f $pth, $finalHdr.Shapes.Count, $finalHdr.Range.InlineShapes.Count)
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
    // Surface any FHNW-HDR-FAIL / FHNW-FTR-FAIL diagnostics from
    // stderr to the user.
    let stderr = String::from_utf8_lossy(&out.stderr);
    for line in stderr.lines() {
        if line.contains("FHNW-HDR-FAIL") || line.contains("FHNW-FTR-FAIL") {
            eprintln!("{line}");
        }
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

    // Honor model-review exclusions (ADR-0049 ph3): a chapter whose current
    // model_review verdict is "exclude" is held out of the mainline build.
    // Append-only — a later "accept" review supersedes and re-includes it.
    let excluded = agentic_core::review::excluded_paths(&conn, project).unwrap_or_default();

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

        let result = build_one(&conn, project, spec, &work, out, lang, &excluded);
        let _ = std::fs::remove_dir_all(&work); // always clean this step's scratch
        match result {
            Ok((figs, bytes, held)) => {
                built += 1;
                built_docs.push(out.join(format!("{}.docx", spec.key)));
                let included = spec.chapters.len() - held.len();
                report.push(serde_json::json!({
                    "key": spec.key, "chapters": included, "held_by_review": held,
                    "figures": figs, "docx_bytes": bytes
                }));
                if !json_out {
                    let held_note = if held.is_empty() {
                        String::new()
                    } else {
                        format!(", {} held by review", held.len())
                    };
                    println!(
                        "  + {}.docx  ({} figures, {included}/{} chapters{held_note})",
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
                // 2026-05-29 DEBUG: surface per-doc HEADER_* diagnostics so
                // the FHNW header injection lifecycle is visible. Remove
                // when item D1 is resolved.
                for (p, r) in &res {
                    if r.contains("HEADER_") {
                        eprintln!("[finalize-debug] {p}\t{r}");
                    }
                }
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
    excluded: &std::collections::HashSet<String>,
) -> Result<(usize, u64, Vec<String>)> {
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
    let mut held: Vec<String> = Vec::new();
    for ch in &spec.chapters {
        // ADR-0049 ph3 — skip chapters held out by a model_review "exclude" verdict.
        if excluded.contains(ch) {
            held.push(ch.clone());
            eprintln!("    ~ held by review: {ch}");
            continue;
        }
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
    // ADR-0050: parse the manifest's typography / caption-format strings into
    // the typed enums; unknown values fall back to defaults (zero regression
    // for every manifest authored before v0.1.13).
    let thesis_typography = match spec.thesis_typography.as_deref() {
        Some("fhnw-proposal-parity") => agentic_export::book::TypographyProfile::FhnwProposalParity,
        _ => agentic_export::book::TypographyProfile::Designer,
    };
    let caption_format = match spec.caption_format.as_deref() {
        Some("colon") => agentic_export::book::CaptionFormat::Colon,
        _ => agentic_export::book::CaptionFormat::Period,
    };
    // ADR-0050 §1 item 1: load the FHNW running-header logo bytes from the
    // project DB when the manifest specifies one. Failures are non-fatal
    // (the engine falls back to no header); we log a context-rich warning.
    let header_logo: Option<Vec<u8>> = if let Some(path) = spec.header_logo.as_deref() {
        match agentic_core::worktree::read_at(conn, project, path) {
            Ok(blob) => Some(blob.content),
            Err(e) => {
                eprintln!("warn: header_logo {path} not loadable ({e}) — rendering without logo");
                None
            }
        }
    } else {
        None
    };
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
        thesis_typography,
        page_numbering: agentic_export::book::PageNumbering::default(),
        caption_format,
        header_logo,
        header_lines: spec.header_lines.clone(),
    };
    let bytes = render_book(&meta, &chapters, work)?;
    let path = out.join(format!("{}.docx", spec.key));
    std::fs::write(&path, &bytes).with_context(|| format!("writing {}", path.display()))?;

    // ADR-0050 §1 item 1 (v0.1.15-engine, 2026-05-29 fix): the engine
    // can't reliably produce a Word-acceptable header drawing via
    // docx-rs's `Pic` — Word's parser silently discards it on open.
    // Write a sidecar JSON next to the docx; the finalize step injects
    // the header via Word's own API. The logo bytes are materialised to
    // a sibling PNG so finalize doesn't need the project DB.
    if agentic_export::book::fhnw_header_sidecar_needed(&meta) {
        let logo_path_abs: Option<String> = if let Some(bytes) = meta.header_logo.as_ref() {
            let logo_path = out.join(format!("{}.fhnw_logo.png", spec.key));
            std::fs::write(&logo_path, bytes)
                .with_context(|| format!("writing FHNW logo to {}", logo_path.display()))?;
            // Use the absolute path so the PowerShell finalize step is
            // independent of the working directory.
            std::fs::canonicalize(&logo_path)
                .ok()
                .map(|p| p.to_string_lossy().replace(r"\\?\", ""))
        } else {
            None
        };
        let sidecar =
            agentic_export::book::FhnwHeaderSidecar::from_meta(&meta, logo_path_abs.clone());
        let sidecar_path = out.join(format!("{}.docx.fhnw_header.json", spec.key));
        let json = serde_json::to_string_pretty(&sidecar)
            .with_context(|| "serialising FHNW header sidecar")?;
        std::fs::write(&sidecar_path, json).with_context(|| {
            format!("writing FHNW header sidecar to {}", sidecar_path.display())
        })?;
    }

    Ok((figs, bytes.len() as u64, held))
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
