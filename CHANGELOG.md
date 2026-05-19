# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### P9 — release packaging
- Release workflow hardened: `verify-version` job rejects tags that don't match `Cargo.toml` workspace version; per-target binary smoke test (`agentic --version` + `agentic doctor --json`) before packaging
- crates.io publish list extended from 3 → 8 crates, ordered bottom-up (tier 0: `agentic-core`, `agentic-resources`; tier 1: `agentic-providers`, `agentic-tui`; tier 2: `agentic-checks`, `agentic-import`, `agentic-export`; tier 3: `agentic`)
- Archive contents now include `CHANGELOG.md` alongside `README.md` + `LICENSE`

## [0.1.0] — pending tag

### P8 — turnkey legacy-repo migration
- `agentic migrate <src>` creates a Thesis project and ingests an entire FACTORYAI / interim-presentation directory in one shot
- Mapping: `thesis-draft/`, `specs/`, `docs/`, `iterations/`, `code/` mirrored under stable working-tree prefixes; root files land under `proposal/`; 14 hidden / build / vendor directories skipped
- DOCX/PDF target extensions auto-rewritten to `.md` (stored blob is always markdown)
- `MigrationReport` with per-bucket counts; journal entry #1 records the migration

### P6 — embeddings + classify-folder
- Migration 0003: `embeddings(blob_sha, model, chunk_idx, chunk_text, dims, vector)` — little-endian f32 vectors, UNIQUE on `(blob_sha, model, chunk_idx)`
- `agentic_core::embeddings` DAO: `put_embedding` (upsert), `get_embedding`, `list_by_model`, `cosine()` helper (NaN-safe)
- `agentic_import::embed::embed_project_blobs` — async, embeds every markdown blob via registry + router; skips already-embedded pairs; gracefully skips providers without embed API
- `agentic_import::classify::classify_project` — cosine-ranks the 6 default thesis slots (intro / related_work / methodology / results / discussion / conclusion) or a custom CSV
- New CLI: `agentic embed <project>` and `agentic classify <project>`

### P5 — proposal import (markdown / DOCX / PDF)
- `agentic-import::detect` / `markdown` / `import` / `walk` modules
- Single-file and recursive directory import; non-markdown formats extracted to plain text and wrapped with an H1 from the file stem
- New CLI: `agentic import file <path>` and `agentic import dir <path>`

### P4 — DOCX + Typst PDF export + xAI Grok as 8th provider
- `agentic-export`: `collect` (HEAD-tree walker), `markdown` (md→Typst + md→DocxBlock), `docx` (docx-rs renderer with title page, page breaks, numbered/bullet lists), `pdf` (in-memory `typst::World` + typst-kit embedded fonts)
- New CLI: `agentic export <project> --format docx|pdf`
- Eighth provider: xAI Grok (`api.x.ai/v1/chat/completions`, OpenAI-compatible). `ProviderKind` 7 → 8; `CliContext::GrokBuild` detected via `GROK_BUILD` / `XAI_BUILD`

### P3 — ratatui onboarding wizard
- Migration 0002: `wizard_drafts` for resumable state; provider keys never persisted there
- `agentic_tui::wizard` (state + ratatui app); 8 steps; auto-saves draft on every keypress
- `agentic init` launches the wizard by default; `--no-wizard` keeps the minimal scaffold; `--resume` continues from a saved draft

### P2 — seven LLM provider clients + routing
- Concrete `Provider` impls: Anthropic, OpenAI, Google (Gemini), Mistral, Cohere v2, Voyage (embed-only), Ollama (no-auth local)
- `registry::build(ProviderKind) → Arc<dyn Provider>` via OS keychain with `AGENTIC_<PROVIDER>_KEY` env-var fallback
- `router::route(Task)` priority chain: CLI-context → per-task env → default env → per-task fallback (Voyage for Embed, Anthropic otherwise); `supports_task()` filters incompatible pairs
- New CLI: `agentic provider list|test|route`, `agentic config set-key|unset-key|where-key`

### P1 — integrity checkers + worktree
- Four checks: `self` (DB structural integrity), `writing-quality` (46 AI-typical patterns + FHNW rules), `citations` (APA7 in-text vs corpus), `contamination` (Crossref / OpenAlex / S2 signals)
- `agentic-core::worktree`: per-project HEAD ref + path→blob mapping; `put_at`, `read_at`, `list`, `head_tree`, `head_commit`
- New CLI: `agentic check {self,writing-quality,citations,contamination}`

### P0 — workspace scaffolding
- Cargo workspace with eight crates: `agentic`, `agentic-core`, `agentic-providers`, `agentic-checks`, `agentic-import`, `agentic-export`, `agentic-tui`, `agentic-resources`
- Migration 0001: blobs / trees / commits / refs / projects / passport_entries / journal_entries / audit_rows / audit_verdicts / schemas / protocols / adrs / i18n_strings / api_cache / sprint_contracts / fts
- `agentic` binary with `init` / `project` / `journal` / `passport` / `content` / `doctor` subcommands
- CI: rustfmt / clippy / test (ubuntu / macos / windows) / cargo audit
- Release workflow: cross-platform binaries + GitHub Releases + crates.io publish
