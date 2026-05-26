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
    dry_run: bool,
    root: String,
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

/// Per-profile gate subsets (ADR-0045). The UNIVERSAL gates apply to every
/// bookkit profile (A books, B companion, C thesis) — they are about
/// correctness and house style, not regulated thesis structure. The THESIS
/// gates apply only to bookkit C (the master thesis): the 60-page body bound,
/// the R&R traceability matrix, and reviewer calibration. Each entry is
/// `(check subcommand, verdict checkpoint)`.
const UNIVERSAL_GATES: &[(&str, &str)] = &[
    ("self", "self"),
    ("tree", "tree"),
    ("deliverable", "deliverable"),
    ("citations", "citation_tracker"),
    ("contamination", "contamination"),
    ("bibliography", "bibliography"),
    ("aibom", "aibom"),
    ("docs", "docs"),
    ("facts-integrity", "facts_integrity"),
    ("i18n", "i18n"),
    ("bookkit", "bookkit"),
    ("prisma", "prisma"),
    ("cross-model", "cross_model"),
    ("temporal", "temporal"),
    ("ground-truth", "ground_truth"),
    ("compliance", "compliance"),
    ("sprint", "sprint"),
    ("predatory", "predatory"),
    ("reproducibility", "reproducibility"),
    ("integrity", "integrity"),
    ("figure-quality", "figure_quality"),
    ("disclosure", "disclosure"),
];

/// Bookkit-C-only gates (master thesis): page boundary, R&R matrix, calibration.
const THESIS_GATES: &[(&str, &str)] = &[
    ("page-boundary", "page_boundary"),
    ("rr-matrix", "rr_matrix"),
    ("calibration", "calibration"),
];

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
fn build_plan(opts: &CascadeOpts, dim_paths: &[String], inbox_has_items: bool) -> Vec<Step> {
    let mut steps = Vec::new();
    push_boot_gate(&mut steps, opts);
    push_inbox_intake(&mut steps, opts, inbox_has_items);
    push_regenerate(&mut steps, opts, dim_paths);
    push_merge(&mut steps, opts);
    push_build_book(&mut steps, opts);
    push_audit_gates(&mut steps, opts);
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
    // The manifest is authoritative in `thesis.db` (gitignored on disk), so
    // materialise it from the DB first — otherwise `book build` fails with
    // "cannot find the file" on a clean checkout.
    steps.push(Step::run(
        5,
        "checkout manifest",
        vec![
            "content".into(),
            "checkout".into(),
            "--project".into(),
            opts.project.clone(),
            "--to".into(),
            opts.root.clone(),
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
            opts.manifest.clone(),
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
    }
}

/// 6. AUDIT GATES — per-profile subsets (ADR-0045): the universal gates apply to
/// every profile; the thesis-only gates apply to bookkit C (always built here).
/// Each records its own verdict.
fn push_audit_gates(steps: &mut Vec<Step>, opts: &CascadeOpts) {
    let p = &opts.project;
    let emit = |steps: &mut Vec<Step>, sub: &str, cp: &str, tag: &str| {
        let mut args = vec!["check".into(), sub.into(), "--project".into(), p.clone()];
        match sub {
            "tree" | "docs" => {
                args.push("--root".into());
                args.push(opts.root.clone());
            }
            "contamination" => args.push("--offline".into()),
            _ => {}
        }
        steps.push(Step::gate(6, format!("check {sub}{tag}"), args, cp));
    };
    for (sub, cp) in UNIVERSAL_GATES {
        emit(steps, sub, cp, "");
    }
    // Bookkit C — the master thesis is always a cascade deliverable.
    for (sub, cp) in THESIS_GATES {
        emit(steps, sub, cp, " [C]");
    }
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
            dry_run,
            root,
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
                dry_run,
                root: root.to_string_lossy().to_string(),
            };
            run_cascade(db_path, &opts, json_out)
        }
    }
}

