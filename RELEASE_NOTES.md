# Release Notes — `mt-fhnw-agentic`

Curated, cross-cutting release history along three axes: **tool version**,
**database schema version**, and **thesis iteration**. For the granular
change list see [`CHANGELOG.md`](CHANGELOG.md); for concepts see
[`ARCHITECTURE.md`](ARCHITECTURE.md).

## At-a-glance matrix

| Tool | DB schema | Thesis iteration | Headline |
|---|---|---|---|
| 0.1.0 | v1 | iter 1–7 | Storage layer, content DAG, project/journal/passport, import, export |
| 0.1.1 | v1 | iter 7–8 | Import per-file failures surfaced; ARM Linux native build; bootstrap `--json` |
| 0.1.3 | v2–v3 | iter 8–13 | Wizard drafts (v2); embeddings (v3); checks; EN-core; quality remediation |
| 0.1.5 | v4 | iter 13–15 | `content ingest`/`checkout` (DB = source of truth); PQC audit + ML-DSA-87 signing (ADR-0039); check-tree boot gate |
| 0.1.10 | v4 | iter 16–20 | Cascade orchestrator; per-book audit_verdicts; gate-matrix profile split (`thesis-default` / `ai-norms-default`) |
| 0.1.15 | v4 | iter 21–27 | AI-Norms parity Round V/D-C; visual-parity gate (ADR-0057); figure-audit / on-page floor / auto-landscape (`agentic-figures`); FhnwProposalParity typography profile |
| 0.1.17 | v4 | iter 28–32 | ARS-gate codification (ADR-0044); ranking-acceptance levels (ADR-0046); FHNW thesis profile completes the deliverable (ADR-0050); scan-artefact persistence cap (ADR-0055); AIBOM SBOM post-deployment lifecycle (ADR-0054) |
| 0.1.20 | v4 | iter 33–42 | `MasterThesis-Bookkit` third parity-gate profile (ADR-0061); per-book parity-fixture routing; Wave-1/2/3 close-out for AI-Norms visual parity residuals |
| 0.1.20+ (unreleased) | v4 | **iter 43–44** | **FHNW MT-Template consolidation as third typography profile (ADR-0064); `external_source` byte-identical delegation for reference-anchored books; sidecar cleanup on all Rust exit paths ([PR #15](https://github.com/dcasota/mt-fhnw-agentic/pull/15)); figspec figure-width floor ([PR #16](https://github.com/dcasota/mt-fhnw-agentic/pull/16)); ListTemplate outline-numbering gated behind sidecar flag; RUSTSEC-2026-0204 `crossbeam-epoch` bump** |

## Database schema history

- **v1 `0001_initial`** — projects, blobs/trees/commits/refs (content DAG),
  journal_entries, passport_entries, audit_rows, audit_verdicts, schemas,
  protocols, adrs, fts, api_cache, i18n_strings.
- **v2 `0002_wizard_drafts`** — `wizard_drafts` (TUI onboarding state).
- **v3 `0003_embeddings`** — `embeddings` (vectors keyed by blob + model).
- **v4 `0004_audit_signatures`** — `crypto_keys` + `signatures` (PQC
  non-repudiation; ML-DSA-87 public keys and detached signatures over commits
  and audit reports). Additive and idempotent; existing DBs migrate on next open.

## Tool releases

### Unreleased (DB v4) — 2026-07-10 (iter44 arc)

**Added**

- **`external_source` field on `BookSpec`** (iter44.p,
  [`b966285`](https://github.com/dcasota/mt-fhnw-agentic/commit/b966285),
  ADR-0064). Byte-identical delegation for reference-anchored books:
  `commands/book.rs::build` copies the reference file into the snapshot
  byte-verbatim and skips Word finalize. Used for `master_thesis` — the
  Rust port cannot byte-parity-match the FHNW-approved June-8 reference
  produced by the MT-Template Python + PowerShell pipeline (17 %+ pixel-diff
  floor across 40 iterations of iter44.a-o); delegation makes the guarantee
  a pipeline property, not a heroic effort. Marked in
  `_render_report.json` as `"delegated_to_external_pipeline": true`.

**Fixed**

- **ListTemplate outline-numbering behind sidecar flag** (iter44.q,
  [`85e6d0a`](https://github.com/dcasota/mt-fhnw-agentic/commit/85e6d0a)).
  `FhnwHeaderSidecar` gains
  `outline_numbering_enabled: bool` (default `false`); the finalize block
  is now `if ($side.outline_numbering_enabled) { ... }`. Master_thesis and
  bookkit opt in; the 9 campaign profiles opt out. Closes doubled chapter
  numbers on campaigns whose source markdown already carries literal
  chapter numbers.
- **Sidecar cleanup on all Rust exit paths, not just success**
  ([PR #15](https://github.com/dcasota/mt-fhnw-agentic/pull/15), iter44.ag,
  [`69f70fc`](https://github.com/dcasota/mt-fhnw-agentic/commit/69f70fc)).
  `*.docx.fhnw_header.json` + `*.fhnw_logo.png` transient hand-offs to Word
  COM are cleaned up whether finalize succeeded or failed — moved before
  the `bail!` on non-zero exit status. Reported 2026-07-09: a killed cascade
  snapshot left 15 sidecars + 14 logos orphaned alongside 16 real docx
  deliverables.
- **Minimum readable width for figspec-rendered figures**
  ([PR #16](https://github.com/dcasota/mt-fhnw-agentic/pull/16), iter44.ap,
  [`fb6fb0e`](https://github.com/dcasota/mt-fhnw-agentic/commit/fb6fb0e)).
  `image_dims_to_emu` Branch 2 previously kept native ~1-in width for
  figspec PNGs; they clustered as unreadable thumbnails. Floor added: if
  natural width < 1.75 in, scale to `IMAGE_MAX_W_EMU` (5.91 in)
  preserving aspect ratio via `snap_emu_to_grid`.
- **`crossbeam-epoch 0.9.18 → 0.9.20`** for RUSTSEC-2026-0204 (invalid
  pointer deref in `fmt::Pointer` impl for `Atomic`/`Shared`, advisory
  2026-07-06). Transitive supply-chain fix; no first-party call site
  touches `Atomic::fmt` / `Shared::fmt`.

**Policy — unchanged**

- ADR-0039 PQC-only cryptography (ML-DSA-87 via `fips204`); classical
  ciphers still forbidden.

### 0.1.20 — 2026-06-04 (DB v4)
- MasterThesis-Bookkit profile + per-book parity-gate routing (ADR-0061).
  Wave-1/2/3 close-out for AI-Norms visual parity. See
  [`CHANGELOG.md#0120---2026-06-04`](CHANGELOG.md#0120--2026-06-04).

### 0.1.17 — 2026-05-30 (DB v4)
- ARS-gate codification (ADR-0044); ranking-acceptance levels (ADR-0046);
  scan-artefact persistence cap (ADR-0055); AIBOM SBOM lifecycle (ADR-0054);
  FHNW thesis profile completes the deliverable (ADR-0050).

### 0.1.15 — 2026-05-29 (DB v4)
- AI-Norms parity Round V/D-C; visual-parity gate (ADR-0057); figure-audit
  + on-page pt floor + auto-landscape in `agentic-figures`;
  FhnwProposalParity typography profile.

### 0.1.10 — 2026-05-28 (DB v4)
- Cascade orchestrator; per-book `audit_verdicts`; gate-matrix profile
  split (`thesis-default` / `ai-norms-default`); page-boundary body-range
  scoping.

### 0.1.5 — 2026-05-27 (DB v4)
- `content ingest`/`checkout` — DB becomes the byte-authoritative source
  of truth (round-trip verified over 637 files). PQC audit + ML-DSA-87
  signing (ADR-0039); `check tree` boot gate; audit report compiler
  + APA7 renderer. Migration `0004_audit_signatures`.

### 0.1.3 — 2026-05-19 (DB v2–v3)
- Wizard drafts, embeddings, integrity checkers, EN-core enforcement, and the
  13-issue quality-remediation gates (blocking English/reference/number gates,
  figure-standards). See `CHANGELOG.md`.

### 0.1.1 — 2026-05-19 (DB v1)
- `import dir` surfaces per-file failures and exits non-zero on any failure;
  ARM Linux native release build; `bootstrap/init.ps1` resolves project ID via
  `--json`.

### 0.1.0 — 2026-05 (DB v1)
- Initial storage layer, content DAG, project/journal/passport, import/export,
  provider registry, TUI wizard.

## Thesis-iteration alignment

The tool versions track the thesis's iteration cadence (governance and content
live in the thesis repo; the tool is the runtime):

- **iter 1–7** — corpus ingestion, dimension drafting, SDD chain bootstrap.
- **iter 8–9** — EN-core remediation (ADR-0035), 3-tier depth, snapshots.
- **iter 10–12** — inbox processing, ISO/IEC 42001 transitions, CNSA 2.0,
  sovereign Campaign C09.
- **iter 13** — briefing-deck processing, figure fixes, **13-issue
  quality-remediation** (blocking gates + figure-renderer rewrite + verified-facts).
- **iter 14–15** — external-input dispatch (SDD bundle, Apple corecrypto,
  NSA MCP, …); **DB made the source of truth**; **PQC audit /
  non-repudiation** (ADR-0039); `content ingest`/`checkout` round-trip
  verified over 637 files.
- **iter 16–20** — cascade orchestrator introduces per-book `audit_verdicts`;
  gate-matrix profile split; page-boundary body-range scoping.
- **iter 21–27** — AI-Norms parity Round V / D-C; the figure-renderer gains
  auto-landscape + a 7 pt on-page floor; ADR-0057 visual-parity gate lands
  as the third scan-time integrity check.
- **iter 28–32** — governance ADR wave: ARS-gate codification (ADR-0044),
  ranking-acceptance levels (ADR-0046), FHNW thesis profile completes the
  deliverable (ADR-0050), scan-artefact persistence cap (ADR-0055),
  AIBOM SBOM lifecycle (ADR-0054).
- **iter 33–42** — MasterThesis-Bookkit third profile (ADR-0061) + per-book
  parity-gate routing; Wave-1/2/3 close-out for AI-Norms visual-parity
  residuals; FHNW MT-Template consolidation as the third typography
  profile (ADR-0064); Windows CreateProcess 32 KB + PowerShell `-File`
  codepage fixes (`finalize-temp-file-bom` memory).
- **iter 43–44** — book-build hardening (iter44.p → iter44.ap): master_thesis
  routes through `external_source` byte-identical delegation to the FHNW
  MT-Template Python + PowerShell pipeline (SHA256-identical to the
  June-8 reference); sidecar cleanup on all Rust exit paths
  ([PR #15](https://github.com/dcasota/mt-fhnw-agentic/pull/15)); ListTemplate
  outline-numbering behind a sidecar flag (closes doubled chapter numbers on
  campaign books); figspec figure-width floor
  ([PR #16](https://github.com/dcasota/mt-fhnw-agentic/pull/16));
  RUSTSEC-2026-0204 `crossbeam-epoch` bump. Chain sealed at 4 934 commits
  with ML-DSA-87 as of 2026-07-10; per-book detection gate ships in
  `scratch/check_cascade_content.ps1` covering XML wellformedness,
  heading-length ceiling, Word COM openability, and baseline size-delta.

## Upgrade notes

- Opening an older `thesis.db` auto-applies pending migrations (to v4). Back up
  the DB first (`Copy-Item thesis.db thesis.db.bak`).
- After upgrading, run `agentic audit keygen` once, then `audit sign-commits` to
  establish the signed baseline.
- If a manifest carries `"external_source": "<abs-path>"` on any book, the
  path must exist at build time and be readable by the Rust process. The
  copy is byte-verbatim; changing the reference file changes the shipped
  bytes without any Rust code change.
- If Word finalize fails on a book (RPC disconnect, external kill), the
  new sidecar-cleanup pass still runs — but external SIGKILL of the Rust
  process itself can never trigger Rust code (see `iter44.ag` commit
  message). Post-mortem cleanup of `snapshots/<ts>/*.fhnw_header.json` and
  `snapshots/<ts>/*.fhnw_logo.png` is a manual step in that case.
