//! `agentic cascade run` — monolithic SDD full-cycle orchestrator.
//!
//! Chains the EXISTING in-tool steps into the enforced cascade:
//!   1. BOOT GATE          — `check self` + `check tree --root .`
//!   2. INBOX INTAKE       — `import dir inbox` → `embed` → `classify` (if any)
//!   3. REGENERATE DIMS    — `orchestrate add dim<NN>` × N → `orchestrate run --wave`
//!   4. MERGE              — `merge dimensions`
//!   5. BUILD BOOK         — the three bookkit profiles (A merged, B companion,
//!                           C thesis); `--per-dimension` renders every book
//!   6. AUDIT GATES        — universal gates (all profiles) + thesis-only gates
//!                           (bookkit C); verdicts harvested (ADR-0045)
//!   7. SEAL               — `audit sign-commits` → `audit report`
//!
//! This module COMPOSES the existing commands; it does not reimplement them.
//! Every heavy / exiting step is run by re-invoking THIS executable as a
//! subprocess — the robust choice, because `check::run` calls
//! `std::process::exit(1)` on a FAIL gate (calling it in-process would abort the
//! whole cascade on the first failing gate). As a subprocess a FAIL surfaces as
//! a non-zero exit code we record and keep going; gate verdicts are harvested
//! from `audit_verdicts` (every gate records its verdict there per ADR-0041).

use std::path::Path;
use std::process::Command as ProcCommand;

use anyhow::{Context, Result};
use serde_json::json;

use agentic_core::worktree;

use crate::cli::CascadeAction;

/// Inputs to the cascade plan (everything the step list depends on).
struct CascadeOpts {
    project: String,
    manifest: String,
    out: String,
    regenerate: bool,
    per_dimension: bool,
    /// Bookkit A — the merged dimensions book key.
    merged_key: String,
    /// Bookkit B — the student-notes companion key.
    companion_key: String,
    /// Bookkit C — the master-thesis key.
    thesis_key: String,
    /// Bookkit — the master-thesis-bookkit key (parity + toc-coverage scoped gates).
    bookkit_key: String,
    /// Skip expensive steps already completed for the current fingerprint.
    resume: bool,
    /// Ignore checkpoints; run every step from scratch.
    force_full: bool,
    dry_run: bool,
    root: String,
    /// Bookkit C structural-rule HITL: when true, a PAGE_OVER /
    /// BOLD_OVERUSE / NON_ENGLISH / HEADING_DEPTH finding from the
    /// thesis-profile gate run halts the cascade before phase 7 (seal).
    thesis_strict: bool,
}

/// Phases whose steps are checkpointed/skippable on `--resume` (the expensive
/// ones: regenerate, merge, build). Gates and seal always re-run.
const CHECKPOINTED_PHASES: &[u8] = &[3, 4, 5];

/// A content fingerprint of the project's working tree — changes whenever any
/// blob does, naturally invalidating stale checkpoints (input-delta gating).
fn input_fingerprint(conn: &rusqlite::Connection, project: &str) -> String {
    use std::hash::{Hash, Hasher};
    let entries = worktree::list(conn, project, "").unwrap_or_default();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for (p, sha) in &entries {
        p.hash(&mut h);
        sha.hash(&mut h);
    }
    format!("{:016x}", h.finish())
}

fn step_done(conn: &rusqlite::Connection, project: &str, fp: &str, label: &str) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM cascade_steps WHERE project_id=?1 AND fingerprint=?2 AND step_label=?3",
        rusqlite::params![project, fp, label],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n > 0)
    .unwrap_or(false)
}

fn record_step(conn: &rusqlite::Connection, project: &str, fp: &str, label: &str) {
    let _ = conn.execute(
        "INSERT INTO cascade_steps (project_id, fingerprint, step_label) VALUES (?1,?2,?3)",
        rusqlite::params![project, fp, label],
    );
}

fn clear_steps(conn: &rusqlite::Connection, project: &str) {
    let _ = conn.execute(
        "DELETE FROM cascade_steps WHERE project_id=?1",
        rusqlite::params![project],
    );
}

/// One planned step: a human label, the `agentic` args (after `--db <db>`) to
/// run it as a subprocess, and the `audit_verdicts` checkpoint to read the
/// verdict back from (if it records one). A step with empty `args` is a
/// printed-intent placeholder (skipped / informational) that records no run.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Step {
    /// The `[cascade N/7]` banner number (1..7).
    phase: u8,
    label: String,
    args: Vec<String>,
    checkpoint: Option<String>,
}

impl Step {
    fn run(phase: u8, label: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            phase,
            label: label.into(),
            args,
            checkpoint: None,
        }
    }
    fn gate(phase: u8, label: impl Into<String>, args: Vec<String>, cp: &str) -> Self {
        Self {
            phase,
            label: label.into(),
            args,
            checkpoint: Some(cp.to_string()),
        }
    }
    /// A printed-intent placeholder (no subprocess; just a note in the table).
    fn note(phase: u8, label: impl Into<String>) -> Self {
        Self {
            phase,
            label: label.into(),
            args: Vec::new(),
            checkpoint: None,
        }
    }
}

/// The audit gate suite is composed from the governed rule-matrix (ADR-0047 R4)
/// — see [`agentic_core::profiles`]. The gate catalog (subcommand → checkpoint)
/// is in code; which gates apply is governed data loaded from
/// `specs/rule-matrix.json` (or the default matrix when absent). The cascade
/// resolves the suite once, in `run_cascade`, and threads it into the plan.
const RULE_MATRIX_PATH: &str = "specs/rule-matrix.json";

/// Load the rule-matrix from the content store, falling back to the default.
fn load_rule_matrix(
    conn: &rusqlite::Connection,
    project: &str,
) -> agentic_core::profiles::RuleMatrix {
    if let Ok(blob) = worktree::read_at(conn, project, RULE_MATRIX_PATH) {
        let text = String::from_utf8_lossy(&blob.content);
        if let Ok(m) = agentic_core::profiles::RuleMatrix::parse(&text) {
            return m;
        }
    }
    agentic_core::profiles::RuleMatrix::default_matrix()
}

