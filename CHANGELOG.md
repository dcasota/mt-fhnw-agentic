# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### P0 — workspace scaffolding (in progress)
- Cargo workspace with eight crates: `agentic`, `agentic-core`, `agentic-providers`, `agentic-checks`, `agentic-import`, `agentic-export`, `agentic-tui`, `agentic-resources`
- SQLite schema covering blobs, trees, commits, refs, projects, passport_entries, journal_entries, audit_rows, audit_verdicts, schemas, protocols, adrs, i18n_strings, api_cache, sprint_contracts, fts
- Content-addressed storage: blobs/trees/commits/refs with SHA-256 hashing
- Project / journal / passport persistence layer
- `agentic` binary with `init`, `project`, `journal`, `passport`, `content`, `doctor` subcommands
- Cross-platform CI workflows (Linux x64, macOS Intel + Apple Silicon, Windows x64)
- Release workflow producing GitHub Releases artefacts + crates.io publish
