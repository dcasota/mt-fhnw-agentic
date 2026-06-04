//! CLI argument parsing.
//!
//! The structure mirrors the command tree from the plan (section 4).
//! Only commands whose handlers exist in [`crate::commands`] are wired
//! here; the rest are stubbed and emit "not yet implemented" at runtime.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "agentic",
    version,
    about = "Monolithic Rust CLI + SQLite repository for agentic thesis work",
    long_about = None,
)]
pub struct Cli {
    /// Path to the SQLite database. Default: `./thesis.db`.
    #[arg(long, env = "AGENTIC_DB", global = true, default_value = "thesis.db")]
    pub db: PathBuf,

    /// Output language (en|de|fr|it|rm|hi). Default: en.
    #[arg(long, env = "AGENTIC_LANG", global = true, default_value = "en")]
    pub lang: String,

    /// Emit machine-readable JSON instead of human-readable text.
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Initialise a new thesis.db (launches the wizard unless flags are set).
    Init(InitArgs),

    /// Project lifecycle (new / list / switch / status / archive).
    Project {
        #[command(subcommand)]
        action: ProjectAction,
    },

    /// Material-passport operations (append/read/validate/reset-boundary/repro-lock).
    Passport {
        #[command(subcommand)]
        action: PassportAction,
    },

    /// Journal operations (append/show/search).
    Journal {
        #[command(subcommand)]
        action: JournalAction,
    },

    /// Content store: blobs, trees, commits, refs.
    Content {
        #[command(subcommand)]
        action: ContentAction,
    },

    /// Irreversible-action authorisations (ADR-0047): grant/list the audited
    /// records that the tool requires before performing push/tag/publish/
    /// translate/supersede/content-delete.
    Authorize {
        #[command(subcommand)]
        action: AuthorizeAction,
    },

    /// SDD-Cycle promotion gate (ADR-0047 R6/R8): evaluate the latest gate
    /// verdicts; BLOCK on any FAIL, else record the promotion (a
    /// `submission_freeze` verdict) and issue authorisations for held actions.
    Promote {
        #[arg(long)]
        project: String,
        /// Irreversible action(s) to authorise on a clean promotion (repeatable),
        /// e.g. --issue publish --issue translate.
        #[arg(long = "issue")]
        issue: Vec<String>,
        /// Optional snapshot path to record as the promoted/citable deliverable.
        #[arg(long)]
        snapshot: Option<String>,
    },

    /// Integrity checkers (self / writing-quality / citations / contamination / ...).
    Check {
        #[command(subcommand)]
        action: CheckAction,
    },

    /// Per-dimension APA7 bibliography: harvest web/email/user traces into the
    /// material passport, and emit the reference list (non-repudiation, Phase 1).
    Bibliography {
        #[command(subcommand)]
        action: BibAction,
    },

    /// Verified-facts backbone (ADR-0016/0042): anchor recurring claims to
    /// provenance-bearing records so numbers resolve against a signed fact.
    Facts {
        #[command(subcommand)]
        action: FactsAction,
    },

    /// PRISMA claim→source map (ADR-0020/0026): coverage of in-text citations
    /// against the reference universe.
    Prisma {
        #[command(subcommand)]
        action: PrismaAction,
    },

    /// Independent verification (ADR-0028): cross-model attestation of a fact.
    Verify {
        #[command(subcommand)]
        action: VerifyAction,
    },

    /// LLM document/ranking review (ADR-0049): a second model (e.g. xAI Grok)
    /// reviews each deliverable class and the rankings; verdicts are recorded as
    /// signed `claim_audit_results` (kind=model_review) that feed cascade adoption.
    Review {
        #[command(subcommand)]
        action: ReviewAction,
    },

    /// Diagnose the environment + binary configuration.
    Doctor,

    /// Provider management (list providers, set API keys, smoke-test).
    Provider {
        #[command(subcommand)]
        action: ProviderAction,
    },

