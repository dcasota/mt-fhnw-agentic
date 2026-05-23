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
