# Quickstart — `mt-fhnw-agentic`

How to build the tool, set it up in a new location, run an iteration, keep the
database as the source of truth, and do housekeeping. Commands use PowerShell on
Windows; the same commands work in bash (swap `$env:X` for `export X`).

## 1 Build the binary

```powershell
cd C:\path\to\mt-fhnw-agentic
cargo build --release -p agentic
# binary: target\release\agentic.exe   (~3–4 min cold; incremental ~30s)
```

Optionally put it on PATH, or reference it by full path (`$agx` below).

```powershell
$agx = "C:\path\to\mt-fhnw-agentic\target\release\agentic.exe"
```

## 2 Configure in a new project location

The DB lives in the project working directory as `thesis.db` (override with
`--db` or `$env:AGENTIC_DB`). To stand up a fresh project:

```powershell
cd C:\path\to\my-thesis
& $agx init --no-wizard --working-lang en --institution fhnw-mas   # or: & $agx init  (TUI wizard)
& $agx project list                                                # note the project ULID
$proj = "<ULID-from-list>"
& $agx doctor --json                                               # detected_cli_context should be your CLI
```

Provider keys (for embed/classify; never committed) come from the OS keychain or
env vars — e.g. `$env:ANTHROPIC_API_KEY` or `AGENTIC_<PROVIDER>_KEY`:

```powershell
& $agx provider list
& $agx provider set-key anthropic        # stores in OS keychain (short API keys only)
```

## 3 Make the database the source of truth

Ingest exactly the git-tracked files (the authored sources) in one commit, then
prove the round-trip:

```powershell
git -c core.quotepath=false ls-files > $env:TEMP\tracked.txt
& $agx content ingest --project $proj --root "." --from-list $env:TEMP\tracked.txt --replace `
      --author "Your Name" --message "ingest source of truth"

# Verify checkout reproduces the tree byte-for-byte:
& $agx content checkout --project $proj --to $env:TEMP\restored
# (diff $env:TEMP\restored against the repo with Get-FileHash; expect 0 mismatches)
```

Re-run `content ingest --replace` whenever you want the DB to re-mirror the
current sources (e.g. at the end of an iteration, after committing new files).

## 4 Turn on non-repudiation (PQC signing)

```powershell
& $agx audit keygen --signer "Your Name"      # ML-DSA-87 keypair (FIPS 204, ADR-0039)
& $agx audit sign-commits --project $proj      # sign every commit
& $agx audit verify       --project $proj      # 'N valid, 0 invalid, 0 unsigned'
```

The secret key is written to a protected file under your user data dir (NOT in
git/DB, because PQC keys exceed the OS-keychain blob limit). Back it up securely;
losing it means you can sign new commits with a new key but cannot reproduce the
old signatures. See [`AUDIT.md`](AUDIT.md).

## 4b Boot integrity check (DB ⇄ disk)

When source files are materialised on disk *and* stored in the DB, they can
drift. Run the boot check at the **start of every session** — it fails (exit 1)
if any on-disk file differs from its DB blob:

```powershell
& $agx check self                                  # DB structural integrity
& $agx check tree --project $proj --root "."        # DB ⇄ disk consistency (boot gate)
& $agx check tree --project $proj --root "." --prefix "specs/"   # scope to one area
```

- **`tree-drift` (Error → FAIL)**: an on-disk file differs from the DB → reconcile
  before working. Restore the authoritative bytes from the DB (not git, which may
  re-apply line-ending normalisation):
  ```powershell
  & $agx content checkout --project $proj --to "." --prefix "specs/adr/0039-….md"
  ```
  Or, if the on-disk edit is the intended new truth, capture it:
  `agentic content ingest … --replace`.
- **`tree-untracked` (Warn)**: a file on disk is not yet in the DB → `content ingest`.
- **`tree-unmaterialised` (Info)**: a DB path isn't on disk (expected when the DB
  is the file's only home) → `content checkout` if you need it locally.

The verdict is recorded in `audit_verdicts` (checkpoint `pre_iteration`), so the
boot check is itself part of the audit trail.

## 4c Inbox lifecycle (intake → acceptance → retirement)

Raw inputs land in `inbox/`. The lifecycle has explicit states
(`queued → ranked → justified → accepted → archived | skipped`); the content
blob in the DB is the permanent archive, so **retiring** an item removes only
its on-disk copy ("empty inbox = done", nothing destroyed).

```powershell
& $agx inbox register --project $proj                       # capture inbox/* blobs as queued
& $agx embed          --project $proj --prefix inbox         # vectors (local model) for scoring
& $agx inbox process  --project $proj --model <embed-model>  # SELF-DRIVING: rank→justify→accept|hold
#   ^ auto-advances state, auto-writes passport justifications, records audit_rows per step;
#     duplicates/low-novelty -> lowrankings (auto); mainline-eligible -> held for HITL (review).
& $agx inbox accept   --project $proj --path "inbox/x.md" --placement thesis_main --hitl  # confirm a held item
& $agx inbox skip     --project $proj --path "inbox/README.md"      # non-input
& $agx inbox retire   --project $proj --path "inbox/x.md"           # delete disk copy; blob kept
& $agx inbox status   --project $proj                               # "empty-inbox = done"
```

`retire` refuses unless the content blob is in the DB **and** the item is
accepted/justified/skipped — so an item is never removed from disk before it is
both captured and adjudicated. Restore a retired item's file with
`content checkout --to . --prefix inbox/x.md`.

## 5 A typical iteration

```
   ┌── 1. pull inputs ──────────────────────────────────────────────┐
   │   drop sources into inbox/ ; agentic import file/dir            │
   ├── 2. rank & justify ───────────────────────────────────────────┤
   │   embed + classify ; write claim_audit_results to the passport   │
   ├── 3. draft / edit deliverables (working tree)                   │
   ├── 4. check (gates) ────────────────────────────────────────────┤
   │   agentic check self ; writing-quality ; citations ; contamination
   ├── 5. record AI decisions + gate verdicts ──────────────────────┤
   │   agentic audit record … (per LLM decision)                     │
   ├── 6. journal every action ; git commit                          │
   ├── 7. ingest --replace (DB = source of truth) ; sign-commits     │
   └── 8. audit report (signed) ; export DOCX/PDF                    ┘
