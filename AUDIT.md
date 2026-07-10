# Audit & Non-Repudiation — `mt-fhnw-agentic`

How the tool makes **every change auditable** and **cryptographically
attributable**, and how to ask the database for a complete audit: what the user
did, the origin of sources (in APA7) that fed each document, and a full index of
AI (LLM) decisions per item. Signing is **PQC-only (ML-DSA-87, FIPS 204)** per
[ADR-0039](../specs/adr/0039-pqc-only-cryptography.md) — no classical ciphers.

## 1 What the audit is built from

Four append-only surfaces already in `thesis.db`, plus the signature registry:

| Surface | Table | Answers |
|---|---|---|
| Journal | `journal_entries` | **What the user did** — actor, action, reasoning, approval, timestamp |
| Commit DAG | `commits` (+ `trees`,`blobs`) | **Every change** — hash-chained, `actor_kind ∈ human/ai/hook/system` |
| Material passport | `passport_entries` | **Source origins → APA7** + the AI ranking decisions, with provenance |
| AI-decision index | `audit_rows` | **LLM decisions per item** — agent, action, target, result, model, tokens |
| Gate verdicts | `audit_verdicts` | check/gate PASS/WARN/FAIL per checkpoint |
| Signatures | `crypto_keys`, `signatures` | **ML-DSA-87** public keys + detached signatures (non-repudiation) |

## 2 Non-repudiation: how it works

```
keygen  ─► ML-DSA-87 keypair; secret → protected file (user data dir, not git/DB);
           public → crypto_keys (active)
sign-commits ─► sig = MLDSA87(sk, commit_sha)  for every commit  → signatures
report  ─► sign the rendered report body too    → signatures(target='audit_report')
verify  ─► check each signature vs the public key; any tampered byte ⇒ INVALID
```

The commit SHA already hash-chains tree+parents+author+timestamp, so the content
is tamper-evident; the ML-DSA-87 signature over each commit SHA makes authorship
**cryptographically provable** and quantum-safe. Verified behaviour: flipping a
single byte of one stored signature turns `135 valid` into `134 valid, 1
invalid`.

### Why ML-DSA-87 (not Ed25519)
Signing a PQC-migration thesis's audit trail with classical crypto would
contradict the thesis. ADR-0039 mandates ML-DSA (FIPS 204); ML-DSA-87 (Category
5) matches CNSA 2.0 and the thesis's "every image signed by ML-DSA-87"
recommendation. Trade-off: keys/signatures are KB-scale (sk 4 896 B), which is
why the secret key is a file, not a keychain entry.

## 3 Commands

```bash
agentic audit keygen --signer "Your Name"            # one-time per project/machine
agentic audit sign-commits --project <ID>            # sign the whole chain
agentic audit verify       --project <ID>            # integrity check
agentic audit record --project <ID> --agent <model> --action "<decision>" \
    --target <item> --result <pass|warn|fail|ok|info> --model <model> --iteration <N> \
    --detail "<why>"                                 # record one AI decision (going-forward)
agentic audit report --project <ID> [--item <substr>] [--format md|json] [--to <file>]
```

`report` compiles the complete audit (sections below), signs the rendered body,
and (for `md`) appends the signature block. `--item` filters to one
campaign/dimension/document by substring of commit/journal/passport content.

## 4 What a report contains

1. **Summary** — counts + signing algorithm.
2. **What the user did** — the journal, with approvals.
3. **Change records** — every commit, actor kind, iteration, signed yes/no.
4. **Source origins (APA7)** — each `literature_corpus` source rendered APA7,
   tagged `[ai_suggestion]`/`[human]`, with the items it was embedded into
   (derived from `claim_audit_results` provenance).
5. **AI (LLM) decision index, per item** — `audit_rows` (recorded) plus
   reconstructed ranking decisions from `claim_audit_results`.
6. **Gate verdicts** — `audit_verdicts`.
7. **Integrity seal** — HEAD commit, signed-commit count, key id.
8. **Cryptographic signature** — ML-DSA-87 signature over the report body +
   body SHA-256, with verification instructions.

Example APA7 line:
`Amodei, D., Olah, C., … & Mané, D. (2016). Concrete Problems in AI Safety. arXiv. https://arxiv.org/abs/1606.06565`

## 5 Backfill vs going-forward

- **Going-forward**: record each AI decision (`audit record`) and gate verdict as
  it happens, so `audit_rows`/`audit_verdicts` are contemporaneous.
- **Backfill**: historical AI ranking decisions are reconstructed in the report
  from `claim_audit_results` (tagged "reconstructed"), because the early external
  generation pipeline did not write `audit_rows`. Raw session transcripts remain
  the deepest historical record.

## 6 Current audit log (latest iteration)

Snapshot of project `01KS117RNSSE7NERSWM0H6SJ6P` as of 2026-07-10
(regenerate with `agentic audit report --project <ID> --to out/AUDIT_latest_EN.md`):

- **User/journal actions:** 212 · **Commits:** 4 934 (**4 934 signed of
  4 934**, ML-DSA-87 key `2853f41320db8a80`) · **APA7 source origins:** 1 673 ·
  **AI decisions indexed:** 327 · **Gate verdicts:** 5 421 · **External-platform
  sessions (ADR-0053):** 1 (Claude Code session)  · **Signing:** ML-DSA-87
  (ADR-0039).

Growth from the 0.1.5-era baseline (135 commits) reflects the iter 15–44 arc:
external-input dispatch, per-book cascade orchestration + Wave-1/2/3 close-out,
FHNW MT-Template consolidation (ADR-0064) and the iter44 hardening arc
(external_source delegation, sidecar cleanup, figspec width floor).

The full signed report — every user action + APA7 origin + AI decision
+ gate verdict + the ML-DSA-87 integrity seal — lives as an in-DB blob at
`out/sources/AI_Audit_BOM_EN.md` (source) and renders to the
`AI-Audit-BOM.docx` book in each snapshot. Regenerate on demand via
`agentic audit report`; the rendered docx post-processes with
`scratch/aibom_table_widths.ps1` to apply the manually-adjusted Journal-table
column widths (# 562 / When 1701 / Actor 1418 / Action 992 / Approval 1134 /
Description 3487 twips).

## 7 Verifying a report independently

1. Take the report body (everything before "## 8 Cryptographic signature").
2. Recompute its SHA-256 — must equal the stated "Body SHA-256".
3. Fetch the active public key from `crypto_keys` and verify the ML-DSA-87
   signature over the body (`agentic audit verify` covers commit signatures;
   report-body verification follows the same scheme).
