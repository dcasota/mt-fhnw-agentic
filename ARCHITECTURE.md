# Architecture — `mt-fhnw-agentic`

This document explains what the tool is, the concepts it is built on, and how
the pieces fit together, with ASCII schemas. For commands see
[`README.md`](README.md); for setup/workflow see [`QUICKSTART.md`](QUICKSTART.md);
for the audit model see [`AUDIT.md`](AUDIT.md).

## 1 What it is

A single Rust binary plus a single SQLite database (`thesis.db`) that holds the
entire state of a Spec-Driven-Development research project: a content-addressed
working tree (git-like), an append-only journal and material passport,
embeddings, an audit trail, and post-quantum cryptographic signatures. Everything
a thesis needs — store, check, render, attest — is in one place, invoked over the
shell so any AI CLI drives it identically. No servers, no MCP, no Python runtime.

## 2 The core idea: the DB is the source of truth

```
        ingest (one commit)                 checkout (inverse)
 working tree  ───────────────►   thesis.db   ───────────────►  working tree
 (git files)                    (content store)                  (reproduced)

 byte-for-byte round-trip:  checkout(ingest(tree)) == tree   (verified, SHA-256)
```

The on-disk files are convenient editing surface; the authoritative copy lives
in the content store. `content ingest --replace` makes HEAD's tree exactly the
git-tracked set; `content checkout` reproduces it. (See ADR / QUICKSTART.)

## 3 Crate graph

```
                      ┌─────────────────────────────────────────────┐
                      │  agentic  (bin) — clap CLI, commands/*.rs     │
                      └───────────────┬─────────────────────────────┘
        ┌───────────────┬─────────────┼───────────────┬───────────────┐
        ▼               ▼             ▼               ▼               ▼
 agentic-core   agentic-checks  agentic-import  agentic-export  agentic-providers
 (storage,      (self,          (md/DOCX/PDF    (DOCX FHNW,     (Anthropic, OpenAI,
  content DAG,   citations,      ingest +        PDF Typst,      Google, Mistral,
  journal,       contamination,  classify)       markdown)       Cohere, Voyage,
  passport,      writing_qual.)        │              │           Ollama, Grok)
  embeddings,          │               │              │
  signing, audit)      └───────────────┴──────────────┘
        │                         all read/write
        ▼
   thesis.db (SQLite, WAL)            agentic-tui (onboarding wizard)
                                      agentic-resources (templates, seeds)
                                      agentic-figures (figspec→PNG, plotters)
                                      agentic-thesis-template (FHNW MT-Template
                                        embedded fixtures, ADR-0064; loaded by
                                        agentic-export's FhnwMtTemplate profile)
```

`agentic-core` is the only crate that talks to SQLite. It stays crypto-pure
(ML-DSA-87 via `fips204`) and IO-light; the CLI layer owns keychain/file glue.

`agentic-thesis-template` (added in 0.1.20) embeds the FHNW-canonical parts as
byte-verbatim fixtures — `styles.xml` (350 KB, 178 base styles), `numbering.xml`,
`settings.xml` (mirrorMargins + evenAndOddHeaders), `theme1.xml`, `fontTable.xml`,
`webSettings.xml`, `content_types.xml`, both `_rels` files, and
`assets/fhnw_logo.png` (129 051 B, byte-identical to the MT-Template asset).
`agentic-export` loads them when the `FhnwMtTemplate` typography profile is
selected on a book (`thesis_typography: "fhnw-mt-template"` in
`out/book_manifest.json`).

## 4 Data model (SQLite, schema v4)

```
 projects ──1:N── journal_entries          (what the user/agent did; append-only)
    │     ──1:N── passport_entries          (literature_corpus, claim_audit_results, …)
    │     ──1:N── audit_rows                (per-item AI/LLM decision index)
    │     ──1:N── audit_verdicts            (gate verdicts per checkpoint)
    │
    └── head_ref ─► refs ─► commits ─► trees ─► blobs        (the content DAG)
                                  │
 crypto_keys ──1:N── signatures ──┘  (ML-DSA-87 public keys + detached signatures
                                      over commits and audit reports; ADR-0039)
 embeddings (blob_sha, model, vector)         api_cache, schemas, protocols, adrs, i18n
```