```

Concretely:

```powershell
# 2 rank/justify
& $agx import dir .\inbox --project $proj
& $agx embed --project $proj
& $agx classify --project $proj
& $agx passport append --project $proj --section claim_audit_results --json-file car.json

# 4 checks
& $agx check self
& $agx check writing-quality --project $proj
& $agx check citations       --project $proj
& $agx check contamination   --project $proj

# 5/6 record + journal
& $agx audit record --project $proj --agent "claude-opus-4-7" --action "draft ch5" --target "ch5" --result ok --model "claude-opus-4-7" --iteration 14
& $agx journal append --project $proj --actor me --action-type draft --description "iter-14 …"
git add -A; git commit -m "iter-14: …"

# 7 source of truth + sign
git -c core.quotepath=false ls-files > $env:TEMP\tracked.txt
& $agx content ingest --project $proj --root "." --from-list $env:TEMP\tracked.txt --replace --author "Your Name" --message "iter-14 ingest"
& $agx audit sign-commits --project $proj

# 8 audit + export
& $agx audit report --project $proj --format md --to out\AUDIT_latest_EN.md
& $agx export $proj --format docx --to out\thesis.docx
```

## 6 Housekeeping & cleanup

- **Regenerable artefacts** (safe to delete; rebuilt from sources): `*.docx`,
  `*.pdf`, rendered `figures/`, `*_resolved.md`, `snapshots/`, session
  transcripts. These are gitignored by convention.
- **Do NOT delete** authored sources (the git-tracked `.md`, `specs/`, etc.) on
  the assumption they are "in the DB" unless you have **re-ingested** them and a
  `content checkout` round-trip is byte-identical. The DB only mirrors what you
  last ingested.
- **Database hygiene**:
  ```powershell
  Copy-Item thesis.db thesis.db.bak           # back up before bulk operations
  & $agx content log --limit 20               # inspect recent commits
  & $agx passport validate --project $proj    # structural check of the passport
  & $agx check self                           # structural integrity gate
  ```
- **WAL files** (`thesis.db-wal`, `thesis.db-shm`) are normal; they checkpoint
  into `thesis.db`. Keep them with the DB; both are gitignored.
- **Verify integrity any time**: `agentic audit verify --project $proj` — any
  tampering with a signed commit shows as `invalid`.

## 7 Multi-iteration / new machine

`thesis.db` is portable: copy it (plus your signing-key file) to a new machine,
put the binary on PATH, and continue. `agentic project list` shows the project;
`agentic content checkout` reconstructs the working tree if you only carried the
DB.