/// `Dimension_NN_*.md` basename → its numeric index (01..11), else None.
/// Mirrors the merge command's selector so the two stay in lock-step.
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

/// `out/sources/Dimension_03_intelligent_agents_EN.md` → (`"03"`, `"intelligent_agents"`).
/// The slug is everything between `Dimension_NN_` and a trailing `_EN`.
fn split_dim(path: &str) -> Option<(String, String)> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let rest = name.strip_prefix("Dimension_")?;
    let nn: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if nn.is_empty() {
        return None;
    }
    let after = &rest[nn.len()..];
    let after = after.strip_prefix('_').unwrap_or(after);
    // Drop a `.md` extension, then a trailing `_EN` language tag if present.
    let stem = after.strip_suffix(".md").unwrap_or(after);
    let stem = stem.strip_suffix("_EN").unwrap_or(stem);
    Some((nn, stem.to_string()))
}

/// The per-dimension regeneration task prompt (step 3).
fn regen_prompt(nn: &str, slug: &str) -> String {
    format!(
        "Update out/sources/Dimension_{nn}_{slug}_EN.md in place: integrate the latest ranked \
inbox findings relevant to this dimension; keep it English-only; use bold ONLY as a short \
leading label (never inline emphasis); keep headings within 3 sub-levels (H2/H3/H4); preserve \
all ```figspec``` blocks. Then stop."
    )
}

/// Build the ordered 7-phase step plan (pure: depends only on `opts` + the
/// discovered dimension paths). Used both by the executor and the unit tests.
fn build_plan(
    opts: &CascadeOpts,
    dim_paths: &[String],
    inbox_has_items: bool,
    gate_suite: &[(&'static str, &'static str)],
) -> Vec<Step> {
    let mut steps = Vec::new();
    push_boot_gate(&mut steps, opts);
    push_inbox_intake(&mut steps, opts, inbox_has_items);
    push_review(&mut steps, opts);
    push_regenerate(&mut steps, opts, dim_paths);
    push_merge(&mut steps, opts);
    push_bibliography_emit(&mut steps, opts);
    push_build_book(&mut steps, opts);
    push_audit_gates(&mut steps, opts, gate_suite);
    push_seal(&mut steps, opts);
    steps
}

/// 1. BOOT GATE — `check self` then `check tree --root`.
fn push_boot_gate(steps: &mut Vec<Step>, opts: &CascadeOpts) {
    let p = &opts.project;
    steps.push(Step::gate(
        1,
        "check self",
        vec!["check".into(), "self".into(), "--project".into(), p.clone()],
        "self",
    ));
    steps.push(Step::gate(
        1,
        "check tree",
        vec![
            "check".into(),
            "tree".into(),
            "--project".into(),
            p.clone(),
            "--root".into(),
            opts.root.clone(),
        ],
        "tree",
    ));
}

/// 2. INBOX INTAKE — import→embed→classify if the inbox carries items.
fn push_inbox_intake(steps: &mut Vec<Step>, opts: &CascadeOpts, inbox_has_items: bool) {
    let p = &opts.project;
    if !inbox_has_items {
        steps.push(Step::note(2, "inbox empty — skipping"));
        return;
    }
    if opts.dry_run {
        // import/embed/classify mutate the DB and may call a provider; in a
        // dry-run we only announce the intake (no state change).
        steps.push(Step::note(
            2,
            "inbox has items — skipped (dry-run); would import→embed→classify",
        ));
        return;
    }
    steps.push(Step::run(
        2,
        "import dir inbox",
        vec![
            "import".into(),
            "dir".into(),
            "inbox".into(),
            "--project".into(),
            p.clone(),
            "--prefix".into(),
            "inbox".into(),
        ],
    ));
    steps.push(Step::run(
        2,
        "embed inbox",
        vec!["embed".into(), p.clone(), "--prefix".into(), "inbox".into()],
    ));
    steps.push(Step::run(
        2,
        "classify inbox",
        vec![
            "classify".into(),
            p.clone(),
            "--prefix".into(),
            "inbox".into(),
        ],
    ));
}

/// 2b. MODEL REVIEW (ADR-0049 ph3) — a second model reviews every deliverable
/// + the rankings, writing verdicts that the downstream merge/build adopt:
/// `exclude` paths are held out of the mainline (append-only, auditable; a
/// later `accept` re-includes). Failure (e.g. no chat provider) is reported by
/// the orchestrator and the cascade continues — generation is never blocked on
/// model availability (consistent with the Word-finalize policy).
fn push_review(steps: &mut Vec<Step>, opts: &CascadeOpts) {
    let p = &opts.project;
    if opts.dry_run {
        steps.push(Step::note(2, "model review — skipped (dry-run)"));
        return;
    }
    steps.push(Step::run(
        2,
        "review run",
        vec!["review".into(), "run".into(), "--project".into(), p.clone()],
    ));
}

/// 3. REGENERATE DIMENSIONS — one bounded sub-session per dimension, then a wave run.
fn push_regenerate(steps: &mut Vec<Step>, opts: &CascadeOpts, dim_paths: &[String]) {
    let p = &opts.project;
    if !opts.regenerate || opts.dry_run {
        let why = if opts.dry_run {
            "dry-run"
        } else {
            "--no-regenerate"
        };
        steps.push(Step::note(3, format!("regenerate skipped ({why})")));
        return;
    }
    for path in dim_paths {
        if let Some((nn, slug)) = split_dim(path) {
            steps.push(Step::run(
                3,
                format!("orchestrate add dim{nn}"),
                vec![
                    "orchestrate".into(),
                    "add".into(),
                    "--project".into(),
                    p.clone(),
                    "--id".into(),
                    format!("dim{nn}"),
                    "--task".into(),
                    regen_prompt(&nn, &slug),
                ],
            ));
        }
    }
    steps.push(Step::run(
        3,
        "orchestrate run --wave",
        vec![
            "orchestrate".into(),
            "run".into(),
            "--project".into(),
            p.clone(),
            "--wave".into(),
            "--root".into(),
            opts.root.clone(),
        ],
    ));
}

