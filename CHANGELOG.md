# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.7] — 2026-05-28

### Fixed — `check model-review` per-path dedupe (display)

- The gate iterated `passport::current` directly, which can keep two live
  entries for one path when an orphan legacy verdict (nothing's `replaces`
  pointed at it) coexists with a fresh chain that ends in a newer override.
  The display then double-listed the path and the SUMMARY counted entries,
  not unique paths. The gate now mirrors `agentic_core::review::excluded_paths`
  latest-wins-per-path logic: one row per path, SUMMARY counts unique paths,
  and the per-path emit order is sorted for deterministic output. The
  rankings-scope review is unchanged (path-less; reported as before).
  Regression test `orphan_legacy_verdict_is_deduped_by_latest_wins` locks it.

## [0.1.6] — 2026-05-27

### Fixed — gate precision (cascade triage, reduce-only)

- `deliverable` NUMBER_UNSOURCED carries a source across a multi-line
  parenthetical and skips quoted titles; `freshness` requires a month for bare
  `updated`/`verified` and honours `early/mid/late <year>`; `integrity` gained
  `has_genuine_shortcut`/`has_genuine_impl_bug` (skip decomposed/tamper-evidence
  "broken", quoted/dismissed terms, noun stub/placeholder), skips the derived
  merged doc, and skips list/heading scaffolding in frame-lock; `figure_quality`
  treats a descriptive alt as a label; `temporal` skips forecast-framed /
  horizon-table / regulatory-deadline-pair future years and refines the
  comparator check; `ground_truth` recognises measurement scripts and the RAMP
  estimator as concrete anchors.

### Fixed — DOCX / bookkit fidelity (gold `book_build` parity)

- **Word refreshes fields on open**: inject `<w:updateFields>` and add `\h` to
  the main TOC (clickable entries) + `dirty` on TOC/list fields.
- **Table of Tables is complete**: every table is numbered (was numbered only
  when captioned).
- **Pagination**: table rows set `w:cantSplit`; the "Table N." caption keeps
  with its table; a figure keeps with its caption; multi-page table headers
  repeat (`w:tblHeader`, injected post-pack since docx-rs 0.4 has no API).
- **True superscript** for source-ref `[n]` via `RunProperty::vert_align`.

## [0.1.5] — 2026-05-27

### Fixed — Linux release binaries

- **Release workflow now builds the Linux targets.** v0.1.4's `x86_64-` and
  `aarch64-unknown-linux-gnu` jobs failed because `typst`
  (`yeslogic-fontconfig-sys`) links the system fontconfig at build time and the
  GitHub-hosted Ubuntu images don't ship `libfontconfig-dev`. The build and
  `publish-crate` jobs now `apt-get install libfontconfig-dev pkg-config` on
  Linux. Windows/macOS were unaffected (their builds shipped in v0.1.4). The
  binary is otherwise identical to v0.1.4 — this release exists only to produce
  a complete cross-platform asset set.

## [Unreleased] — 2026-05-23

### Fixed — book render quality + the missing render-audit gate

- **Heading styles now defined** (`Heading1–4` with outline levels): docx-rs ships
  no heading styles, so the prior `agentic book` referenced them undefined →
  **empty TOC** and inconsistent heading formatting. Now defined, so the Word TOC
  field populates and headings are consistent (Calibri/navy).
- **Page-number footer** restored (`Footer` + `PageNum`).
- **`agentic book build` leaves no intermediates**: figures render in a per-book
  system-temp scratch dir created and deleted *within* the step (not a global
  `_work/` in the output, not a blanket end-of-run wipe). Output dir holds only
  `.docx` + a `_render_report.json`.
- **`agentic book audit --current <dir> [--previous <dir>]`** — the render-quality
  gate that was missing: inspects each rendered DOCX (figure count, Heading-style
  presence, page size, byte size) and **compares against the previous iteration**,
  failing on regression (figures dropped, size collapsed, heading styles lost).
  This is why the earlier regression went undetected — `check deliverable` only
  validates *source* policy, never *rendered* fidelity.
- **All 21 Python toolchain scripts purged** from the content store (figures,
  gate, normalize, book builders, `gen_*`, `prompt_rules` are now Rust). The
  framework is Rust-only.

### Changed — Python→Rust toolchain migration