fn run_cascade(db_path: &Path, opts: &CascadeOpts, json_out: bool) -> Result<()> {
    let conn = agentic_core::db::open(db_path)?;

    // Discover the dimension sources (content store is the source of truth).
    let mut dim_paths: Vec<(u32, String)> = worktree::list(&conn, &opts.project, "out/sources/")?
        .into_iter()
        .filter_map(|(path, _sha)| dimension_index(&path).map(|n| (n, path)))
        .collect();
    dim_paths.sort_by_key(|(n, _)| *n);
    let dim_paths: Vec<String> = dim_paths.into_iter().map(|(_, p)| p).collect();

    // Does the inbox/ worktree prefix carry any items?
    let inbox_has_items = !worktree::list(&conn, &opts.project, "inbox")?.is_empty();

    let plan = build_plan(opts, &dim_paths, inbox_has_items);

    let exe = std::env::current_exe().context("locating the agentic executable")?;
    let db = db_path.to_string_lossy().to_string();

    let mut rows: Vec<(u8, String, String)> = Vec::new();
    let mut last_phase = 0u8;
    let mut any_fail = false;

    for step in &plan {
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

        // Boot-gate `check self` FAIL is a hard stop (per the spec); every other
        // step continues and records its verdict.
        let mut cmd = ProcCommand::new(&exe);
        cmd.arg("--db").arg(&db).args(&step.args);
        // Sub-sessions need the subscription OAuth, not an API key.
        cmd.env_remove("ANTHROPIC_API_KEY");
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
            dry_run,
            root: ".".into(),
        }
    }

    fn dims() -> Vec<String> {
        vec![
            "out/sources/Dimension_01_agile_leadership_EN.md".into(),
            "out/sources/Dimension_02_cybersecurity_ai_EN.md".into(),
        ]
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
        let plan = build_plan(&opts(true, false, true), &dims(), false);
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
        // The gate suite (phase 6) runs all read-only gates (universal + thesis).
        assert_eq!(
            plan.iter().filter(|s| s.phase == 6).count(),
            UNIVERSAL_GATES.len() + THESIS_GATES.len()
        );
    }

    #[test]
    fn default_builds_three_profiles_per_dimension_builds_all() {
        // Default (no --per-dimension): three profile builds (A/B/C), each --only.
        let plan = build_plan(&opts(false, false, false), &dims(), false);
        let profile_builds: Vec<&Step> = plan
            .iter()
            .filter(|s| s.label.starts_with("book build ") && s.label != "book build (all)")
            .collect();
        assert_eq!(profile_builds.len(), 3, "A + B + C profile builds");
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

        // With --per-dimension: one all-books build, `--only` omitted.
        let plan = build_plan(&opts(false, true, false), &dims(), false);
        let build = plan
            .iter()
            .find(|s| s.label == "book build (all)")
            .expect("all-books build present");
        assert!(!build.args.contains(&"--only".to_string()));
    }

    #[test]
    fn regenerate_adds_one_session_per_dimension_plus_run() {
        let plan = build_plan(&opts(false, false, true), &dims(), false);
        let adds = plan
            .iter()
            .filter(|s| s.label.starts_with("orchestrate add dim"))
            .count();
        assert_eq!(adds, 2, "one orchestrate-add per discovered dimension");
        assert!(plan.iter().any(|s| s.label == "orchestrate run --wave"));
    }

    #[test]
    fn no_regenerate_skips_phase_three() {
        let plan = build_plan(&opts(false, false, false), &dims(), false);
        assert!(
            plan.iter()
                .filter(|s| s.phase == 3)
                .all(|s| s.args.is_empty())
        );
        assert!(!plan.iter().any(|s| s.label.starts_with("orchestrate add")));
    }

    #[test]
    fn inbox_intake_runs_only_when_items_present() {
        let with = build_plan(&opts(false, false, false), &dims(), true);
        assert!(with.iter().any(|s| s.label == "import dir inbox"));
        assert!(with.iter().any(|s| s.label == "embed inbox"));
        assert!(with.iter().any(|s| s.label == "classify inbox"));

        let without = build_plan(&opts(false, false, false), &dims(), false);
        assert!(without.iter().any(|s| s.label == "inbox empty — skipping"));
        assert!(!without.iter().any(|s| s.label == "import dir inbox"));

        // dry-run with items present is intent-only (no mutating subprocess).
        let dry = build_plan(&opts(true, false, false), &dims(), true);
        assert!(
            dry.iter()
                .filter(|s| s.phase == 2)
                .all(|s| s.args.is_empty())
        );
        assert!(!dry.iter().any(|s| s.label == "import dir inbox"));
    }

    #[test]
    fn gate_suite_has_twentyfive_distinct_checkpoints() {
        use std::collections::HashSet;
        let cps: HashSet<&str> = UNIVERSAL_GATES
            .iter()
            .chain(THESIS_GATES.iter())
            .map(|(_, cp)| *cp)
            .collect();
        assert_eq!(UNIVERSAL_GATES.len() + THESIS_GATES.len(), 25);
        assert_eq!(cps.len(), 25, "checkpoint names must be distinct");
        // contamination runs offline; tree/docs carry --root.
        let plan = build_plan(&opts(true, false, true), &dims(), false);
        let contam = plan
            .iter()
            .find(|s| s.label == "check contamination")
            .unwrap();
        assert!(contam.args.contains(&"--offline".to_string()));
    }
}