    /// Configuration (key/value pairs persisted in the DB; mirrors env vars).
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },

    /// Import a proposal / draft (markdown / DOCX / PDF) into a project.
    Import {
        #[command(subcommand)]
        action: ImportAction,
    },

    /// Turnkey migration: create a fresh project and ingest an entire
    /// legacy directory (FACTORYAI / interim-presentation layout) into it.
    Migrate {
        /// Source directory to migrate.
        src: PathBuf,
        /// Project name. Defaults to the source directory's basename.
        #[arg(long)]
        name: Option<String>,
        #[arg(long, default_value = "en")]
        working_lang: String,
        #[arg(long)]
        institution: Option<String>,
        #[arg(long)]
        track: Option<String>,
        /// Embed every imported markdown blob after migration.
        #[arg(long)]
        embed: bool,
        /// Force a provider for embeddings.
        #[arg(long)]
        provider: Option<String>,
        /// Force a model for embeddings.
        #[arg(long)]
        model: Option<String>,
    },

    /// Embed every markdown chapter in a project (vectors stored in DB).
    Embed {
        /// Project ID (ULID).
        project: String,
        /// Restrict to a working-tree path prefix (e.g. "thesis-draft/").
        #[arg(long, default_value = "")]
        prefix: String,
        /// Force a specific provider (override the router).
        #[arg(long)]
        provider: Option<String>,
        /// Force a specific model.
        #[arg(long)]
        model: Option<String>,
        /// Re-embed chapters that already have a stored vector for this model.
        #[arg(long)]
        force: bool,
    },

    /// Classify chapters against thesis-chapter slots. Two strategies:
    /// `embed` (cosine on embeddings; needs an embed-capable provider)
    /// or `chat` (LLM-driven; works with any chat-capable provider). If
    /// `--strategy` is omitted, picks `embed` when an embed-capable
    /// provider is configured, else `chat`.
    Classify {
        /// Project ID (ULID).
        project: String,
        /// Restrict to a working-tree path prefix.
        #[arg(long, default_value = "")]
        prefix: String,
        /// Comma-separated slot keys. If omitted, the six standard thesis chapters are used.
        #[arg(long)]
        slots: Option<String>,
        /// Force `embed` or `chat`. If omitted, auto-detected from configured providers.
        #[arg(long, value_parser = ["embed", "chat"])]
        strategy: Option<String>,
        /// Force a specific provider (override the router).
        #[arg(long)]
        provider: Option<String>,
        /// Force a specific model.
        #[arg(long)]
        model: Option<String>,
    },

    /// Export a project to DOCX or PDF.
    Export {
        /// Project ID (ULID).
        project: String,
        /// Output format.
        #[arg(long, default_value = "docx", value_parser = ["docx", "pdf"])]
        format: String,
        /// Output path. If omitted, writes bytes to stdout.
        #[arg(long)]
        to: Option<std::path::PathBuf>,
        /// Restrict to a path prefix in the working tree (e.g. "thesis-draft/").
        #[arg(long, default_value = "")]
        prefix: String,
        /// Title for the first page (falls back to project name).
        #[arg(long)]
        title: Option<String>,
    },

    /// Audit + non-repudiation: PQC (ML-DSA-87) signing and complete audit
    /// reports (user actions, APA7 source origins, AI-decision index).
    Audit {
        #[command(subcommand)]
        action: AuditAction,
    },

    /// Inbox lifecycle: register / status / accept / skip / retire / dedup.
    /// Processed items are retired (disk copy removed; the DB blob is the
    /// permanent archive — "empty inbox = done").
    Inbox {
        #[command(subcommand)]
        action: InboxAction,
    },

    /// External AI-platform session ingest (ADR-0053): record a session
    /// from grok.com / gemini.google.com / chatgpt.com / claude.ai /
    /// perplexity.ai / other into the AIBOM. The captured export file
    /// is stored as a content-addressed blob (ML-DSA-87 signed at
    /// import); the row in `external_sessions` carries the author's
    /// attestation that makes the entry the audit anchor.
    #[command(name = "external-session")]
    ExternalSession {
        #[command(subcommand)]
        action: ExternalSessionAction,
    },

    /// Refresh the Pages column of the front-matter acronyms table from a
    /// rendered `master_thesis.docx`. Uses the v4 token-aware algorithm
    /// (case-sensitive substring scan with alphanumeric-neighbour
    /// post-filter) so `AI` matches "AI-First" but not "AIBOM".
    /// Pages come from Word's `lastRenderedPageBreak` hints embedded in
    /// `word/document.xml` (written by Word on every save — the
    /// `agentic book finalize` step guarantees this).
    Acronyms {
        #[command(subcommand)]
        action: AcronymsAction,
    },

    /// Deterministically normalise content-store markdown (expand prediction×mode
    /// codes, shorten over-long figure captions, apply verified-facts
    /// corrections). Writes changed blobs back in a single commit.
    Normalize {
        #[arg(long)]
        project: String,
        #[arg(long, default_value = agentic_core::paths::SOURCES_PREFIX)]
        prefix: String,
    },

    /// Render professional DOCX books (the Rust book engine) and audit render
    /// quality against the previous iteration.
    Book {
        #[command(subcommand)]
        action: BookAction,
    },

    /// Assemble a generation prompt with the mandatory rules prepended (Rust port
    /// of prompt_rules.py + gen_*.py). Pipe the output to your LLM CLI.
    Gen {
        #[command(subcommand)]
        action: GenAction,
    },

    /// RAMP — Risk-Adjusted Metadata Prediction (ADR-0040): predict SLOC,
    /// human-hours, impact, risk, market projections and reskilling surge from
    /// declared metadata, across region/Broadcom scenarios and operating modes.
    Risk {
        #[command(subcommand)]
        action: RiskAction,
    },

    /// Bounded autonomous sub-session orchestration (in-tool replacement for the
    /// external PowerShell launcher). Manages `claude`-CLI sub-sessions with a
    /// fixed budget + guard prompt; state lives in the DB, transcripts on disk.
    Orchestrate {
        #[command(subcommand)]
        action: OrchestrateAction,
    },

    /// Ranking summary (perception P-4): aggregate ADR-0046 acceptance per
    /// section. Walks `claim_audit_results` ranking-kind entries, classifies
    /// each by section, and counts Critical / High / Medium tiers + the
    /// LowRankings holdouts. Markdown / JSON output.
    Rank {
        #[command(subcommand)]
        action: RankAction,
    },

    /// Cross-stream synthesis (perception P-3): an LLM proposes draft
    /// cross-dimension findings from the Critical+High dimension findings
    /// and the column-max campaign findings the runtime already accepted.
    /// Writes a *candidate* file under `out/sources/synthesis/`;
    /// never auto-promotes to the student-notes companion or to the thesis.
    Synthesize {
        #[command(subcommand)]
        action: SynthesizeAction,
    },

    /// First-class named profile bundles (perception P-1): reusable JSON
    /// configuration sets the operator can attach to one or more sections.
    /// Stored in the passport (`profiles` section); latest-wins per name.
    Profile {
        #[command(subcommand)]
        action: ProfileAction,
    },

    /// Content translation (perception P-5). The chrome-only `--lang` flag
    /// already exists on every command; this verb releases the held *content*
    /// translation pipeline for an explicit operator-chosen scope. Gated by
    /// `agentic authorize grant --action translate` (the runtime's
    /// irreversible-action discipline; ADR-0047) because translation calls
    /// out to an LLM provider and writes new content blobs.
    Translate {
        #[command(subcommand)]
        action: TranslateAction,
    },

    /// Source-merge utilities (codifies manual assembly steps). Currently:
    /// assemble the eleven dimension sources into one English compendium.
    Merge {
        #[command(subcommand)]
        action: MergeAction,
    },

    /// Monolithic SDD full-cycle orchestrator. Chains the EXISTING in-tool steps
    /// into the enforced cascade: boot gate → inbox intake/rank → (auto)
    /// regenerate dimensions → merge → build the MERGED book → gate suite → seal.
    /// Composes existing commands; does not reimplement them.
    Cascade {
        #[command(subcommand)]
        action: CascadeAction,
    },

    /// Figure-asset utilities (extraction, manifests). The Rust renderer in
    /// `agentic-figures` only handles figspec→PNG; this verb covers the bulk
    /// asset-pipeline steps the renderer does not.
    Figures {
        #[command(subcommand)]
        action: FiguresAction,
    },
}

#[derive(Debug, Subcommand)]
pub enum FiguresAction {
    /// Extract every embedded raster from a `.docx` (treated as a zip) into
    /// `<out>/` per source filename. Useful for harvesting the
    /// `word/media/` assets of a reference book so they can be authored as
    /// `image-embed` figspecs (Wave-5 AI-Norms parity flow).
    ///
    /// The output is a flat copy of every file under `word/media/` — no
    /// transcoding, no filtering, no rewriting. Optional `--min-bytes` skips
    /// trivial admonition glyphs (default 0 = keep all).
    ExtractFromDocx {
        /// Source docx to harvest.
        #[arg(long)]
        docx: PathBuf,
        /// Destination directory; created if missing.
        #[arg(long)]
        out: PathBuf,
        /// Skip files smaller than this many bytes (e.g. 5120 to drop tiny
        /// admonition icons). 0 keeps everything.
        #[arg(long, default_value_t = 0u64)]
        min_bytes: u64,
    },
}