### The content DAG (git-like, content-addressed)

```
 blob   = (sha256, mime, content, lang)                    immutable bytes
 tree   = (sha256, entries[] = {path, blob_sha, mode})     a flat path→blob map
 commit = (sha256 = H(tree+parents+author+actor_kind+iter+msg+ts),
           parent, actor_kind ∈ {human|ai|hook|system}, iteration, message, ts)
 ref    = (name = "<project>/main", commit_sha)             moves with each commit

   commit_n ──parent──► commit_n-1 ──parent──► … ──► commit_0
      │ tree                                            (hash chain = tamper-evident)
      ▼
   tree_n ──► {blobs}
```

Because each commit's SHA-256 incorporates its tree, parents, author and
timestamp, altering any historical blob or commit breaks the chain — the
foundation the PQC signatures attest (see §6).

## 5 The SDD chain (governance the storage enforces)

```
 PRD / RRD ──► FRDs ──► ADRs ──► tasks ──► implementation
   (REQ-N)     (FR-x)  (0001..)  backlog    deliverables
      ▲                                          │
      └──────────── every artefact traces back ──┘
                    (claim_audit_results in the passport carry score / placement /
                     justification / provenance for each item)
```

The material passport (`passport_entries`) is the provenance ledger: every claim
and source carries a justification record. The journal records every action. The
commit DAG records every change. Together they make the project auditable.

## 6 Audit & non-repudiation flow (PQC)

```
 keygen ─► ML-DSA-87 keypair (FIPS 204)
            secret → protected file (user data dir, NOT in git/DB)
            public → crypto_keys (active)

 sign-commits ─► for each commit: sig = MLDSA87.sign(sk, commit_sha)
                 → signatures(target_kind='commit', target_id=sha, …)

 report ─► compile {journal, commits, passport→APA7, audit_rows, verdicts, seal}
           render (md|json) ─► sig = MLDSA87.sign(sk, body)
           → signatures(target_kind='audit_report', target_id=H(body))

 verify ─► recompute / check every signature against the public key
           tamper in any signed byte ⇒ verification FAILS
```

ADR-0039 mandates PQC-only: signatures are ML-DSA-87, no classical ciphers, so
the audit trail stays valid past the classical-deprecation horizon — the tool is
itself a worked example of the thesis's crypto-agility recommendation.

## 7 Generation boundary (important)

The tool stores, checks, renders, and attests. Heavy *generation* (LLM drafting,
figure rendering) historically ran in an external pipeline (`code/tools/*.py` +
`claude -p`) writing to the working tree, which is then ingested into the DB.
Going forward, AI decisions are recorded into `audit_rows` (`agentic audit
record`) and gate verdicts into `audit_verdicts`, so the audit trail captures the
generation that the content DAG alone does not.

## 8 Languages

Working language is English (`--lang en`); DE/FR/IT/RM/HI exports are supported
via the `--lang` flag and the `i18n_strings` table (translation pipeline is gated
behind explicit go-ahead).

## 9 Book export (`agentic book`, all-Rust)

Turning curated DB content into professional A4 DOCX books is a **Rust command**,
not a Python skill (the Python toolchain has been ported out — see §10):

```
 content sources (DB)  ──agentic-figures──►  resolved md + figures/*.png
        │  (figspec→PNG, plotters)                 │
        ▼                                          ▼
 agentic book  ──markdown→blocks──►  agentic-export::book  ──►  Book.docx
 (cmd: manifest = title +           (docx-rs: A4 typography, TOC,   (title page,
  ordered DB chapter paths)          tables, embedded figures)       TOC, figures)
```

- **`agentic-figures`** — `figspec` JSON → PNG via `plotters` (bar/hbar/line/
  matrix/quadrant/flow) + `resolve_markdown()`. Pure Rust, no system deps.
- **`agentic-export::book`** — `docx-rs` renderer: A4 Georgia/Calibri typography,
  Word **TOC** (heading styles), shaded-header tables, embedded figures with
  captions. `markdown.rs` parses headings/paragraphs/lists/**tables**/**images**.
