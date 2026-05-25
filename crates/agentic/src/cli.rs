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

    /// Deterministically normalise content-store markdown (expand prediction×mode
    /// codes, shorten over-long figure captions, apply verified-facts
    /// corrections). Writes changed blobs back in a single commit.
    Normalize {
        #[arg(long)]
        project: String,
        #[arg(long, default_value = "out/sources/")]
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
        #[arg(long, default_value = "out/sources/")]
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
        #[arg(long, default_value = "out/sources/")]
        prefix: String,
    },
    /// AIBOM chronological-ledger integrity (Phase 2): every commit signed, the
    /// journal covers the commit span, and AI decisions are recorded.
    Aibom {
        #[arg(long)]
        project: String,
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
    /// List the project's verified facts.
    List {
        #[arg(long)]
        project: String,
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
    /// Emit the per-dimension APA7 reference list (all dimensions or one).
    Emit {
        #[arg(long)]
        project: String,
        #[arg(long)]
        dimension: Option<i64>,
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
pub enum ContentAction {
    /// Put a file into the content store; print its SHA.
    Put {
        path: PathBuf,
        #[arg(long)]
        lang: Option<String>,
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
    },
}
