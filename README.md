# agentic — Monolithic Rust CLI for Agentic Thesis Work

> Single binary, single SQLite database, every agentic-thesis feature.
> Built for FHNW MAS Digital Leadership in IT / Leadership in Cybersecurity,
> but applicable to any institutional MAS or research portfolio.

## Status

- **Phase**: P0 — workspace scaffolding (in progress)
- **Crates ready**: workspace skeleton, CI config, migration SQL
- **Next**: P1 (storage layer + `init` / `project` / `journal` / `passport` commands)

## Why

- **One binary, one DB.** No Python, no pip, no per-OS divergence.
- **All AI CLIs supported.** Claude Code, Gemini CLI, OpenAI Codex, Cursor, FactoryAI all invoke `agentic` the same way via shell.
- **Quality not optional.** Every feature works fully — claim audit, contamination signals, multilingual export, FHNW template fidelity, embedded high-quality embedding model, Typst-rendered PDFs.
- **Cross-platform.** Windows x64, macOS Intel + Apple Silicon, Linux x64 from a single tag push.
- **No MCP, no IPC, no servers.** Shell invocation only. Monolithic in the strict sense.

## Installation (once shipped)

```bash
# Option 1: GitHub Releases (curl one-liner)
curl -fsSL https://github.com/dcasota/mt-fhnw-agentic/releases/latest/download/install.sh | sh

# Option 2: cargo (from crates.io)
#
# Prerequisite: a working Rust toolchain (rustc + cargo). If you don't
# have one yet, install via rustup:
#
#   Windows (winget):
#     winget install --id Rustlang.Rustup -e
#     # Open a new shell so cargo is on PATH, then:
#     rustup default stable
#
#   macOS / Linux:
#     curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
#
# Then:
cargo install agentic

# Option 3: Scoop (Windows)
scoop bucket add agentic https://github.com/dcasota/mt-fhnw-agentic.git
scoop install agentic

# Option 4: Homebrew (macOS)
brew tap dcasota/agentic
brew install agentic
```

## Quick start

```bash
agentic init                              # launches the M0 onboarding wizard (TUI)
agentic project status                    # where am I?
agentic content edit thesis/ch-02.md      # edit a chapter (lives in thesis.db)
agentic check consolidated                # run all 13 integrity checkers
agentic export thesis --format docx --institution fhnw-mas
```

## Architecture

```
crates/
├── agentic/              # main binary (clap CLI)
├── agentic-core/         # DB schema, blob/tree/commit/ref storage, project + journal + passport
├── agentic-providers/    # Anthropic, OpenAI, Google, Mistral, Cohere, Voyage, Ollama
├── agentic-checks/       # 13 integrity checkers (self, contamination, writing_quality, ...)
├── agentic-import/       # proposal PDF/DOCX import, recursive folder classification
├── agentic-export/       # DOCX (FHNW template), PDF (Typst), PPTX
├── agentic-tui/          # ratatui-based M0 onboarding wizard
└── agentic-resources/    # embedded FHNW templates, Typst stylesheets, ADR/schema seeds
```

## License

MIT. See [`LICENSE`](LICENSE).