/// 4. MERGE — assemble the dimensions into the merged compendium.
fn push_merge(steps: &mut Vec<Step>, opts: &CascadeOpts) {
    if opts.dry_run {
        steps.push(Step::note(
            4,
            "merge dimensions — skipped (dry-run); would write out/sources/Dimensions_merged_EN.md",
        ));
    } else {
        steps.push(Step::run(
            4,
            "merge dimensions",
            vec![
                "merge".into(),
                "dimensions".into(),
                "--project".into(),
                opts.project.clone(),
            ],
        ));
    }
}

/// 4b. BIBLIOGRAPHY REFRESH — emit the curated alphabetical APA7 References
/// chapter from the literature_corpus passport and ingest into the worktree
/// at `out/sources/Dimensions_bibliography_EN.md`. Runs after merge so newly-
/// landed dimension content (which may have introduced URLs the previous
/// `harvest` already collected) is reflected in the rendered References. The
/// step is idempotent: when the passport hasn't changed the worktree blob
/// stays identical, so the input-fingerprint guard naturally skips it on
/// `--resume`.
fn push_bibliography_emit(steps: &mut Vec<Step>, opts: &CascadeOpts) {
    if opts.dry_run {
        steps.push(Step::note(
            4,
            "bibliography emit --write — skipped (dry-run); would refresh out/sources/Dimensions_bibliography_EN.md",
        ));
    } else {
        steps.push(Step::run(
            4,
            "bibliography emit --write",
            vec![
                "bibliography".into(),
                "emit".into(),
                "--project".into(),
                opts.project.clone(),
                "--write".into(),
            ],
        ));
    }
}

/// 5. BUILD BOOK — the three bookkit profiles (ADR-0045): A merged dimensions
/// book, B student-notes companion, C master thesis. With `--per-dimension`,
/// every book in the manifest is rendered instead (covers the individual
/// dimension/campaign books too).
fn push_build_book(steps: &mut Vec<Step>, opts: &CascadeOpts) {
    if opts.dry_run {
        let scope = if opts.per_dimension {
            "all books".to_string()
        } else {
            format!(
                "three profiles (A {}, B {}, C {})",
                opts.merged_key, opts.companion_key, opts.thesis_key
            )
        };
        steps.push(Step::note(
            5,
            format!("book build — skipped (dry-run); would render {scope}"),
        ));
        return;
    }
    // The manifest is authoritative in `thesis.db`. Materialise it into an
    // EPHEMERAL temp scratch dir — never the project root — so a cascade run no
    // longer creates an on-disk `out/` folder in the working tree (out/
    // deprecation: the on-disk `out/` is retired; the DB keeps its `out/sources/`
    // paths, and `book build` reads chapters straight from the DB). `book build`
    // only needs the manifest *file* on disk; chapters resolve from the DB.
    let scratch = std::env::temp_dir()
        .join(format!("agentic_cascade_{}", std::process::id()))
        .to_string_lossy()
        .replace('\\', "/");
    let manifest_disk = format!("{scratch}/{}", opts.manifest);
    steps.push(Step::run(
        5,
        "checkout manifest (scratch)",
        vec![
            "content".into(),
            "checkout".into(),
            "--project".into(),
            opts.project.clone(),
            "--to".into(),
            scratch.clone(),
            "--prefix".into(),
            opts.manifest.clone(),
        ],
    ));
    let build = |only: Option<&str>| -> Vec<String> {
        let mut a = vec![
            "book".into(),
            "build".into(),
            "--project".into(),
            opts.project.clone(),
            "--manifest".into(),
            manifest_disk.clone(),
            "--out".into(),
            opts.out.clone(),
        ];
        if let Some(k) = only {
            a.push("--only".into());
            a.push(k.to_string());
        }
        a
    };
    if opts.per_dimension {
        // Render every book in the manifest (all profiles + per-dimension/campaign).
        steps.push(Step::run(5, "book build (all)", build(None)));
    } else {
        // Default: the three profile deliverables, one build each.
        steps.push(Step::run(
            5,
            format!("book build A ({})", opts.merged_key),
            build(Some(&opts.merged_key)),
        ));
        steps.push(Step::run(
            5,
            format!("book build B ({})", opts.companion_key),
            build(Some(&opts.companion_key)),
        ));
        steps.push(Step::run(
            5,
            format!("book build C ({})", opts.thesis_key),
            build(Some(&opts.thesis_key)),
        ));
        // Bookkit build runs AFTER the master_thesis book (cross-cutting risk #2:
        // ordering matters — the bookkit overlay reads artefacts produced by
        // the thesis book build).
        steps.push(Step::run(
            5,
            format!("book build bookkit ({})", opts.bookkit_key),
            build(Some(&opts.bookkit_key)),
        ));
    }
}

