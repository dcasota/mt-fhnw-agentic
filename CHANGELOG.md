# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.1] — 2026-05-19

### Fixed

- **`agentic import dir` now surfaces per-file failures** instead of silently
  logging them as `tracing::warn`. The returned `Vec<ImportOutcome>` now
  contains one entry **per file attempted**, with `success: bool` and
  `error: Option<String>` fields. The CLI report prints `+`/`-` lines for
  successes/failures, ends with a summary line ("N imported, M failed"),
  and exits non-zero when any file failed.
- **`bootstrap/init.ps1` now resolves the project ID via `--json`** instead
  of parsing the human-readable text table. The old parser grabbed the
  header row (`ID  KIND  LANG  NAME`), split on whitespace, and wrote the
  literal string `"ID"` to `.project-id` — every subsequent `--project ID`
  call then failed with "project not found: ID". The new script does
  `agentic --json project list | ConvertFrom-Json` and pulls `.id` off the
  first object. Added an `-Force` flag to delete an existing `thesis.db`
  + sidecars + `.project-id` before re-init.

### Changed

- **`aarch64-unknown-linux-gnu` release build switched from `cross` Docker
  container to a native `ubuntu-24.04-arm` GitHub-hosted runner.** The
  cross-build was failing in the v0.1.0 release run (typst-kit /
  rusqlite-bundled in the cross image). The native runner is free,
  faster, and the binary-smoke-test step now also runs on this target.
- `ImportOutcome` struct grew two fields: `success: bool` (defaults to
  `true` for backwards compatibility on serde-deserialised values) and
  `error: Option<String>` (defaults to `None`). Downstream serde
  consumers reading the v0.1.0 JSON shape continue to work.

## [Unreleased]

### Changed (workflow only — applies to next tag push)

- **Release workflow tolerates partial build-matrix failures.** The
  `release` job now runs `if: always() && needs.build.result != 'cancelled'
  && needs.build.result != 'skipped'` — so a single matrix entry's failure
  no longer blocks the publish step. The 4 archives that did build still
  reach the GitHub Release. If **every** matrix entry fails (no archives),
  the checksum step hard-fails and no release is created — empty releases
  are still refused. `publish-crate` runs only when `release.result ==
  success` (unchanged behaviour relative to v0.1.0/0.1.1).
- **`x86_64-apple-darwin` (Intel macOS) dropped from the release matrix.**
  GitHub-hosted `macos-13` runners had a multi-hour-to-multi-day queue
  backlog that blocked v0.1.1 across two retags. Intel macOS is a
  shrinking share of macOS users (Apple stopped shipping Intel Macs in
  2023; `aarch64-apple-darwin` covers the modern fleet). The matrix
  entry can be re-added once the `macos-13` backlog clears. v0.1.1 ships
  4 archives instead of 5: linux x86_64, linux arm64, macOS arm64,
  Windows x86_64.

### P9 — release packaging
- Release workflow hardened: `verify-version` job rejects tags that don't match `Cargo.toml` workspace version; per-target binary smoke test (`agentic --version` + `agentic doctor --json`) before packaging
- crates.io publish list extended from 3 → 8 crates, ordered bottom-up (tier 0: `agentic-core`, `agentic-resources`; tier 1: `agentic-providers`, `agentic-tui`; tier 2: `agentic-checks`, `agentic-import`, `agentic-export`; tier 3: `agentic`)
- Archive contents now include `CHANGELOG.md` alongside `README.md` + `LICENSE`
- **Multi-hash integrity**: every GitHub Release now ships three checksum manifests — `SHA256SUMS` (coreutils), `SHA512SUMS` (coreutils), `B3SUMS` (BLAKE3 via `b3sum` ^1). Lets downstream verifiers pick whichever algorithm fits their toolchain

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