- **`agentic book` (Rust book engine)** replaces the Python `bookkit`/`build_book`
  skill: `agentic-export::book` (docx-rs — A4 typography, Word TOC, shaded-header
  tables, embedded figures + captions) + extended `markdown` parser (tables,
  images). One book = a manifest entry `{key,title,subtitle,chapters:[DB paths]}`;
  figures rendered by `agentic-figures`. The `skills/book-export/*.py` are removed.
- **`agentic check deliverable`** (Rust port of `verify_gate.py`) and
  **`agentic normalize`** (port of `normalize_deliverable.py`) in `agentic-checks`,
  operating on the content store.
- **`agentic gen`** (Rust port of `prompt_rules.py` + the `gen_*.py` family):
  `gen rules` prints the mandatory generation/figure rules; `gen prompt --kind …
  --topic …` assembles a rule-prefixed prompt to pipe to an LLM CLI. The
  deterministic prompt logic now lives in Rust.
- Used to regenerate 22 thesis books (incl. a merged dimension combining
  Dimensions 03 + 07 with the imported AI Norms & Regulations book) — all
  `check deliverable`-compliant.

### Added

- **`agentic-figures` crate** — pure-Rust `figspec` JSON → PNG renderer
  (`plotters`; bar/hbar/line/matrix/quadrant/flow + `resolve_markdown`). The Rust
  port of `render_figspec.py`, first step of the Python→Rust migration of the
  toolchain (no system/C deps; native fonts).
- **`skills/book-export/`** — a reusable book-export skill (function library +
  driver) that turns curated DB content into professional A4 DOCX books:
  `bookkit.py` (engine: Georgia/Calibri typography, block grammar, Word TOC,
  page-referenced XE index, per-chapter "Sources & QR codes") + `build_book.py`
  (driver: markdown→blocks converter, figure rendering via `render_figspec`,
  front/back matter). One book = a manifest entry `{title, chapters:[…]}`. Used
  to generate 23 thesis books (per dimension, per campaign+projects, solutions,
  student notes, AI-audit), all verify_gate-compliant.

- **`inbox` lifecycle** — DB-native port of the Scramblings inbox "meccano".
  `agentic inbox register | status | accept | skip | retire | dedup`. State is
  explicit (`queued → ranked → justified → accepted → archived | skipped`) rather
  than encoded by file location. **Retirement** deletes only the on-disk copy and
  journals an `inbox_archive` entry — the content blob in the store is the
  permanent archive ("empty inbox = done"; nothing is destroyed). Dedup is
  **exact (SHA-256, built-in) + semantic (embedding cosine ≥ 0.90)**, replacing
  the original `text[:80]` lexical method (research: embedding cosine beats
  MinHash/SimHash on near-dup accuracy — RETSim, SemHash). New core
  `inbox` module + migration `0005`.
- **`inbox process`** — self-driving pipeline: advances every queued item
  through rank → justify → accept|hold, **auto-advancing the lifecycle state**,
  auto-writing the passport `claim_audit_results` justification, and **recording
  an `audit_rows` decision per transition**. Autonomous acceptance with a HITL
  safety valve: duplicates and below-threshold novelty auto-accept to
  `lowrankings`; mainline-eligible items are **held for HITL** unless
  `--auto-mainline`; novelty/near-dup scored by embedding cosine (degrades to
  exact-dup-only + HITL-hold without embeddings).
- **`check tree`** — boot-time DB⇄disk integrity gate. Compares every on-disk
  file (under an optional `--prefix`, skipping dot-dirs/`target`/`node_modules`/
  `__pycache__` and the DB files) against its DB blob: on-disk files that differ
  are `tree-drift` (Error → FAIL → exit 1), on-disk files not in the DB are
  `tree-untracked` (Warn), DB paths not materialised are `tree-unmaterialised`
  (Info). Records the verdict in `audit_verdicts`. New core
  `worktree::reconcile`. Run after `check self` at session start.
- **`content ingest` + `content checkout`** — the database can now be the source
  of truth. `ingest` bulk-stages many files in a single commit (`--from-list`
  for an explicit `git ls-files` set; `--replace` makes HEAD's tree exactly the
  staged set, preserving history). `checkout` reproduces the working tree from
  the DB. Round-trip verified byte-for-byte over 637 files. New core API
  `worktree::put_many`.
