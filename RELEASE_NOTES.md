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
| 0.1.3+ (unreleased) | **v4** | **iter 13–14** | **content ingest/checkout (DB = source of truth); PQC audit + ML-DSA-87 signing** |

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

### Unreleased (DB v4) — 2026-05-23
**Added**
- `content ingest` — bulk-stage many files into a project's working tree in a
  **single commit**; `--from-list` takes an explicit path list (e.g.
  `git ls-files`), `--replace` makes HEAD's tree **exactly** the staged set
  while preserving history. (`worktree::put_many`)
- `content checkout` — write a project's entire working tree to disk; the inverse
  of `ingest`. Together they let the **database be the source of truth**
  (round-trip verified byte-for-byte over 637 files).
- `audit` command group (ADR-0039, PQC-only):
  - `keygen` — ML-DSA-87 (FIPS 204) keypair; secret to a protected file (PQC keys
    exceed the OS-keychain blob limit), public to `crypto_keys`.
  - `sign-commits` / `verify` — sign and verify the whole commit chain
    (non-repudiation; tamper-evident).
  - `record` — append one AI/LLM decision to the per-item `audit_rows` index.
  - `report` — compile a complete, signed audit (user actions, APA7 source
    origins, AI-decision index, gate verdicts, integrity seal); MD or JSON;
    whole-project or per-item (`--item`).
- `agentic-core` modules `signing` (ML-DSA-87 via `fips204`) and `audit`
  (report compiler + APA7 renderer); migration `0004`.

**Policy**
- **ADR-0039 PQC-only cryptography**: all signing uses ML-DSA-87; classical
  ciphers (Ed25519/RSA/ECDSA) are forbidden. Aligns the tool with the thesis's
  own CNSA 2.0 / FIPS 204 recommendations.

**Notes**
- `fips204` (pure-Rust, no C toolchain) added to the workspace; no
  classical-crypto crate is on the signing path.

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
- **iter 14 (in progress)** — external-input dispatch (SDD bundle, Apple
  corecrypto, NSA MCP, …); **DB made the source of truth**; **PQC audit /
  non-repudiation** (this release); ADR-0039.

## Upgrade notes

- Opening an older `thesis.db` auto-applies pending migrations (to v4). Back up
  the DB first (`Copy-Item thesis.db thesis.db.bak`).
- After upgrading, run `agentic audit keygen` once, then `audit sign-commits` to
  establish the signed baseline.