/// 6. AUDIT GATES — composed from the governed rule-matrix (ADR-0047 R4): the
/// `gate_suite` is `universal + per-profile additions`, resolved once in
/// `run_cascade` from `specs/rule-matrix.json` (or the default matrix). Each
/// gate records its own verdict.
///
/// Thesis-profile scope wiring (2026-05-28): `page-boundary` and `bookkit`
/// are upgraded with `--paths-from-manifest` + `--book-key=<thesis_key>` so
/// they measure the exact chapter list the master-thesis book composes
/// (mixed `thesis/` + `out/sources/` prefixes). `page-boundary` additionally
/// passes `--words-per-page=280` — the empirical FHNW Word render density.
/// Both args are NEW opt-ins on the gates; non-thesis cascades pass nothing
/// extra and the gates fall back to their legacy `--prefix` behaviour.
fn push_audit_gates(
    steps: &mut Vec<Step>,
    opts: &CascadeOpts,
    gate_suite: &[(&'static str, &'static str)],
) {
    let p = &opts.project;
    // Scope-narrowed per-key: page-boundary and bookkit run once per key
    // (master_thesis + master_thesis_bookkit) so the two profiles can carry
    // distinct manifest-scoped findings (ADR-0047 R4 + the new bookkit
    // overlay). Other gates run once, profile-agnostic.
    let scoped_keys: Vec<&String> = vec![&opts.thesis_key, &opts.bookkit_key];
    for (sub, cp) in gate_suite {
        match *sub {
            "page-boundary" | "bookkit" => {
                for key in &scoped_keys {
                    let mut args =
                        vec!["check".into(), (*sub).into(), "--project".into(), p.clone()];
                    args.push("--paths-from-manifest".into());
                    args.push(opts.manifest.clone());
                    args.push("--book-key".into());
                    args.push((*key).clone());
                    if *sub == "page-boundary" {
                        args.push("--words-per-page".into());
                        args.push("280".into());
                    }
                    steps.push(Step::gate(6, format!("check {sub} ({key})"), args, cp));
                }
            }
            "parity" => {
                // Per-book parity dispatch — ADR-0057 §3.4 / ADR-0061 §3.1.
                // Iterate the two book keys cascade tracks (`thesis_key` and
                // `bookkit_key`, default `"master_thesis"` and
                // `"master_thesis_bookkit"`), look up the frozen reference
                // docx via `parity::canonical_reference_path`, and emit one
                // gate step per (book, reference) pair. Books with no
                // canonical reference (today: the old-pipeline
                // `master_thesis` book and any third-party book) are skipped
                // — the gate has no baseline to compare against, so manual
                // `agentic check parity --book <k> --reference <docx>` is
                // the only invocation path for those. Missing fixtures on
                // disk are handled inside the gate itself
                // (`PARITY_FIXTURE_ABSENT` INFO finding ⇒ PASS), so the
                // cascade survives an unprovisioned fixture without losing
                // the audit_verdicts row.
                for key in &scoped_keys {
                    let Some(ref_path) = agentic_checks::parity::canonical_reference_path(key)
                    else {
                        continue;
                    };
                    let args = vec![
                        "check".into(),
                        "parity".into(),
                        "--project".into(),
                        p.clone(),
                        "--book".into(),
                        (*key).clone(),
                        "--reference".into(),
                        ref_path.display().to_string(),
                    ];
                    steps.push(Step::gate(6, format!("check parity ({key})"), args, cp));
                }
            }
            _ => {
                let mut args = vec!["check".into(), (*sub).into(), "--project".into(), p.clone()];
                match *sub {
                    "tree" | "docs" => {
                        args.push("--root".into());
                        args.push(opts.root.clone());
                    }
                    "contamination" => args.push("--offline".into()),
                    _ => {}
                }
                steps.push(Step::gate(6, format!("check {sub}"), args, cp));
            }
        }
    }
}

/// Bookkit-C structural-rule categories that the `--thesis-strict` HITL
/// pause treats as cascade-stoppers (FHNW master-thesis structure must hold
/// before the seal step; see ADR-0045 and the 2026-05-28 root-cause report).
const STRICT_STRUCTURAL_CATEGORIES: &[&str] =
    &["PAGE_OVER", "BOLD_OVERUSE", "NON_ENGLISH", "HEADING_DEPTH"];

/// Inspect the latest `audit_verdicts` rows for the bookkit-C gates and return
/// the structural categories that fired. Empty = clean; non-empty triggers
/// the [HITL PAUSE] block in `--thesis-strict` mode.
fn strict_structural_violations(
    db_path: &Path,
    project: &str,
) -> Vec<(&'static str, &'static str, String)> {
    let mut hits = Vec::new();
    let conn = match agentic_core::db::open(db_path) {
        Ok(c) => c,
        Err(_) => return hits,
    };
    for checkpoint in ["page_boundary", "bookkit"] {
        let row: Result<String, _> = conn.query_row(
            "SELECT findings_json FROM audit_verdicts \
             WHERE project_id = ?1 AND checkpoint = ?2 \
             ORDER BY id DESC LIMIT 1",
            rusqlite::params![project, checkpoint],
            |r| r.get::<_, String>(0),
        );
        let Ok(json_text) = row else { continue };
        let Ok(findings): Result<Vec<serde_json::Value>, _> = serde_json::from_str(&json_text)
        else {
            continue;
        };
        for f in findings {
            let category = f
                .get("category")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            if let Some(matched) = STRICT_STRUCTURAL_CATEGORIES
                .iter()
                .find(|c| **c == category)
            {
                let location = f
                    .get("location")
                    .and_then(|v| v.as_str())
                    .unwrap_or("-")
                    .to_string();
                hits.push((checkpoint, *matched, location));
            }
        }
    }
    hits
}

/// 7. SEAL — sign every commit then compile the signed audit report.
fn push_seal(steps: &mut Vec<Step>, opts: &CascadeOpts) {
    let p = &opts.project;
    if opts.dry_run {
        steps.push(Step::note(
            7,
            "audit sign-commits + report — skipped (dry-run)",
        ));
        return;
    }
    steps.push(Step::run(
        7,
        "audit sign-commits",
        vec![
            "audit".into(),
            "sign-commits".into(),
            "--project".into(),
            p.clone(),
        ],
    ));
    steps.push(Step::run(
        7,
        "audit report",
        vec![
            "audit".into(),
            "report".into(),
            "--project".into(),
            p.clone(),
        ],
    ));
}