- **`audit` command group** (PQC non-repudiation, ADR-0039): `keygen`,
  `sign-commits`, `verify`, `record`, `report`. Signs commits and audit-report
  bodies with **ML-DSA-87 (FIPS 204)**. `report` compiles a complete audit —
  what the user did (journal), every change (commit DAG with human/AI
  authorship), **source origins in APA7** (passport `literature_corpus`), a
  **per-item AI-decision index** (`audit_rows` + reconstructed from
  `claim_audit_results`), gate verdicts, and an integrity seal — as MD or JSON,
  whole-project or per `--item`.
- **New `agentic-core` modules**: `signing` (ML-DSA-87 via the pure-Rust
  `fips204` crate; key + signature registry) and `audit` (report compiler +
  APA7 renderer).
- **Docs**: rewritten `README.md`; new `ARCHITECTURE.md`, `QUICKSTART.md`,
  `AUDIT.md`, `RELEASE_NOTES.md`.

### Schema

- **Migration `0004_audit_signatures`** (schema **v4**): `crypto_keys` (active
  ML-DSA-87 public keys; secret-key file reference) and `signatures` (detached
  signatures over commits / audit reports). Additive, idempotent.

### Policy

- **ADR-0039 PQC-only cryptography**: all signing uses ML-DSA-87; classical
  ciphers (Ed25519/RSA/ECDSA) are forbidden. Secret keys (~4.9 KB) are stored as
  a protected file under the user data dir, since they exceed the OS-keychain
  blob limit; only public keys + signatures are persisted in the DB.

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

## [0.1.3] — 2026-05-19

### Fixed

- **`auto_strategy()` now pre-resolves the router's intent before picking
  `Strategy::Embed`.** v0.1.2's logic asked "is there *any* embed-capable
  provider with a key?" — but the router's per-task fallback is hard-coded
  to Voyage for `Task::Embed`, independently of which providers are
  configured. So if Ollama (keyless / always "configured") was the only
  embed-capable provider, `auto_strategy()` happily picked `Embed`, then
  `classify` called `router::route(Task::Embed)` which returned Voyage,
  then `registry::build(Voyage)` failed with "no API key configured for
  provider voyage".
- The new logic asks `router::route(Task::Embed).kind` who *would* be
  picked, then checks `registry::has_key(...)` on that specific
  provider. Only commits to `Strategy::Embed` if the router's chosen
  provider has a key. Same check for `Strategy::Chat` as the fallback.
- Error message now names the providers the router would pick for each
  task and lists the vendor env-var names to set.

### Test

- New `auto_strategy_errors_when_no_provider_has_a_key` test verifying
  the no-keys-anywhere error path emits the helpful hint. Uses
  `#[allow(unsafe_code)]` for the env-mutation it needs (Rust 2024 made
  `set_var`/`remove_var` unsafe).
- 126 / 126 workspace tests passing (was 125; +1 new).

## [0.1.2] — 2026-05-19

### Added

- **Vendor-native API key env vars.** `keychain::get_key` now reads
  `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GOOGLE_API_KEY`,
  `MISTRAL_API_KEY`, `COHERE_API_KEY`, `VOYAGE_API_KEY` and
  `XAI_API_KEY` directly — matching the conventions each vendor's
  official SDK publishes. Lookup order is `AGENTIC_<PROV>_KEY` (explicit
  override) → vendor-native env var → OS keychain. Users who already
  have vendor keys set for the official CLI get zero-config integration.
- **`agentic classify --strategy chat`.** New LLM-driven classification
  that sends each chapter + the slot list to `provider.chat()` and
  parses a JSON `{placement, score, justification, alternatives}`
  response. **No embeddings, no router fallback to Voyage.** Works
  with any chat-capable provider — Anthropic alone is sufficient.
- **`agentic classify --strategy embed`.** Explicit form of the
  existing cosine-on-embeddings pipeline.
- Auto-detect: if `--strategy` is omitted, `embed` is used when any
  embed-capable provider has a key, otherwise `chat`. Falls back with
  a clear error when no provider is configured at all.
- `agentic doctor --json` now reports per-provider key source
  (`AGENTIC_env` / `vendor_env` / `keychain`) and the env-var names
  checked. Human output shows ✓ / · per provider with the source in
  parentheses.
- `agentic provider list` adds `SOURCE`, `VENDOR_ENV`, `AGENTIC_ENV`
  columns so users can see exactly which env var hit.

### Changed

- `ImportOutcome` JSON is unchanged from v0.1.1; no migration needed.
- Router behaviour for `Task::Classify` unchanged — but
  `agentic classify` no longer requires `Task::Embed` to succeed, so
  the "fall through to Voyage" path is bypassable via `--strategy chat`.

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
