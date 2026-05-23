# agentic — Monolithic Rust CLI for Agentic Thesis Work

> One binary, one SQLite database (`thesis.db`), every agentic-thesis feature.
> Built for FHNW MAS (Digital Leadership in IT / Leadership in Cybersecurity),
> applicable to any institutional MAS or research portfolio.

`agentic` is the runtime behind a Spec-Driven-Development (SDD) thesis: it stores
the working tree, journal, material passport, embeddings, audit trail and
cryptographic signatures of a research project in a single content-addressed
SQLite database, and renders DOCX/PDF deliverables from it. No Python, no MCP,
no servers — shell invocation only, so every AI CLI (Claude Code, Gemini CLI,
OpenAI Codex, Cursor, Factory) drives it identically.

- **What it is** and **how the pieces fit** → [`ARCHITECTURE.md`](ARCHITECTURE.md)
- **Set it up in a new location and run an iteration** → [`QUICKSTART.md`](QUICKSTART.md)
- **Non-repudiation, audit log, APA7 source origins, AI-decision index** → [`AUDIT.md`](AUDIT.md)
- **What changed per tool release / DB schema / iteration** → [`RELEASE_NOTES.md`](RELEASE_NOTES.md) and [`CHANGELOG.md`](CHANGELOG.md)

## Status

- **Version**: 0.1.3 · **DB schema**: v4 · cross-platform (Windows x64, macOS, Linux).
- Functional surface: storage + content store, journal, passport, import,
  embed/classify, checks, DOCX/PDF export, and PQC audit/signing.

## Design principles

- **One binary, one DB.** `thesis.db` is the source of truth; the working tree
  can be reproduced from it (`content checkout`).
- **Append-only history.** Commits, journal and passport are immutable logs.
- **PQC-only crypto (ADR-0039).** Signing is ML-DSA-87 (FIPS 204); no classical
  ciphers (Ed25519/RSA/ECDSA).
- **Verify, don't trust.** Integrity checkers and blocking gates, not vibes.

## Install

```bash
# From source (this repo)
cargo build --release -p agentic           # -> target/release/agentic(.exe)

# Or, once published
cargo install agentic
```

Add `target/release` to PATH, or call the binary by full path. The database
defaults to `./thesis.db` (override with `--db` or `AGENTIC_DB`).

## Command surface

Global options: `--db <PATH>` (default `thesis.db`), `--lang <en|de|fr|it|rm|hi>`,
`--json` (machine-readable output).

| Command | What it does | Example |
|---|---|---|
| `init` | Create a `thesis.db` (wizard, or `--no-wizard` flags) | `agentic init --no-wizard --working-lang en --institution fhnw-mas` |
| `project` | Lifecycle: `list / new / switch / status / archive` | `agentic project list` |
| `journal` | Append-only activity log: `append / show / search` | `agentic journal append --project <ID> --actor me --action-type spec --description "…"` |
| `passport` | Material passport: `append / read / validate / …` | `agentic passport read --project <ID> claim_audit_results` |
| `content` | Content store: `put / put-at / read-at / ls / get / log / ingest / checkout` | `agentic content ls --project <ID>` |
| `import` | Ingest a proposal/draft (`file` / `dir`), md/DOCX/PDF | `agentic import file proposal.docx --project <ID>` |
| `embed` | Embed markdown chapters (vectors in DB) | `agentic embed --project <ID>` |
| `classify` | Classify chapters against thesis-chapter slots | `agentic classify --project <ID>` |
| `check` | Integrity checkers: `self / writing-quality / citations / contamination / …` | `agentic check self` |
| `export` | Render a project to DOCX/PDF | `agentic export <ID> --format docx --to thesis.docx` |
| `audit` | PQC signing + audit reports: `keygen / sign-commits / verify / record / report` | `agentic audit report --project <ID> --format md --to AUDIT.md` |
| `migrate` | Turnkey: new project + ingest a legacy directory | `agentic migrate ./legacy --name "thesis"` |
| `provider` | Provider keys + smoke-test | `agentic provider list` |
| `config` | Key/value config persisted in the DB | `agentic config set <k> <v>` |
| `doctor` | Diagnose environment + detected CLI context | `agentic doctor --json` |

### Content store (the source-of-truth surface)

```bash
# Make the DB an exact mirror of the git-tracked working tree (one commit):
git -c core.quotepath=false ls-files > /tmp/tracked.txt
agentic content ingest --project <ID> --root . --from-list /tmp/tracked.txt --replace \
    --author "Your Name" --message "ingest source of truth"

# Reproduce the working tree from the DB (inverse of ingest):
agentic content checkout --project <ID> --to ./restored

# Inspect:
agentic content ls   --project <ID>             # list tracked paths
agentic content read-at --project <ID> path/to/file.md
agentic content log  --limit 20                 # recent commits
```

### Audit & non-repudiation (PQC)

```bash
agentic audit keygen --signer "Your Name"        # ML-DSA-87 keypair (FIPS 204)
agentic audit sign-commits --project <ID>        # sign every commit
agentic audit verify       --project <ID>        # verify the signed chain
agentic audit report --project <ID> --format md --to AUDIT.md          # whole project
agentic audit report --project <ID> --item C09 --format md --to C09.md  # one item
```

See [`AUDIT.md`](AUDIT.md) for the full model (user actions, APA7 source origins,
AI-decision index, integrity seal).

## Architecture at a glance

```
crates/
├── agentic/              main binary (clap CLI; commands/*.rs dispatchers)
├── agentic-core/         DB + migrations, content store (blob/tree/commit/ref),
│                         project, journal, passport, embeddings, signing, audit
├── agentic-providers/    Anthropic, OpenAI, Google, Mistral, Cohere, Voyage, Ollama, Grok
├── agentic-checks/       integrity checkers (self, citations, contamination, writing_quality)
├── agentic-import/       proposal/draft import (md/DOCX/PDF), recursive classification
├── agentic-export/       DOCX (FHNW template), PDF (Typst), markdown
├── agentic-figures/      figspec JSON → PNG (plotters; Rust port of render_figspec)
├── agentic-tui/          ratatui onboarding wizard
└── agentic-resources/    embedded templates, stylesheets, ADR/schema seeds

skills/
└── book-export/         bookkit engine + build_book driver — turn DB content
                         into professional DOCX books (TOC, index, QR, figures)
```

**Book export** — `skills/book-export/` renders curated content into professional
A4 DOCX books (one book = title + ordered chapter sources): `python
skills/book-export/build_book.py --manifest books.json --src <sources> --tools
<code/tools> --out <dir>`. See [`ARCHITECTURE.md`](ARCHITECTURE.md) §9.

Full diagrams and the data model are in [`ARCHITECTURE.md`](ARCHITECTURE.md).

## License

MIT. See [`LICENSE`](LICENSE).