- **`agentic book`** (CLI) — reads a manifest `{books:[{key,title,subtitle,
  chapters:[DB paths]}]}`, pulls each chapter from the content store, resolves
  figures, and writes one DOCX per book.

```
agentic book --project <ID> --manifest books.json --out <dir> [--only <key>]
```

Chapters are the same gate-passing markdown the framework governs, so books
inherit English-core / reference / number / figure-standard compliance (verify
with `agentic check deliverable`).

### 9.1 Typography profiles (`thesis_typography` on each book)

`agentic-export::book` picks a typography profile per book. Three profiles
ship today (ADR-0002 / ADR-0050 / ADR-0061 / ADR-0064):

| Profile key | Purpose | Loads from |
|---|---|---|
| `default` (Designer) | Generic A4 book — Georgia body + Calibri headings; general-purpose books | Built-in defaults |
| `fhnw-proposal-parity` | FHNW proposal → thesis outline parity target (ADR-0050) | Built-in styles + margin ADR overrides |
| `fhnw-mt-template` | FHNW-canonical thesis look-and-feel (ADR-0064): Palatino Linotype pinned on all four `<w:rFonts>` slots; black H1 colour; accent + hyperlink `#294F6D`; mirrored margins; STYLEREF chapter refs + PAGE fields; multilevel outline list bound to Heading 1/2/3; Roman/Arabic per-section pagination; `.dotx` companion save | `agentic-thesis-template` embedded fixtures (styles.xml, numbering.xml, settings.xml, theme1.xml, fhnw_logo.png) |

Any book in the manifest may set `"thesis_typography": "<key>"` to opt into a
profile.

### 9.2 `external_source` — byte-identical delegation (iter44.p, ADR-0064)

For books whose reference deliverable is authoritative — the FHNW-approved
June-8 `master_thesis.docx` in this project's case — the manifest can set
`"external_source": "<abs-path>"` on that book entry. `commands/book.rs::build`
then copies the reference file into the snapshot byte-verbatim and skips Word
finalize entirely; the shipped SHA256 is guaranteed to match the reference.

The render report tags the book with:

```json
{
  "key": "master_thesis",
  "delegated_to_external_pipeline": true,
  "finalize_skipped": "byte-identity requires no post-processing",
  "docx_bytes": 1405721,
  "external_source": "…/FHNW2026_DanielCasota_MT_en.docx"
}
```

Rationale: the MT-Template Python + PowerShell pipeline that produced the
June-8 reference is out-of-scope for the Rust port; any reconstruction of the
identical bytes through Rust would drift by a 17 %+ pixel-diff floor
(empirically measured across 40 iterations of iter44.a-o). Delegation makes
the byte-identical guarantee a pipeline property, not a heroic effort.

### 9.3 Finalize sidecars (Windows only)

For books that route through Word COM finalize (all profiles except delegated
ones), `commands/book.rs::finalize_docs` writes two transient sidecars per
docx into the snapshot directory before invoking `powershell -File`:

- `<book>.docx.fhnw_header.json` — logo path, header font + size, footer
  page-number style, `outline_numbering_enabled` flag, etc.
- `<book>.fhnw_logo.png` — the FHNW logo bytes staged for the Word COM
  `InlineShapes.AddPicture` call.

**Both are deleted on all Rust exit paths after finalize returns** — success
or failure (iter44.ag, [PR #15](https://github.com/dcasota/mt-fhnw-agentic/pull/15)).
The only detritus case is external SIGKILL of the Rust process itself, in
which case manual cleanup of the snapshot directory is required.

## 10 Toolchain is Rust (no Python in the pipeline)

The deliverable pipeline that once lived in `code/tools/*.py` is now Rust:

| Was (Python) | Now (Rust) |
|---|---|
| `render_figspec.py` | `agentic-figures` crate |
| `verify_gate.py` | `agentic-checks::deliverable_gate` (`agentic check deliverable`) |
| `normalize_deliverable.py` | `agentic-checks::normalize` (`agentic normalize`) |
| `bookkit.py` + `build_book.py` + `build_*_docx.py` | `agentic-export::book` (`agentic book`) |
| `gen_*.py` + `prompt_rules.py` (generation orchestration) | being ported to `agentic` commands + `agentic-providers` |