pub fn run(db_path: &Path, action: CascadeAction, json_out: bool) -> Result<()> {
    match action {
        CascadeAction::Run {
            project,
            manifest,
            out,
            no_regenerate,
            per_dimension,
            merged_key,
            companion_key,
            thesis_key,
            bookkit_key,
            resume,
            force_full,
            dry_run,
            root,
            thesis_strict,
        } => {
            // Snapshot-by-default (ADR-0035): when --out is omitted, render into
            // an immutable timestamped snapshot dir rather than the live out/.
            let out = out.map_or_else(
                || {
                    let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
                    format!("snapshots/{ts}-books-cascade")
                },
                |p| p.to_string_lossy().to_string(),
            );
            let opts = CascadeOpts {
                project,
                manifest: manifest.to_string_lossy().to_string(),
                out,
                regenerate: !no_regenerate,
                per_dimension,
                merged_key,
                companion_key,
                thesis_key,
                bookkit_key,
                resume,
                force_full,
                dry_run,
                root: root.to_string_lossy().to_string(),
                thesis_strict,
            };
            run_cascade(db_path, &opts, json_out)
        }
    }
}

fn run_cascade(db_path: &Path, opts: &CascadeOpts, json_out: bool) -> Result<()> {
    let conn = agentic_core::db::open(db_path)?;

    // Discover the dimension sources (content store is the source of truth).
    let mut dim_paths: Vec<(u32, String)> =
        worktree::list(&conn, &opts.project, agentic_core::paths::SOURCES_PREFIX)?
            .into_iter()
            .filter_map(|(path, _sha)| dimension_index(&path).map(|n| (n, path)))
            .collect();
    dim_paths.sort_by_key(|(n, _)| *n);
    let dim_paths: Vec<String> = dim_paths.into_iter().map(|(_, p)| p).collect();

    // Does the inbox/ worktree prefix carry any items?
    let inbox_has_items = !worktree::list(&conn, &opts.project, "inbox")?.is_empty();

    // Compose the audit gate suite from the governed rule-matrix (ADR-0047 R4).
    let matrix = load_rule_matrix(&conn, &opts.project);
    let gate_suite = matrix.gate_suite();

    let plan = build_plan(opts, &dim_paths, inbox_has_items, &gate_suite);

    let exe = std::env::current_exe().context("locating the agentic executable")?;
    let db = db_path.to_string_lossy().to_string();

    let mut rows: Vec<(u8, String, String)> = Vec::new();
    let mut last_phase = 0u8;
    let mut any_fail = false;
    // Set only when --thesis-strict fires its HITL pause. Used to surface a
    // non-zero exit code so CI / wrapper scripts can distinguish a clean
    // cascade-with-advisory-WARNs from a structural-rule break that REFUSED
    // to seal.
    let mut strict_hitl_fired = false;

    // Checkpoint/resume (ADR-0047 R3): a content fingerprint scopes the
    // checkpoints; `--force-full` clears them; `--resume` skips expensive steps
    // already completed for the current fingerprint.
    let fingerprint = input_fingerprint(&conn, &opts.project);
    if opts.force_full && !opts.dry_run {
        clear_steps(&conn, &opts.project);
    }

    for step in &plan {
        // --thesis-strict HITL pause: when the phase transitions OUT of the
        // gate phase (6 → 7), inspect the bookkit-C structural findings just
        // recorded. Any PAGE_OVER / BOLD_OVERUSE / NON_ENGLISH / HEADING_DEPTH
        // halts the cascade before the seal step. Default off — preserves the
        // existing advisory-only behaviour.
        if opts.thesis_strict && last_phase == 6 && step.phase == 7 && !opts.dry_run {
            let violations = strict_structural_violations(db_path, &opts.project);
            if !violations.is_empty() {
                if !json_out {
                    println!(
                        "\n  \u{2716} [HITL PAUSE] --thesis-strict: bookkit-C structural-rule \
                         violation(s) detected — refusing to seal.\n"
                    );
                    println!("  The following findings must be cleared before phase 7 (SEAL):");
                    for (checkpoint, category, location) in &violations {
                        println!("    [{checkpoint:<14}] [{category:<14}] {location}");
                    }
                    println!(
                        "\n  Resolve, re-run the cascade (without --thesis-strict to bypass), \
                         OR run `agentic check {{page-boundary,bookkit}} \
                         --paths-from-manifest <m> --book-key <k>` directly to triage."
                    );
                }
                rows.push((7, "thesis-strict HITL pause".into(), "FAIL".into()));
                any_fail = true;
                strict_hitl_fired = true;
                break;
            }
        }

        // Print the phase banner once per phase boundary.
        if step.phase != last_phase && !json_out {
            println!("\n[cascade {}/7] {}", step.phase, phase_title(step.phase));
            last_phase = step.phase;
        } else if step.phase != last_phase {
            last_phase = step.phase;
        }

        if step.args.is_empty() {
            // Printed-intent placeholder.
            if !json_out {
                println!("  · {}", step.label);
            }
            rows.push((step.phase, step.label.clone(), "SKIP".into()));
            continue;
        }

        // Resume: skip an expensive step already completed for this fingerprint.
        let checkpointed = CHECKPOINTED_PHASES.contains(&step.phase);
        if checkpointed && opts.resume && step_done(&conn, &opts.project, &fingerprint, &step.label)
        {
            if !json_out {
                println!("  \u{27f3} {} (cached — unchanged inputs)", step.label);
            }
            rows.push((step.phase, step.label.clone(), "CACHED".into()));
            continue;
        }

        // Boot-gate `check self` FAIL is a hard stop (per the spec); every other
        // step continues and records its verdict.
        let mut cmd = ProcCommand::new(&exe);
        cmd.arg("--db").arg(&db).args(&step.args);
        // Sub-sessions need the subscription OAuth, not an API key.
        cmd.env_remove("ANTHROPIC_API_KEY");
        // ADR-0023 / cascade-phase-ordering remediation (2026-05-30):
        // mark every cascade-spawned subprocess so the aibom gate can
        // distinguish "user invoked `agentic check aibom` standalone
        // and a real unsigned commit exists" (FAIL) from "cascade
        // phase 6 ran the gate before phase 7 sign-commits had a
        // chance to sign newly-created phase-5-ingest commits"
        // (transient — INFO, not blocking). The seal step then
        // signs those commits and a re-run would PASS. Setting the
        // env per-Command keeps the parent-shell env clean and
        // avoids unsafe std::env::set_var under Rust 2024.
        cmd.env("AGENTIC_CASCADE_IN_PROGRESS", "1");
        if !json_out {
            println!("  → {}", step.label);
        }
        let status = cmd.status();
        let verdict = match status {
            Ok(st) => step.checkpoint.as_deref().map_or_else(
                || {
                    if st.success() {
                        "OK".to_string()
                    } else {
                        "FAIL".to_string()
                    }
                },
                |cp| {
                    latest_verdict(db_path, &opts.project, cp).unwrap_or_else(|| {
                        if st.success() {
                            "PASS".to_string()
                        } else {
                            "FAIL".to_string()
                        }
                    })
                },
            ),
            Err(e) => format!("ERR: {e}"),
        };
        if verdict == "FAIL" {
            any_fail = true;
        }
        // Checkpoint a completed expensive step so --resume can skip it next time.
        if checkpointed && matches!(verdict.as_str(), "OK" | "PASS" | "WARN") {
            record_step(&conn, &opts.project, &fingerprint, &step.label);
        }
        rows.push((step.phase, step.label.clone(), verdict.clone()));

        // Hard stop only on a failing boot-gate `check self`.
        if step.label == "check self" && verdict == "FAIL" {
            if !json_out {
                println!("\n  ✖ boot gate `check self` FAILED — stopping cascade.");
            }
            break;
        }
    }

    // ── final summary table ───────────────────────────────────────────────────
    if json_out {
        let arr: Vec<_> = rows
            .iter()
            .map(|(phase, label, verdict)| {
                json!({ "phase": phase, "step": label, "status": verdict })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "dry_run": opts.dry_run,
                "any_fail": any_fail,
                "steps": arr,
            }))?
        );
    } else {
        println!("\n=== CASCADE COMPLETE — step → status/verdict ===");
        for (phase, label, verdict) in &rows {
            println!("  [{phase}] {label:<28} {verdict}");
        }
        if any_fail {
            println!("\n  note: one or more steps reported FAIL (not aborted; review above).");
        }
    }
    // Non-zero exit only for the --thesis-strict HITL pause; advisory FAIL
    // verdicts on individual gates remain Ok(()) (existing convention) so a
    // single failing gate does not break the wrapper scripts that depend on
    // the cascade always returning successfully when it finished.
    if strict_hitl_fired {
        anyhow::bail!("cascade refused to seal — --thesis-strict HITL pause fired");
    }
    Ok(())
}

