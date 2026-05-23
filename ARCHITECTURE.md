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
```

`agentic-core` is the only crate that talks to SQLite. It stays crypto-pure
(ML-DSA-87 via `fips204`) and IO-light; the CLI layer owns keychain/file glue.

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