#[derive(Debug, Subcommand)]
pub enum CascadeAction {
    /// Run the full 7-step SDD cascade end-to-end.
    Run {
        /// Project ID (ULID).
        #[arg(long)]
        project: String,
        /// Book manifest consumed by the build step.
        #[arg(long, default_value = agentic_core::paths::MANIFEST_PATH)]
        manifest: PathBuf,
        /// Output directory for the rendered .docx books. If omitted, the
        /// cascade renders into an immutable timestamped snapshot
        /// `snapshots/<ts>-books-cascade/` (ADR-0035 snapshot convention).
        #[arg(long)]
        out: Option<PathBuf>,
        /// Skip regenerating the dimension sources before merging. Regeneration
        /// (bounded `claude` sub-sessions, one per dimension) is ON by default;
        /// pass `--no-regenerate` to skip it.
        #[arg(long = "no-regenerate")]
        no_regenerate: bool,
        /// Also build the individual per-dimension books (R2: by default only the
        /// merged book is built).
        #[arg(long)]
        per_dimension: bool,
        /// Manifest key of the merged book to build.
        #[arg(long, default_value = "governing_the_agentic_machine")]
        merged_key: String,
        /// Bookkit B — the student-notes companion key (built plain, no book chrome).
        #[arg(long, default_value = "student_notes")]
        companion_key: String,
        /// Bookkit C — the master-thesis key (FHNW structure + thesis-only gates).
        #[arg(long, default_value = "master_thesis")]
        thesis_key: String,
        /// Bookkit — the master-thesis-bookkit key (parity + toc-coverage scoped gates).
        #[arg(long, default_value = "master_thesis_bookkit")]
        bookkit_key: String,
        /// Resume: skip the expensive steps (regenerate/merge/build) already
        /// completed for the current content fingerprint (ADR-0047 R3).
        #[arg(long)]
        resume: bool,
        /// Ignore checkpoints and run every step from scratch.
        #[arg(long)]
        force_full: bool,
        /// Print the ordered plan and run only the cheap read-only gates; skip LLM
        /// regeneration, book render and signing.
        #[arg(long)]
        dry_run: bool,
        /// Root directory the DB paths / sub-sessions are relative to.
        #[arg(long, default_value = ".")]
        root: std::path::PathBuf,
        /// Bookkit C (FHNW master-thesis) structural-rule HITL pause.
        ///
        /// When set, the cascade STOPS before the seal step (phase 7) if the
        /// thesis-profile run of `page_boundary` or `bookkit` reports any of:
        /// `PAGE_OVER`, `BOLD_OVERUSE`, `NON_ENGLISH`, `HEADING_DEPTH`. A
        /// clearly-marked `[HITL PAUSE]` block is printed and the cascade
        /// exits non-zero. Default `--thesis-strict=false` preserves the
        /// existing advisory-only behaviour.
        #[arg(long)]
        thesis_strict: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum MergeAction {
    /// Concatenate the eleven `Dimension_NN_*.md` sources (numeric order) into a
    /// single normalised-H1 compendium written to the content store at `--out`.
    Dimensions {
        /// Project ID (ULID).
        #[arg(long)]
        project: String,
        /// Working-tree prefix the dimension sources live under.
        #[arg(long, default_value = agentic_core::paths::SOURCES_PREFIX)]
        prefix: String,
        /// Destination path in the content store for the merged document.
        #[arg(long, default_value = agentic_core::paths::MERGED_DOC)]
        out: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum OrchestrateAction {
    /// Register a new pending sub-session. Prints the stored (timestamp-prefixed) id.
    Add {
        #[arg(long)]
        project: String,
        /// Short id (e.g. "dim01"); stored as "<yyyyMMdd-HHmmss>-<id>".
        #[arg(long)]
        id: String,
        /// The task text piped to the sub-session's stdin.
        #[arg(long)]
        task: String,
        /// Per-session budget in USD.
        #[arg(long, default_value_t = 3.0)]
        budget: f64,
        /// Optional model override (e.g. claude-opus-4-7).
        #[arg(long)]
        model: Option<String>,
    },
    /// List every sub-session for a project (supports the global --json).
    List {
        #[arg(long)]
        project: String,
    },
    /// Execute pending sub-sessions by spawning the `claude` CLI.
    Run {
        #[arg(long)]
        project: String,
        /// Run a single session by its stored id (otherwise all pending).
        #[arg(long)]
        id: Option<String>,
        /// Cap how many sessions are processed this invocation.
        #[arg(long)]
        max: Option<usize>,
        /// Wave mode: keep going through all pending, sleeping + retrying on
        /// rate-limit, until none remain or --max is reached.
        #[arg(long)]
        wave: bool,
        /// Root directory the sub-session may edit (`--add-dir`).
        #[arg(long, default_value = ".")]
        root: String,
    },
    /// Cycle-close: run the in-tool gates + audit in sequence and print a verdict table.
    Close {
        #[arg(long)]
        project: String,
        /// Root directory for the docs gate.
        #[arg(long, default_value = ".")]
        root: std::path::PathBuf,
    },
}

#[derive(Debug, Subcommand)]
pub enum TranslateAction {
    /// Translate a scope of the corpus into a target language. Held by
    /// default; requires `agentic authorize grant --action translate`.
    /// Use `--dry-run` to preview the scope and the provider that would be
    /// used without making any LLM calls or writing any files.
    Scope {
        #[arg(long)]
        project: String,
        /// Target language tag (de, fr, it, rm, hi).
        #[arg(long)]
        target: String,
        /// Scope: `front-matter` (smallest), `thesis`, or `corpus`.
        #[arg(long, default_value = "front-matter")]
        scope: String,
        /// Provider to use; default = first configured cloud provider.
        #[arg(long)]
        provider: Option<String>,
        /// Skip the LLM calls and the writes; preview the plan only.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum ProfileAction {
    /// Create or update a named profile. `--attach` may be repeated; the
    /// profile JSON-body comes from `--settings` (a JSON string) or from
    /// `--settings-file` (a file path). The pair is mutually exclusive.
    Put {
        #[arg(long)]
        project: String,
        /// Profile name (slug-like, e.g. `fhnw-mas-thesis-c`).
        #[arg(long)]
        name: String,
        /// Section slug to attach to; may be repeated.
        #[arg(long = "attach")]
        attach: Vec<String>,
        /// Inline JSON settings bundle.
        #[arg(long, conflicts_with = "settings_file")]
        settings: Option<String>,
        /// Path to a JSON file with the settings bundle.
        #[arg(long)]
        settings_file: Option<PathBuf>,
    },
    /// Read the latest profile by name.
    Get {
        #[arg(long)]
        project: String,
        #[arg(long)]
        name: String,
    },
    /// List every live profile (one row per unique name).
    List {
        #[arg(long)]
        project: String,
    },
    /// Resolve the profile attached to a section slug (latest-id wins when
    /// multiple profiles claim the same section).
    Resolve {
        #[arg(long)]
        project: String,
        #[arg(long)]
        section: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum SynthesizeAction {
    /// Compose a prompt from the runtime's current acceptance set and ask a
    /// configured LLM to propose draft cross-stream findings. Output goes to
    /// `out/sources/synthesis/candidates_<ts>.md`; never promoted to a
    /// canonical deliverable. Use `--dry-run` to preview the prompt without
    /// calling any provider.
    CrossStream {
        #[arg(long)]
        project: String,
        /// Provider to use (default: first configured cloud provider).
        #[arg(long)]
        provider: Option<String>,
        /// Model id (e.g. grok-4.3).
        #[arg(long)]
        model: Option<String>,
        /// Maximum number of accept-tier review findings to include in the
        /// prompt (0 = all).
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Skip the LLM call; print the prompt + a candidate skeleton.
        #[arg(long)]
        dry_run: bool,
        /// Output path; if omitted, writes to
        /// `out/sources/synthesis/candidates_<ts>.md` in the working tree.
        #[arg(long)]
        to: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
pub enum RankAction {
    /// One-screen summary of ADR-0046 acceptance per section. Reads
    /// `claim_audit_results`; classifies each by content path; counts
    /// placements (thesis_main / thesis_appendix / lowrankings / other);
    /// notes the model_review accept/revise/exclude split per section.
    Summary {
        #[arg(long)]
        project: String,
        /// Restrict to one section slug.
        #[arg(long)]
        section: Option<String>,
        /// Output path; if omitted, prints to stdout.
        #[arg(long)]
        to: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
pub enum RiskAction {
    /// Compute the model and emit results as JSON (optionally one --item).
    Compute {
        /// RAMP corpus metadata JSON (see ADR-0040). `-` reads stdin.
        #[arg(long)]
        input: String,
        /// Restrict output to a single item id.
        #[arg(long)]
        item: Option<String>,
    },
    /// Emit a six-page markdown risk-assessment chapter for one item.
    Chapter {
        #[arg(long)]
        input: String,
        #[arg(long)]
        item: String,
    },
    /// Emit an aggregate "graphical illustrations" chapter across all items.
    Graphics {
        #[arg(long)]
        input: String,
    },
    /// Parameterised AI-vs-human investment / replacement estimator with an
    /// auditable low/base/high sensitivity band (ADR-0040). Runs with no args
    /// against a built-in demo set of challenges.
    Invest {
        /// Challenges JSON (`{"challenges":[{name,human_hours,risk}]}` or a
        /// bare array). Risk is one of low|med|high|crit. Omit to use the
        /// built-in demo set.
        #[arg(long)]
        challenges: Option<PathBuf>,
        /// Region key for the fully-loaded human rate (case-insensitive):
        /// CH|US|IL|UK|AU|EU|CA|SG|JP|AE|KR|SA|CN|BR|IN.
        #[arg(long, default_value = "CH")]
        region: String,
        /// Token price, USD per million tokens (overridden by --scenario).
        #[arg(long, default_value_t = 1.67)]
        token_price: f64,
        /// Token-price preset: baseline-2025 (1.67) | asic-2027 (0.20).
        #[arg(long)]
        scenario: Option<String>,
        /// Compute per agent-month, in millions of tokens.
        #[arg(long, default_value_t = 600.0)]
        tokens_per_month: f64,
        /// AI:human productivity factor, 0 < p <= 1.
        #[arg(long, default_value_t = 1.0)]
        parity: f64,
        /// Human-rate premium for the escalation-mode share of work.
        #[arg(long, default_value_t = 1.5)]
        escalation_premium: f64,
        /// Human-rate premium for the disaster-mode share of work.
        #[arg(long, default_value_t = 2.0)]
        disaster_premium: f64,
    },
}

#[derive(Debug, Subcommand)]
pub enum BookAction {
    /// Render books from a manifest, sourcing chapters from the content store.
    /// Manifest: {"books":[{"key","title","subtitle","chapters":[DB paths]}]}.
    Build {
        #[arg(long)]
        project: String,
        #[arg(long)]
        manifest: PathBuf,
        /// Output directory for the .docx files (kept clean: only .docx + report).
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        only: Option<String>,
    },
    /// Audit render quality of a books directory, comparing each book against
    /// the previous iteration (figures, heading styles, page size, size). Fails
    /// on regression.
    Audit {
        /// Current books directory.
        #[arg(long)]
        current: PathBuf,
        /// Previous iteration's books directory to compare against.
        #[arg(long)]
        previous: Option<PathBuf>,
    },
    /// Finalize rendered `.docx` through Microsoft Word (Windows only): update
    /// every field (TOC, List of Figures/Tables, Index), repaginate so page
    /// numbers are real, and save — so the document opens with NO "update
    /// fields" prompt. Ports the gold `book_build/finalize.ps1`. Requires Word;
    /// errors clearly on non-Windows or when Word is absent.
    Finalize {
        /// A `.docx` file, or a directory whose `.docx` files are all finalized.
        #[arg(long)]
        path: PathBuf,
        /// Also export a PDF next to each finalized `.docx`.
        #[arg(long, default_value_t = false)]
        pdf: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum GenAction {
    /// Print the mandatory generation + figure rules only.
    Rules,
    /// Print a rule-prefixed prompt for an artefact kind.
    Prompt {
        /// dimension | campaign | project | condense | generic
        #[arg(long, default_value = "generic")]
        kind: String,
        #[arg(long)]
        topic: String,
        /// Extra task-specific instructions.
        #[arg(long)]
        extra: Option<String>,
    },
    /// Generate the per-tool mission-control agent-defs from the single canonical
    /// body (ADR-0047 R10): writes .claude/agents/ and .factory/droids/ as
    /// front-matter adapter + the verbatim canonical body. The Gemini runtime
    /// uses the AGENTS.md pointer model and needs no generated droid file.
    AgentDefs {
        /// Portfolio root (holds specs/mission-control.canonical.md).
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
pub enum InboxAction {
    /// Register inbox blobs from the content store as queued items (idempotent).
    Register {
        #[arg(long)]
        project: String,
    },
    /// Show every inbox item and its lifecycle state.
    Status {
        #[arg(long)]
        project: String,
    },
    /// Mark an item accepted (acceptance level reached).
    Accept {
        #[arg(long)]
        project: String,
        #[arg(long)]
        path: String,
        #[arg(long)]
        score: Option<f64>,
        /// thesis_main | thesis_appendix | lowrankings
        #[arg(long)]
        placement: Option<String>,
        /// Passport id / ranking-deliverable path / note.
        #[arg(long)]
        justification: Option<String>,
        /// Record the acceptance as human-confirmed (else autonomous).
        #[arg(long)]
        hitl: bool,
    },
    /// Mark an item skipped (non-input, e.g. a folder README).
    Skip {
        #[arg(long)]
        project: String,
        #[arg(long)]
        path: String,
    },
    /// Retire a processed item: delete its on-disk copy (the DB blob remains the
    /// permanent archive) and journal the move. Refuses unless the content is in
    /// the DB and the item is accepted/justified/skipped.
    Retire {
        #[arg(long)]
        project: String,
        #[arg(long)]
        path: String,
        #[arg(long, default_value = ".")]
        root: std::path::PathBuf,
    },
    /// Report duplicates: exact (shared SHA) and, if embeddings exist, semantic
    /// near-duplicates (cosine ≥ threshold).
    Dedup {
        #[arg(long)]
        project: String,
        #[arg(long)]
        model: Option<String>,
        #[arg(long, default_value_t = 0.90)]
        threshold: f32,
    },
    /// Self-driving: rank → justify → accept|hold every queued item, recording
    /// an audit_rows decision per transition and auto-writing the passport
    /// justification. Mainline-eligible items are held for HITL unless
    /// --auto-mainline. Run `agentic embed` first for novelty/near-dup scoring.
    Process {
        #[arg(long)]
        project: String,
        #[arg(long)]
        model: Option<String>,
        /// Novelty score (0..1) at/above which an item is mainline-eligible.
        #[arg(long, default_value_t = 0.50)]
        accept_threshold: f64,
        /// Cosine at/above which two items are treated as near-duplicates.
        #[arg(long, default_value_t = 0.90)]
        near_dup: f32,
        /// Auto-accept mainline-eligible items instead of holding for HITL.
        #[arg(long)]
        auto_mainline: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum AuditAction {
    /// Generate an ML-DSA-87 keypair; secret to the OS keychain, public to DB.
    Keygen {
        /// Human-readable signer identity recorded with the key.
        #[arg(long, default_value = "agentic")]
        signer: String,
    },
    /// Sign all of a project's commits with the active key (non-repudiation).
    SignCommits {
        #[arg(long)]
        project: String,
    },
    /// Verify recorded signatures (commits and/or a report digest).
    Verify {
        #[arg(long)]
        project: String,
    },
    /// Record one AI/LLM decision into the per-item audit index (going-forward).
    Record {
        #[arg(long)]
        project: String,
        #[arg(long)]
        agent: String,
        #[arg(long)]
        action: String,
        #[arg(long)]
        target: Option<String>,
        #[arg(long, default_value = "info", value_parser = ["pass", "warn", "fail", "ok", "info"])]
        result: String,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        tokens: Option<i64>,
        #[arg(long)]
        iteration: Option<i64>,
        /// Free-text detail (stored as the sidecar).
        #[arg(long)]
        detail: Option<String>,
    },
    /// Compile a complete audit report (signed). MD or JSON; whole-project or
    /// a single item via --item.
    Report {
        #[arg(long)]
        project: String,
        /// Restrict to one item (substring of commit/journal/passport content).
        #[arg(long)]
        item: Option<String>,
        #[arg(long, default_value = "md", value_parser = ["md", "json"])]
        format: String,
        /// Output path. If omitted, writes to stdout.
        #[arg(long)]
        to: Option<PathBuf>,
    },
    /// Per-section verdict report (ADR-0023 + ADR-0046 + perception P-2):
    /// joins the latest `audit_verdicts` per gate × the path → section
    /// classifier × the static gate → ADR map, and emits one row per section
    /// with verdict, finding count, and the ADRs the gates enforce. Markdown
    /// by default; `--json` for machine-readable.
    Profile {
        #[arg(long)]
        project: String,
        /// Restrict to a single section (dimensions, campaigns, projects,
        /// student_notes, master_thesis, agentic_handbook, audit, norms,
        /// frontmatter, other).
        #[arg(long)]
        section: Option<String>,
        /// Output path. If omitted, writes to stdout.
        #[arg(long)]
        to: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
pub enum ImportAction {
    /// Import a single file. Markdown is passed through; DOCX/PDF are
    /// extracted to text and stored as markdown.
    File {
        /// Path on disk to import.
        path: PathBuf,
        /// Project ID (ULID) to import into.
        #[arg(long)]
        project: String,
        /// Working-tree path to store the resulting markdown blob at.
        #[arg(long)]
        to: String,
        /// Author recorded on the new commit.
        #[arg(long, default_value = "import")]
        author: String,
        /// Commit message.
        #[arg(long)]
        message: Option<String>,
        /// Language tag (en|de|fr|it|rm|hi).
        #[arg(long)]
        lang: Option<String>,
    },
    /// Recursively import every supported file under a directory.
    Dir {
        /// Source directory.
        path: PathBuf,
        /// Project ID (ULID).
        #[arg(long)]
        project: String,
        /// Prefix to mirror imports under (e.g. "proposal").
        #[arg(long, default_value = "")]
        prefix: String,
        #[arg(long, default_value = "import")]
        author: String,
        #[arg(long)]
        message: Option<String>,
        #[arg(long)]
        lang: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum ProviderAction {
    /// List the seven supported providers and show whether a key is configured.
    List,
    /// Run a minimal live request against a provider (chat for most; embed for Voyage).
    Test {
        /// Provider name: anthropic|openai|google|mistral|cohere|voyage|ollama (aliases accepted).
        name: String,
        /// Override the model for the test.
        #[arg(long)]
        model: Option<String>,
    },
    /// Show how the router would resolve a given task right now.
    Route {
        /// Task name: chat|judge|embed|extract|classify|translate.
        task: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum ConfigAction {
    /// Store an API key for a provider in the OS keychain.
    SetKey {
        /// Provider name (e.g. anthropic, openai, google, mistral, cohere, voyage).
        provider: String,
        /// Key value. Use `-` to read from stdin (newline-trimmed).
        value: String,
    },
    /// Delete an API key from the OS keychain.
    UnsetKey { provider: String },
    /// Print where a provider's key resolves from (env var, keychain, or missing).
    WhereKey { provider: String },
}

#[derive(Debug, Subcommand)]
pub enum CheckAction {
    /// Structural integrity (schema, projects, refs, journal, passport).
    Self_ {
        /// Optional: restrict to a project (currently informational; the DB-level
        /// checks are global).
        #[arg(long)]
        project: Option<String>,
    },
    /// 46 AI-typical patterns + FHNW style rules.
    WritingQuality {
        #[arg(long)]
        project: String,
    },
    /// APA7 in-text vs. literature-corpus cross-check + online-source quota.
    Citations {
        #[arg(long)]
        project: String,
    },
    /// Crossref / OpenAlex / Semantic Scholar contamination signals.
    Contamination {
        #[arg(long)]
        project: String,
        /// Skip network calls (signals reduce to "no DOI → all unmatched").
        #[arg(long)]
        offline: bool,
    },
    /// Boot integrity: do the on-disk files still match the DB? Fails on drift.
    Tree {
        #[arg(long)]
        project: String,
        /// Root directory the DB paths are relative to.
        #[arg(long, default_value = ".")]
        root: std::path::PathBuf,
        /// Restrict to a path prefix (e.g. "specs/").
        #[arg(long, default_value = "")]
        prefix: String,
    },
    /// Deliverable acceptance gate (ADR-0036/0037/0038 + figure-standards) over
    /// content-store markdown. Fails on any ERROR finding.
    Deliverable {
        #[arg(long)]
        project: String,
        /// Restrict to a path prefix (e.g. "out/sources/").
        #[arg(long, default_value = agentic_core::paths::SOURCES_PREFIX)]
        prefix: String,
    },
    /// Mission-control documentation currency gate (CLAUDE.md rule 9): the
    /// governance docs must exist and PROGRESS.md must not lag the journal.
    Docs {
        #[arg(long)]
        project: String,
        /// Root directory holding the mission-control docs.
        #[arg(long, default_value = ".")]
        root: std::path::PathBuf,
    },
    /// Per-dimension bibliography coverage (Phase 1): every dimension must carry
    /// a reference and no web URL in a dimension source may be orphaned.
    Bibliography {
        #[arg(long)]
        project: String,
        /// Restrict the orphan-URL scan to a path prefix.
        #[arg(long, default_value = agentic_core::paths::SOURCES_PREFIX)]
        prefix: String,
    },
    /// AIBOM chronological-ledger integrity (Phase 2): every commit signed, the
    /// journal covers the commit span, and AI decisions are recorded.
    Aibom {
        #[arg(long)]
        project: String,
    },
    /// ADR enforcement coverage (ADR-0052): every ADR in `specs/adr/`
    /// must declare an `enforced_by:` frontmatter list pointing at
    /// tests, gates, policies, or manual-justifications. Each `test:`
    /// entry is cross-checked against the workspace source tree; each
    /// `gate:` entry against the GATE_CATALOG. Missing enforcement is
    /// WARN (initial — escalation to ERROR gated on backfill
    /// completion in a future ADR).
    AdrEnforcement {
        #[arg(long)]
        project: String,
    },
    /// Deprecated-token surveillance: scan the live worktree for tokens
    /// that have been retired (e.g. an old phrasing replaced by a rename
    /// like "12-week AI-First plan" → "100-days AI-First plan"). Reads
    /// the registry from `specs/deprecated-terms.json` by default;
    /// alternatively, `--token` runs a single-token ad-hoc check.
    /// Frozen artefacts (`inbox/`, `thesis-draft-v*/`, `proposal/citations/`,
    /// the audit ledger, CHANGELOG, and the registry itself) are
    /// excluded by design — they intentionally preserve historical text.
    TermRename {
        #[arg(long)]
        project: String,
        /// One-shot mode: scan for a single token without consulting the
        /// registry file. Use `--replacement` to record the intended
        /// replacement in the finding messages.
        #[arg(long)]
        token: Option<String>,
        /// Intended replacement for the one-shot token (reported in
        /// findings; ignored otherwise).
        #[arg(long, requires = "token")]
        replacement: Option<String>,
        /// Optional ISO date the deprecation took effect (e.g.
        /// `2026-05-31`). Reported in findings.
        #[arg(long, requires = "token")]
        since: Option<String>,
    },
    /// Verified-facts integrity (ADR-0036/0042): every anchored fact has a real
    /// source; needs_verification placeholders are surfaced as outstanding HITL.
    FactsIntegrity {
        #[arg(long)]
        project: String,
    },
    /// Language-interoperability coverage: every engine chrome i18n key must
    /// carry an EXPLICIT translation for the target language(s); a missing key
    /// would silently leak English chrome. Regression guard, not a translator.
    ///
    /// The target language comes from the global `--lang` (default `en`):
    /// `--lang de` checks German; with the default `en` (the fallback baseline)
    /// every non-English supported language is checked.
    I18n {
        #[arg(long)]
        project: String,
    },
    /// Bookkit-polishing audit (REPORTING gate, WARN-level, never blocks):
    /// bold is allowed only as a short leading label (RULE 1), and body prose
    /// must be English (RULE 2, conservative deny-list). Reports violations
    /// with `<path>:<line>` locations plus per-rule totals.
    ///
    /// Scope is `--prefix` by default. When `--paths-from-manifest` + `--book-key`
    /// are supplied the gate measures the exact chapter list of that book in
    /// the manifest (fixes ADR-0045 bookkit-C scope: the thesis profile
    /// composes from mixed prefixes, so a single-prefix scan misses chapters).
    Bookkit {
        #[arg(long)]
        project: String,
        /// Restrict the audit to a path prefix. Ignored when
        /// `--paths-from-manifest` + `--book-key` are supplied.
        #[arg(long, default_value = agentic_core::paths::SOURCES_PREFIX)]
        prefix: String,
        /// Optional bookkit manifest path; when supplied with `--book-key`
        /// the gate audits the exact chapter list of that book instead of
        /// every file under `--prefix`.
        #[arg(long, requires = "book_key")]
        paths_from_manifest: Option<std::path::PathBuf>,
        /// Bookkit manifest key (e.g. `master_thesis`) selecting which book's
        /// chapter list to audit. Requires `--paths-from-manifest`.
        #[arg(long, requires = "paths_from_manifest")]
        book_key: Option<String>,
    },
    /// PRISMA claim→source coverage (ADR-0020/0026): every in-text citation key
    /// must map to a literature_corpus reference. Unmapped keys WARN; an INFO
    /// summary reports <mapped>/<total> claims mapped.
    Prisma {
        #[arg(long)]
        project: String,
        /// Restrict the scan to a path prefix.
        #[arg(long, default_value = agentic_core::paths::SOURCES_PREFIX)]
        prefix: String,
    },
    /// Cross-model attestation coverage (ADR-0028): external_stat / measured
    /// facts lacking a `cross_model` attestation WARN (advisory — providers may
    /// be unconfigured).
    CrossModel {
        #[arg(long)]
        project: String,
    },
    /// Model-review verdicts (ADR-0049): surface the `agentic review run`
    /// document/ranking verdicts; an `exclude` recommendation WARNs (advisory —
    /// adoption is decided downstream).
    ModelReview {
        #[arg(long)]
        project: String,
    },
    /// Undefined-term gate (per-user 2026-05-28): flag any acronym used in a
    /// deliverable before it appears in the front-matter Acronyms and
    /// Abbreviations table or is introduced parenthetically on the same line.
    /// Advisory (WARN-only). Stop-list covers universal abbreviations.
    UndefinedTerms {
        #[arg(long)]
        project: String,
        /// Restrict to a path prefix in the working tree.
        #[arg(long, default_value = "thesis/")]
        prefix: String,
    },
    /// Future-year temporal-contamination gate: any standalone 20xx year greater
    /// than --max-year in deliverable markdown or corpus years WARNs.
    Temporal {
        #[arg(long)]
        project: String,
        /// Years strictly greater than this are flagged.
        #[arg(long, default_value_t = 2026)]
        max_year: u32,
    },
    /// Concrete ground-truth anchor gate: measured / build_artifact facts whose
    /// source has no path/commit/URL/RAMP/index anchor WARN.
    GroundTruth {
        #[arg(long)]
        project: String,
    },
    /// Contamination-report consolidation: reads the newest contamination_status
    /// report; fabricated>0 ERRORs, suspect>0 WARNs, missing report WARNs.
    Compliance {
        #[arg(long)]
        project: String,
    },
    /// Sprint-contract status gate: counts the project's sprint_contracts; a
    /// 'dissent' phase WARNs; no contracts is an INFO PASS.
    Sprint {
        #[arg(long)]
        project: String,
    },
    /// Predatory-venue heuristic: literature_corpus venues/publishers matching a
    /// small conservative deny-list WARN (review prompt, never a block).
    Predatory {
        #[arg(long)]
        project: String,
    },
    /// Reproducibility pinning (ADR-0039): passport corpus/fact entries not bound
    /// to a signed commit WARN; PASS when all are pinned + signed.
    Reproducibility {
        #[arg(long)]
        project: String,
    },
    /// Three-tier body-length gate (ADR-0035): estimates pages from word count
    /// under `--prefix`; over `--max-pages` WARNs, else INFO.
    ///
    /// `--words-per-page` calibrates the heuristic (default 500, ~1 manuscript
    /// page). For rendered FHNW Word output the empirical density is ~280
    /// words/page (TOC, LoF, LoT, declarations, bibliography lines).
    ///
    /// When `--paths-from-manifest` + `--book-key` are supplied the gate
    /// counts only the chapters listed for that book in the manifest, so the
    /// estimate matches what the rendered .docx actually contains.
    PageBoundary {
        #[arg(long)]
        project: String,
        /// Restrict the word-count to a path prefix. Defaults to the FHNW
        /// master-thesis body (ADR-0045); the superseded dimensional draft lives
        /// under `thesis-draft-v5/` and is not the gated body. Ignored when
        /// `--paths-from-manifest` + `--book-key` are supplied.
        #[arg(long, default_value = "thesis/")]
        prefix: String,
        /// Maximum allowed estimated pages.
        #[arg(long, default_value_t = 60)]
        max_pages: usize,
        /// Words-per-page heuristic. Default 500 (raw manuscript). FHNW Word
        /// thesis density is empirically ~280 — passing 280 here matches the
        /// rendered .docx page count.
        #[arg(long, default_value_t = 500)]
        words_per_page: usize,
        /// Optional bookkit manifest path; when supplied with `--book-key`
        /// the gate counts the exact chapter list of that book instead of
        /// every file under `--prefix`.
        #[arg(long, requires = "book_key")]
        paths_from_manifest: Option<std::path::PathBuf>,
        /// Bookkit manifest key (e.g. `master_thesis`) selecting which book's
        /// chapter list to count. Requires `--paths-from-manifest`.
        #[arg(long, requires = "paths_from_manifest")]
        book_key: Option<String>,
    },
    /// Verify a rendered FHNW master-thesis docx against 11 structural
    /// predicates derived from the proposal docx (ADR-0050 v0.1.15-engine):
    /// header logo + 2 text lines, header propagation, body Arial coverage,
    /// no Designer-font leak, no XE/MERGEFORMAT field leaks, justify
    /// alignment, Caption-style coverage, chapter-heading style.
    ///
    /// Windows-only (requires Microsoft Word COM). Without `--rendered-docx`
    /// the gate is no-op (INFO PASS).
    RenderFidelity {
        #[arg(long)]
        project: String,
        /// Path to the rendered docx to inspect. Without this flag the gate
        /// is opt-in and skipped.
        #[arg(long)]
        rendered_docx: Option<std::path::PathBuf>,
        /// Optional path to the FHNW proposal docx (reserved for future
        /// structural-diff mode; today the gate evaluates self-contained
        /// predicates that were extracted FROM the proposal).
        #[arg(long)]
        proposal_docx: Option<std::path::PathBuf>,
    },
    /// ARS 7-mode integrity scan (ADR-0044): hallucinated results, method
    /// fabrication, shortcuts, impl-bug admissions, frame-lock, unverified
    /// results, overclaims — over deliverable markdown.
    Integrity {
        #[arg(long)]
        project: String,
    },
    /// Deterministic figure-hygiene gate (ADR-0044): empty alt text, missing
    /// captions, orphan "Figure N" references (ERROR), unreferenced figures.
    FigureQuality {
        #[arg(long)]
        project: String,
    },
    /// AI-disclosure presence + venue match (ADR-0044): a named venue/track
    /// without a disclosure statement is an ERROR.
    Disclosure {
        #[arg(long)]
        project: String,
        /// Optional explicit venue/track to require a disclosure for.
        #[arg(long)]
        venue: Option<String>,
    },
    /// R&R traceability-matrix column validation (ADR-0044): a matrix (a table
    /// with a "Verified?" column) must also carry reviewer/claim/location columns.
    RrMatrix {
        #[arg(long)]
        project: String,
    },
    /// Reviewer FNR/FPR calibration (ADR-0044) vs `out/calibration_gold.json`;
    /// FAILs if FNR ≥ 0.15 or FPR ≥ 0.10. INFO when no gold set is present.
    Calibration {
        #[arg(long)]
        project: String,
    },
    /// Domain-freshness gate (ADR-0047): flags recency markers ("last verified
    /// <Mon YYYY>", "as of <YYYY>", …) older than --max-age-months. Advisory.
    Freshness {
        #[arg(long)]
        project: String,
        #[arg(long, default_value_t = 12)]
        max_age_months: u32,
    },
    /// Book-manifest ⇄ rendered-TOC coverage (TODO-16): for every book in
    /// `out/book_manifest.json`, compare the declared chapter list against
    /// the TOC paragraphs Word actually emitted into
    /// `<snapshot_dir>/<book_key>.docx`. Missing entries WARN, extras WARN,
    /// level mismatches are INFO. Books without a rendered docx emit
    /// `TOC_NO_SNAPSHOT` INFO and are skipped (the gate never FAILs on
    /// missing renders).
    TocCoverage {
        #[arg(long)]
        project: String,
        /// Directory containing the rendered `<book_key>.docx` files
        /// (typically `snapshots/<ts>-books-cascade/`).
        #[arg(long, default_value = "snapshots/latest")]
        snapshot_dir: std::path::PathBuf,
    },
    /// Visual / structural parity gate (ADR-0057): compare a rendered book
    /// against a frozen reference docx along four scopes — figures (`<w:drawing>`
    /// count), captioned tables (`<w:tbl>` with "Table N." caption + row-1
    /// `<w:tblHeader/>`), style usage (16 named styles within ±10 %), and
    /// layout (sectPr / header/footer parts / back-matter order). The gate
    /// is auditable: the verdict lands in `audit_verdicts` under the
    /// `parity` checkpoint.
    Parity {
        #[arg(long)]
        project: String,
        /// Project-internal book id (informational; the inputs come from
        /// `--reference` and `--book`).
        #[arg(long)]
        book: String,
        /// The frozen reference docx the current rendering is compared against.
        #[arg(long)]
        reference: std::path::PathBuf,
        /// The current docx to grade. If absent, the gate reads
        /// `snapshots/latest/<book>.docx`.
        #[arg(long)]
        current: Option<std::path::PathBuf>,
        /// Optional path to write a self-contained HTML report alongside
        /// the CLI output (parity diff page).
        #[arg(long)]
        html_report: Option<std::path::PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
pub enum FactsAction {
    /// Anchor a recurring claim to one provenance-bearing verified record
    /// (ADR-0016/0042). A real `--source` is required (ADR-0036) unless the
    /// kind is `needs_verification` (an unresolved HITL placeholder).
    Add {
        #[arg(long)]
        project: String,
        /// The claim text/pattern as it appears in prose (e.g. "over 1,000 packages").
        claim: String,
        /// measured | model_estimate | build_artifact | external_stat | needs_verification
        #[arg(long, default_value = "measured")]
        kind: String,
        /// The provenance (DOI/URL/manifest SHA/RAMP run/HITL sign-off).
        #[arg(long, default_value = "")]
        source: String,
        /// Optional canonical value.
        #[arg(long)]
        value: Option<String>,
    },
    /// List the project's verified facts (optionally only unresolved ones).
    List {
        #[arg(long)]
        project: String,
        /// Show only `needs_verification` placeholders (the HITL queue).
        #[arg(long)]
        needs_verification: bool,
    },
    /// HITL sign-off (ADR-0017): resolve a `needs_verification` fact into a
    /// sourced verified fact, superseding the placeholder.
    Verify {
        #[arg(long)]
        project: String,
        /// The verified-fact id to resolve.
        id: i64,
        /// Human confirming the fact.
        #[arg(long)]
        by: String,
        /// Evidence / how it was confirmed.
        #[arg(long)]
        evidence: String,
    },
    /// Machine-verification (ADR-0028/0036): resolve a `needs_verification` fact
    /// against a real, independently-confirmable public source (DOI/URL/standard
    /// clause). Records the source verbatim with `verified_method: machine` —
    /// NOT a human HITL sign-off — so the audit trail names what verified it.
    Resolve {
        #[arg(long)]
        project: String,
        /// The verified-fact id to resolve.
        id: i64,
        /// The confirmed source (DOI / URL / standard clause / CVE id).
        #[arg(long)]
        source: String,
        /// What kind of source it is once resolved.
        #[arg(long, default_value = "external_stat")]
        kind: String,
        /// Optional canonical value the source confirms.
        #[arg(long)]
        value: Option<String>,
        /// How it was confirmed (e.g. "Crossref api.crossref.org/works/<doi>").
        #[arg(long, default_value = "")]
        method: String,
    },
    /// Scan deliverables for `NEEDS-VERIFICATION:` markers and enqueue each as a
    /// `needs_verification` fact (the HITL queue) if not already present.
    Scan {
        #[arg(long)]
        project: String,
        #[arg(long, default_value = agentic_core::paths::SOURCES_PREFIX)]
        prefix: String,
    },
    /// Collapse duplicate `needs_verification` placeholders that share the same
    /// claim text (e.g. a marker present in both a per-dimension file and the
    /// merged document). Keeps the lowest-id copy and supersedes the rest, so
    /// the HITL queue reflects distinct claims, not text duplication.
    Dedupe {
        #[arg(long)]
        project: String,
        /// Report what would be collapsed without writing supersessions.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum PrismaAction {
    /// Build the claim→source coverage map over the deliverables.
    Build {
        #[arg(long)]
        project: String,
        #[arg(long, default_value = agentic_core::paths::SOURCES_PREFIX)]
        prefix: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum VerifyAction {
    /// Cross-model attestation of a verified fact by a second provider.
    CrossModel {
        #[arg(long)]
        project: String,
        /// The verified-fact id to attest.
        #[arg(long)]
        fact: i64,
        /// Provider to use (default: first configured cloud provider).
        #[arg(long)]
        provider: Option<String>,
        /// Model id for the chosen provider.
        #[arg(long)]
        model: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum ReviewAction {
    /// Sequentially review every deliverable document under `--prefix` (and the
    /// current rankings) with a second model, recording a per-document verdict
    /// `{assessment: accept|revise|exclude, score, issues, ranking_feedback}`
    /// into `claim_audit_results` (kind=model_review). Feeds cascade adoption.
    Run {
        #[arg(long)]
        project: String,
        /// Content prefix to review (default: all deliverable sources).
        #[arg(long, default_value = agentic_core::paths::SOURCES_PREFIX)]
        prefix: String,
        /// Provider (default: first configured cloud provider, e.g. grok).
        #[arg(long)]
        provider: Option<String>,
        /// Model id (default for grok: grok-4.3).
        #[arg(long)]
        model: Option<String>,
        /// Review at most N documents (0 = all). Useful for a scoped pass.
        #[arg(long, default_value_t = 0)]
        limit: usize,
        /// Re-review every document, even when the content blob hasn't changed
        /// since the last review (default: skip unchanged for an input-delta pass).
        #[arg(long, default_value_t = false)]
        force: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum BibAction {
    /// Scan content for web URLs (+ add a per-dimension user-input trace) and
    /// append them to the literature_corpus passport, bound to HEAD.
    Harvest {
        #[arg(long)]
        project: String,
        /// Content path prefix to scan (dimension sources + emailresearch).
        #[arg(long, default_value = "")]
        prefix: String,
    },
    /// Emit the APA7 reference list. Default: **curated alphabetical**
    /// (one unified list sorted by first author, deduplicated across
    /// dimensions, placeholder entries filtered). Pass `--per-dimension`
    /// for the legacy grouped output (one `## Dimension N — References`
    /// section per dimension, bullets). Use `--dimension N` to restrict
    /// to one dimension's references (implies `--per-dimension`).
    /// Pass `--write` to ingest the output into the project worktree
    /// at `out/sources/Dimensions_bibliography_EN.md` (the path the
    /// thesis renderer reads the References chapter from). When
    /// `--write` is set, stdout is silent so the cascade subprocess
    /// can call this without capturing.
    Emit {
        #[arg(long)]
        project: String,
        #[arg(long)]
        dimension: Option<i64>,
        /// Force the legacy per-dimension grouped output even when no
        /// `--dimension` is given. Default off — the alphabetical view
        /// is what the FHNW MAS References chapter requires.
        #[arg(long)]
        per_dimension: bool,
        /// Write the emitted markdown into the project worktree at
        /// `out/sources/Dimensions_bibliography_EN.md` (the path the
        /// thesis renderer reads). Suppresses stdout output.
        #[arg(long)]
        write: bool,
    },
}

#[derive(Debug, clap::Args)]
pub struct InitArgs {
    /// Operating mode: 'single' (one project) or 'portfolio' (N sub-projects).
    #[arg(long, default_value = "single")]
    pub mode: String,
    /// Institution profile (e.g., 'fhnw-mas').
    #[arg(long)]
    pub institution: Option<String>,
    /// Track within the institution (e.g., 'lincyber', 'dlinit').
    #[arg(long)]
    pub track: Option<String>,
    /// Working language.
    #[arg(long, default_value = "en")]
    pub working_lang: String,
    /// Skip the wizard, just create the DB.
    #[arg(long)]
    pub no_wizard: bool,
    /// Resume a wizard previously interrupted.
    #[arg(long)]
    pub resume: bool,
}

impl agentic_tui::wizard::WizardArgs for InitArgs {
    fn mode(&self) -> &str {
        &self.mode
    }
    fn working_lang(&self) -> &str {
        &self.working_lang
    }
    fn institution(&self) -> Option<&str> {
        self.institution.as_deref()
    }
}

#[derive(Debug, Subcommand)]
pub enum ProjectAction {
    /// Create a new project.
    New {
        name: String,
        /// 'thesis' | 'sub_paper' | 'standalone' | 'portfolio_root'.
        #[arg(long, default_value = "standalone")]
        kind: String,
        #[arg(long, default_value = "en")]
        working_lang: String,
        #[arg(long)]
        parent: Option<String>,
    },
    /// List all projects.
    List,
    /// Show status for a project (or the current one).
    Status {
        #[arg(long)]
        id: Option<String>,
    },
    /// Archive a project.
    Archive { id: String },
}

#[derive(Debug, Subcommand)]
pub enum PassportAction {
    /// Append a payload to a section.
    Append {
        #[arg(long)]
        project: String,
        section: String,
        /// JSON payload, or `-` to read from stdin.
        payload: String,
        #[arg(long)]
        replaces: Option<i64>,
    },
    /// Read current entries for a section.
    Read {
        #[arg(long)]
        project: String,
        section: String,
    },
    /// Validate passport invariants.
    Validate {
        #[arg(long)]
        project: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum JournalAction {
    /// Append a new entry.
    Append {
        #[arg(long)]
        project: String,
        #[arg(long)]
        actor: String,
        #[arg(long)]
        action_type: String,
        #[arg(long)]
        description: String,
        #[arg(long)]
        reasoning: Option<String>,
        #[arg(long)]
        hallucination_risk: Option<String>,
    },
    /// Show the last N entries.
    Show {
        #[arg(long)]
        project: String,
        #[arg(long, default_value = "10")]
        last: usize,
    },
}

#[derive(Debug, Subcommand)]
pub enum AuthorizeAction {
    /// Grant an audited authorisation for an irreversible action (ADR-0047 R7).
    Grant {
        #[arg(long)]
        project: String,
        /// push_main | tag | publish | translate | supersede | content_delete
        #[arg(long)]
        action: String,
        /// Path/target the grant covers; '*' authorises any scope.
        #[arg(long, default_value = "*")]
        scope: String,
        #[arg(long)]
        rationale: String,
        /// The granting governance actor (Mission-Control / SDD Cycle).
        #[arg(long, default_value = "sdd-cycle")]
        by: String,
    },
    /// List the project's authorisation records.
    List {
        #[arg(long)]
        project: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum ContentAction {
    /// Put a file into the content store; print its SHA.
    Put {
        path: PathBuf,
        #[arg(long)]
        lang: Option<String>,
    },
    /// Supersede (tombstone) a path: retire it from the live working tree without
    /// deleting its history (ADR-0047 R5). Irreversible — requires an authorisation
    /// (`agentic authorize grant --action supersede --scope <path>`).
    Supersede {
        /// The working-tree path to retire (e.g. "out/sources/old_draft.md").
        path: String,
        #[arg(long)]
        project: String,
        #[arg(long)]
        reason: String,
    },
    /// Stage a file at a path in a project's working tree (creates a commit).
    PutAt {
        /// The path inside the project (e.g. "thesis-draft/ch-01.md").
        path: String,
        /// Source file on disk to read content from. Use `-` for stdin.
        #[arg(long)]
        from: String,
        #[arg(long)]
        project: String,
        #[arg(long)]
        lang: Option<String>,
        #[arg(long, default_value = "agentic")]
        author: String,
        #[arg(long)]
        message: Option<String>,
    },
    /// Read the blob at `path` in a project's working tree.
    ReadAt {
        path: String,
        #[arg(long)]
        project: String,
    },
    /// List paths in a project's working tree (optional prefix filter).
    Ls {
        #[arg(long)]
        project: String,
        #[arg(long, default_value = "")]
        prefix: String,
    },
    /// Get a blob by SHA to stdout (or to a path with --to).
    Get {
        sha: String,
        #[arg(long)]
        to: Option<PathBuf>,
    },
    /// Log recent commits.
    Log {
        #[arg(long, default_value = "20")]
        limit: usize,
    },
    /// Bulk-stage many files into a project's working tree in a SINGLE commit.
    /// Paths are stored relative to --root (forward-slash normalised). With
    /// --from-list, only the listed paths are staged (use `-` for stdin);
    /// otherwise --root is walked recursively (dot-dirs and target/ skipped).
    Ingest {
        #[arg(long)]
        project: String,
        /// Root directory the staged paths are taken relative to.
        #[arg(long)]
        root: PathBuf,
        /// File of newline-separated relative paths to stage; `-` reads stdin.
        #[arg(long)]
        from_list: Option<String>,
        /// Make HEAD's tree EXACTLY the staged set (clean mirror), instead of
        /// merging onto the existing tree. History is still preserved.
        #[arg(long)]
        replace: bool,
        #[arg(long, default_value = "import")]
        author: String,
        #[arg(long)]
        message: Option<String>,
    },
    /// Write a project's entire working tree (filtered by --prefix) to disk.
    /// This is the inverse of `ingest`: it reproduces the source tree from the
    /// database so the store can serve as the authoritative source of truth.
    Checkout {
        #[arg(long)]
        project: String,
        /// Destination directory (created if absent).
        #[arg(long)]
        to: PathBuf,
        /// Restrict to a path prefix in the working tree.
        #[arg(long, default_value = "")]
        prefix: String,
        /// Opt-out for the ADR-0048 hardening. Materialising `out/`-prefixed
        /// content to a non-temp `--to` is REFUSED by default (the working-tree
        /// `out/` is deprecated; the DB is source of truth and the cascade
        /// already materialises to `std::env::temp_dir()`). Pass this flag to
        /// allow it — e.g. for a deliberate full-tree restore. Checkouts under
        /// `std::env::temp_dir()` or with no `out/` paths in scope are always
        /// allowed.
        #[arg(long)]
        allow_deprecated_out: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum AcronymsAction {
    /// Walk the rendered DOCX, re-extract per-acronym page lists, and
    /// rewrite the Pages column of `out/sources/frontmatter/acronyms.md`
    /// in the project's working tree.
    Refresh {
        #[arg(long)]
        project: String,
        /// Path to the rendered `master_thesis.docx` (typically the
        /// latest `snapshots/<ts>-books-cascade/master_thesis.docx`).
        #[arg(long)]
        docx: PathBuf,
        /// Print the page lists but do not commit the updated blob.
        #[arg(long)]
        dry_run: bool,
        /// Detect ALL-CAPS acronym candidates in the rendered DOCX that
        /// are missing from the table and add them with the placeholder
        /// expansion `TODO: define`. The author keeps editorial control
        /// over the canonical definition.
        #[arg(long)]
        add_missing: bool,
        /// Drop rows whose acronym has zero page-occurrences in the
        /// rendered DOCX (stale entries from earlier drafts). Applied
        /// BEFORE `--add-missing` so a stale row re-introduced by the
        /// detector is kept (with the placeholder expansion).
        #[arg(long)]
        drop_zero_page: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum ExternalSessionAction {
    /// Ingest an exported AI-platform session (grok / gemini / chatgpt /
    /// claude / perplexity / other) into the AIBOM. The raw export file
    /// is stored as a content-addressed blob; per-platform parsers
    /// extract metadata (session_id, started_at, ended_at, model_hint,
    /// turn_count). Stub parsers store the raw blob only with
    /// turn_count=0. See ADR-0053 §5.2.
    Import {
        #[arg(long)]
        project: String,
        /// Platform identifier — one of: grok, gemini, chatgpt, claude,
        /// perplexity, other.
        #[arg(long)]
        platform: String,
        /// Path to the exported session file (HTML, JSON, MD, or ZIP per
        /// the platform's export format).
        #[arg(long)]
        file: PathBuf,
        /// Author's one-line provenance statement; e.g. "exported
        /// 2026-05-30 14:22 UTC from grok.com share-link
        /// https://grok.com/share/abc123 to ~/Downloads/grok-share.json".
        #[arg(long)]
        attestation: String,
        /// Optional free-form notes (privacy summary, which thesis
        /// paragraph the session informed, etc.).
        #[arg(long)]
        notes: Option<String>,
    },
    /// List captured sessions for a project.
    List {
        #[arg(long)]
        project: String,
        /// Optional: restrict to one platform.
        #[arg(long)]
        platform: Option<String>,
    },
}