/// Title shown in the `[cascade N/7]` banner for each phase.
const fn phase_title(phase: u8) -> &'static str {
    match phase {
        1 => "BOOT GATE",
        2 => "INBOX INTAKE",
        3 => "REGENERATE DIMENSIONS",
        4 => "MERGE",
        5 => "BUILD BOOK",
        6 => "AUDIT GATES",
        7 => "SEAL",
        _ => "?",
    }
}

/// The verdict value (PASS/WARN/FAIL) of the newest verdict row for a checkpoint.
fn latest_verdict(db_path: &Path, project: &str, checkpoint: &str) -> Option<String> {
    let conn = agentic_core::db::open(db_path).ok()?;
    conn.query_row(
        "SELECT verdict FROM audit_verdicts WHERE project_id = ?1 AND checkpoint = ?2 \
         ORDER BY id DESC LIMIT 1",
        rusqlite::params![project, checkpoint],
        |r| r.get::<_, String>(0),
    )
    .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(dry_run: bool, per_dimension: bool, regenerate: bool) -> CascadeOpts {
        CascadeOpts {
            project: "P".into(),
            manifest: "out/book_manifest.json".into(),
            out: "out/books".into(),
            regenerate,
            per_dimension,
            merged_key: "governing_the_agentic_machine".into(),
            companion_key: "student_notes".into(),
            thesis_key: "master_thesis".into(),
            bookkit_key: "master_thesis_bookkit".into(),
            resume: false,
            force_full: false,
            dry_run,
            root: ".".into(),
            thesis_strict: false,
        }
    }

    fn dims() -> Vec<String> {
        vec![
            "out/sources/Dimension_01_agile_leadership_EN.md".into(),
            "out/sources/Dimension_02_cybersecurity_ai_EN.md".into(),
        ]
    }

    /// The default gate suite, as the cascade resolves it from the rule-matrix.
    fn suite() -> Vec<(&'static str, &'static str)> {
        agentic_core::profiles::RuleMatrix::default_matrix().gate_suite()
    }

    #[test]
    fn split_dim_extracts_nn_and_slug() {
        assert_eq!(
            split_dim("out/sources/Dimension_03_intelligent_agents_EN.md"),
            Some(("03".into(), "intelligent_agents".into()))
        );
        assert_eq!(
            split_dim("Dimension_11_productized_builder_commercial_EN.md"),
            Some(("11".into(), "productized_builder_commercial".into()))
        );
        assert_eq!(split_dim("not_a_dimension.md"), None);
    }

    #[test]
    fn regen_prompt_mentions_file_english_and_figspec() {
        let p = regen_prompt("03", "intelligent_agents");
        assert!(p.contains("Dimension_03_intelligent_agents_EN.md"));
        assert!(p.contains("English-only"));
        assert!(p.contains("```figspec```"));
        assert!(p.trim_end().ends_with("Then stop."));
    }

    #[test]
    fn dry_run_produces_the_seven_phase_plan() {
        let plan = build_plan(&opts(true, false, true), &dims(), false, &suite());
        // All seven phases must be represented exactly once-or-more, in order.
        let phases: Vec<u8> = plan.iter().map(|s| s.phase).collect();
        for n in 1u8..=7 {
            assert!(phases.contains(&n), "phase {n} missing from dry-run plan");
        }
        // Phases appear in non-decreasing order (no interleaving).
        assert!(
            phases.windows(2).all(|w| w[0] <= w[1]),
            "phases out of order"
        );
        // Dry-run records NO subprocess for regeneration / merge / build / seal.
        assert!(
            plan.iter()
                .filter(|s| s.phase == 3)
                .all(|s| s.args.is_empty())
        );
        assert!(
            plan.iter()
                .filter(|s| s.phase == 4)
                .all(|s| s.args.is_empty())
        );
        assert!(
            plan.iter()
                .filter(|s| s.phase == 5)
                .all(|s| s.args.is_empty())
        );
        assert!(
            plan.iter()
                .filter(|s| s.phase == 7)
                .all(|s| s.args.is_empty())
        );
        // The gate suite (phase 6) runs every gate the rule-matrix composes,
        // plus an extra invocation per scoped key for page-boundary + bookkit
        // (now fanned out across thesis_key + bookkit_key).
        let scoped_extras = 2; // page-boundary and bookkit each get +1 extra step
        assert_eq!(
            plan.iter().filter(|s| s.phase == 6).count(),
            suite().len() + scoped_extras
        );
    }

    #[test]
    fn default_builds_three_profiles_per_dimension_builds_all() {
        // Default (no --per-dimension): four profile builds (A/B/C + bookkit),
        // each --only.
        let plan = build_plan(&opts(false, false, false), &dims(), false, &suite());
        let profile_builds: Vec<&Step> = plan
            .iter()
            .filter(|s| s.label.starts_with("book build ") && s.label != "book build (all)")
            .collect();
        assert_eq!(
            profile_builds.len(),
            4,
            "A + B + C + bookkit profile builds"
        );
        assert!(
            profile_builds
                .iter()
                .all(|s| s.args.contains(&"--only".to_string()))
        );
        let keys: Vec<String> = profile_builds
            .iter()
            .filter_map(|s| s.args.iter().skip_while(|a| *a != "--only").nth(1).cloned())
            .collect();
        assert!(keys.contains(&"governing_the_agentic_machine".to_string()));
        assert!(keys.contains(&"student_notes".to_string()));
        assert!(keys.contains(&"master_thesis".to_string()));
        assert!(keys.contains(&"master_thesis_bookkit".to_string()));

        // With --per-dimension: one all-books build, `--only` omitted.
        let plan = build_plan(&opts(false, true, false), &dims(), false, &suite());
        let build = plan
            .iter()
            .find(|s| s.label == "book build (all)")
            .expect("all-books build present");
        assert!(!build.args.contains(&"--only".to_string()));
    }

    #[test]
    fn regenerate_adds_one_session_per_dimension_plus_run() {
        let plan = build_plan(&opts(false, false, true), &dims(), false, &suite());
        let adds = plan
            .iter()
            .filter(|s| s.label.starts_with("orchestrate add dim"))
            .count();
        assert_eq!(adds, 2, "one orchestrate-add per discovered dimension");
        assert!(plan.iter().any(|s| s.label == "orchestrate run --wave"));
    }

    #[test]
    fn no_regenerate_skips_phase_three() {
        let plan = build_plan(&opts(false, false, false), &dims(), false, &suite());
        assert!(
            plan.iter()
                .filter(|s| s.phase == 3)
                .all(|s| s.args.is_empty())
        );
        assert!(!plan.iter().any(|s| s.label.starts_with("orchestrate add")));
    }

    #[test]
    fn inbox_intake_runs_only_when_items_present() {
        let with = build_plan(&opts(false, false, false), &dims(), true, &suite());
        assert!(with.iter().any(|s| s.label == "import dir inbox"));
        assert!(with.iter().any(|s| s.label == "embed inbox"));
        assert!(with.iter().any(|s| s.label == "classify inbox"));

        let without = build_plan(&opts(false, false, false), &dims(), false, &suite());
        assert!(without.iter().any(|s| s.label == "inbox empty — skipping"));
        assert!(!without.iter().any(|s| s.label == "import dir inbox"));

        // dry-run with items present is intent-only (no mutating subprocess).
        let dry = build_plan(&opts(true, false, false), &dims(), true, &suite());
        assert!(
            dry.iter()
                .filter(|s| s.phase == 2)
                .all(|s| s.args.is_empty())
        );
        assert!(!dry.iter().any(|s| s.label == "import dir inbox"));
    }

    #[test]
    fn gate_suite_has_distinct_checkpoints() {
        use std::collections::HashSet;
        let s = suite();
        let cps: HashSet<&str> = s.iter().map(|(_, cp)| *cp).collect();
        // Default matrix = the full catalog minus catalog-only registrations
        // (currently just `artefact-cap` — ADR-0055; no CLI subcommand yet).
        // Universal 28 + C additions 3 = 31 invocable gates; catalog 32.
        // See profiles.rs `default_matrix_suite_is_full_catalog` for the
        // authoritative invariant.
        let catalog_only = ["artefact-cap"];
        assert_eq!(
            s.len(),
            agentic_core::profiles::GATE_CATALOG.len() - catalog_only.len()
        );
        assert_eq!(cps.len(), s.len(), "checkpoint names must be distinct");
        // contamination runs offline; tree/docs carry --root.
        let plan = build_plan(&opts(true, false, true), &dims(), false, &suite());
        let contam = plan
            .iter()
            .find(|s| s.label == "check contamination")
            .unwrap();
        assert!(contam.args.contains(&"--offline".to_string()));
    }

    #[test]
    fn thesis_profile_gates_receive_manifest_scope_args() {
        // Regression for the 2026-05-28 scope mismatch: the thesis-profile
        // page_boundary and bookkit gates must be invoked with
        // --paths-from-manifest + --book-key=<thesis_key> so they measure
        // exactly the chapter list of the rendered master_thesis.docx.
        // page_boundary additionally passes --words-per-page=280 (the
        // empirical FHNW Word render density). Other gates get no extra args.
        let plan = build_plan(&opts(true, false, false), &dims(), false, &suite());
        let pb = plan
            .iter()
            .find(|s| s.label == "check page-boundary (master_thesis)")
            .expect("page-boundary scoped to master_thesis present in gate suite");
        assert!(pb.args.contains(&"--paths-from-manifest".to_string()));
        assert!(pb.args.contains(&"out/book_manifest.json".to_string()));
        assert!(pb.args.contains(&"--book-key".to_string()));
        assert!(pb.args.contains(&"master_thesis".to_string()));
        assert!(pb.args.contains(&"--words-per-page".to_string()));
        assert!(pb.args.contains(&"280".to_string()));

        let pb_bk = plan
            .iter()
            .find(|s| s.label == "check page-boundary (master_thesis_bookkit)")
            .expect("page-boundary scoped to bookkit present in gate suite");
        assert!(pb_bk.args.contains(&"master_thesis_bookkit".to_string()));

        let bk = plan
            .iter()
            .find(|s| s.label == "check bookkit (master_thesis)")
            .expect("bookkit scoped to master_thesis present in gate suite");
        assert!(bk.args.contains(&"--paths-from-manifest".to_string()));
        assert!(bk.args.contains(&"--book-key".to_string()));
        assert!(bk.args.contains(&"master_thesis".to_string()));
        // bookkit must NOT receive --words-per-page (it has no such concept).
        assert!(!bk.args.contains(&"--words-per-page".to_string()));

        let bk_bk = plan
            .iter()
            .find(|s| s.label == "check bookkit (master_thesis_bookkit)")
            .expect("bookkit scoped to bookkit present in gate suite");
        assert!(bk_bk.args.contains(&"master_thesis_bookkit".to_string()));

        // Other gates (e.g. citations) must NOT receive the manifest args.
        if let Some(cit) = plan.iter().find(|s| s.label == "check citations") {
            assert!(!cit.args.contains(&"--paths-from-manifest".to_string()));
            assert!(!cit.args.contains(&"--book-key".to_string()));
        }
    }

    #[test]
    fn cascade_parity_step_supplies_book_and_reference() {
        // Regression for the 2026-06-06 parity FAIL: the cascade orchestrator
        // used to fall through to the default `_ =>` arm and invoke
        // `agentic check parity --project <p>` with no `--book` or
        // `--reference`, which clap rejected as a required-arg error before
        // any per-book dispatch could run. The new `parity` arm in
        // `push_audit_gates` MUST emit one `check parity` step per
        // (book_key, canonical_reference_path) pair, and skip books with no
        // canonical baseline (so the old-pipeline `master_thesis` book is
        // not emitted — only `master_thesis_bookkit`).
        let plan = build_plan(&opts(true, false, false), &dims(), false, &suite());
        let parity_steps: Vec<&Step> = plan
            .iter()
            .filter(|s| s.label.starts_with("check parity"))
            .collect();
        // Exactly one parity step: master_thesis_bookkit (master_thesis has
        // no canonical reference; parity::canonical_reference_path returns
        // None and the arm skips it).
        assert_eq!(
            parity_steps.len(),
            1,
            "parity must emit exactly one step (master_thesis_bookkit; \
             master_thesis has no canonical reference)"
        );
        let step = parity_steps[0];
        assert_eq!(step.label, "check parity (master_thesis_bookkit)");
        // Required args present and correctly resolved.
        assert!(step.args.contains(&"--book".to_string()));
        assert!(step.args.contains(&"master_thesis_bookkit".to_string()));
        assert!(step.args.contains(&"--reference".to_string()));
        // Reference path comes from parity::canonical_reference_path —
        // forward-slash-normalised on Windows so display() output matches
        // the expected literal exactly.
        let expected_ref =
            agentic_checks::parity::canonical_reference_path("master_thesis_bookkit")
                .expect("canonical reference for bookkit")
                .display()
                .to_string();
        assert!(
            step.args.contains(&expected_ref),
            "parity step must carry the canonical reference path; args = {:?}",
            step.args
        );
    }

    #[test]
    fn cascade_opts_default_bookkit_key() {
        // The test fixture mirrors the clap default for --bookkit-key: when
        // callers don't override it, the cascade composes its bookkit step +
        // scoped page-boundary/bookkit gates against `master_thesis_bookkit`.
        let o = opts(true, false, false);
        assert_eq!(o.bookkit_key, "master_thesis_bookkit");

        // And the plan actually wires that default through the build + gate
        // phases (label fan-out is the observable contract).
        let plan = build_plan(&o, &dims(), false, &suite());
        // Dry-run skips phase-5 build steps (they're intent-only), so check
        // a non-dry-run build instead.
        let live = opts(false, false, false);
        let live_plan = build_plan(&live, &dims(), false, &suite());
        assert!(
            live_plan
                .iter()
                .any(|s| s.label == "book build bookkit (master_thesis_bookkit)"),
            "phase-5 must emit a 4th `book build bookkit` step for the default key"
        );
        // page-boundary / bookkit fan-out is visible even in dry-run (they're
        // gate steps, which dry-run preserves).
        assert!(
            plan.iter()
                .any(|s| s.label == "check page-boundary (master_thesis_bookkit)"),
            "page-boundary must fan out to the default bookkit key"
        );
        assert!(
            plan.iter()
                .any(|s| s.label == "check bookkit (master_thesis_bookkit)"),
            "bookkit must fan out to the default bookkit key"
        );
    }

    #[test]
    fn strict_structural_violations_recognises_categories() {
        // Unit-test the parser side of `strict_structural_violations` by
        // checking the category list is exactly the four bookkit-C structural
        // rules — adding a fifth here without updating the gate is a guard
        // against silent scope drift.
        let expected = ["PAGE_OVER", "BOLD_OVERUSE", "NON_ENGLISH", "HEADING_DEPTH"];
        assert_eq!(STRICT_STRUCTURAL_CATEGORIES.len(), expected.len());
        for cat in expected {
            assert!(
                STRICT_STRUCTURAL_CATEGORIES.contains(&cat),
                "missing structural category: {cat}"
            );
        }
    }
}
