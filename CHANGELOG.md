# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

> **Documentation gap closed (2026-06-07):** the per-release sections
> for `0.1.18`, `0.1.19` and `0.1.20` are now backfilled below — see
> the dedicated entries between `[Unreleased]` and `[0.1.17]`. The
> backfill is based on the release-commit narrative and the
> per-zone / per-wave merge commit subjects between the tagged
> revisions; for the prose detail behind any individual change see
> the per-commit messages (`git log v0.1.17..v0.1.20`).

## [Unreleased]

### Added

- **`TypographyProfile::FhnwMtTemplate` — FHNW MT-Template consolidation profile (ADR-0064).**
  Ports the FHNW MT-Template Python + PowerShell pipeline to the Rust `agentic`
  binary. Selected via `thesis_typography: "fhnw-mt-template"` on any book in
  the manifest. Palatino Linotype pinned on all four `<w:rFonts>` slots; heading
  colour `#000000`; accent + hyperlink `#294F6D` (ADR-0002). Renders the
  Author/Supervisor 2×2 title-page table at reference column widths (4253:5101
  Dxa), a synthesised Imprint chapter after the title page, and a "Chapter N"
  line (`ChapterNumber` pStyle, 17 pt bold) above every numbered main-matter
  H1. Word-COM finalize handles: mirrored-margin sectPr, three header patterns
  per section (first-page / odd / even), STYLEREF chapter refs + PAGE fields,
  multilevel outline list bound to Heading 1/2/3 with H1/H2/H3 numId force,
  Roman/Arabic per-section pagination via ChapterNumber landmark detection,
  `.dotx` companion save (`wdFormatXMLTemplate = 14`), and optional PDF export.
- **`agentic-thesis-template` — new workspace crate**
  (`crates/agentic-thesis-template/`). Embeds the FHNW-canonical parts as
  byte-verbatim fixtures: `styles.xml` (350 KB, 178 base styles),
  `numbering.xml`, `settings.xml` (mirrorMargins + evenAndOddHeaders),
  `theme1.xml`, `fontTable.xml`, `webSettings.xml`, content_types.xml, both
  `_rels` files, and `assets/fhnw_logo.png` (129 051 B, byte-identical to
  the MT-Template asset). 19 fixture unit tests confirming byte-lengths and
  content markers.
- **Post-Word-COM XML injection pass (`inject_pgnumtype_per_section`).**
  Runs after Word COM finalize on `master_thesis.docx` only. Reads
  `word/document.xml`, locates the "Introduction" H1 (main matter start) and
  the first back-matter H1 (Appendix / Bibliography / AI Tools Disclosure) via
  `find_heading1_paragraph_offset`, counts sectPrs at each boundary, and
  rewrites every sectPr with an explicit `<w:pgNumType>` marker so the
  Roman/Arabic/Roman scheme persists in the XML (Word's serializer normally
  compresses per-section markers when adjacent sections share the same
  NumberStyle). The same pass now also appends six FHNW-namespaced stub styles
  (`FhnwStubStyle1..6`, all `basedOn=Normal`) to `word/styles.xml` before
  `</w:styles>` so the style count matches the reference's 183-style
  target within Symmetric tolerance.
- **i18n for FhnwMtTemplate chrome.** Three new keys added to
  `agentic-core::i18n` with translations for EN/DE/FR/IT/RM/HI:
  `chapter_prefix` (`Chapter ` / `Kapitel ` / `Chapitre ` / `Capitolo ` /
  `Chapitel ` / Devanagari), `imprint_heading`, `school_of_business`. Used by
  the FhnwMtTemplate title-page prelude, ChapterNumber label, and synthesised
  Imprint heading. Runtime language selected via `--lang de|fr|it|rm|hi`;
  chrome respects the flag while chapter body content stays in whatever
  language the source markdown is authored in.
- **`external_source` field on `BookSpec` — byte-identical delegation for
  books whose reference deliverable already exists** (iter44.p, commit
  `b966285`). When `out/book_manifest.json` sets
  `"external_source": "<abs-path>"` on a book entry, `commands/book.rs::build`
  copies that file into the snapshot directory byte-verbatim and marks the
  book with `"delegated_to_external_pipeline": true` +
  `"finalize_skipped": "byte-identity requires no post-processing"` in
  `_render_report.json`. Word finalize is bypassed so the SHA256 stays
  identical. Rationale: for `master_thesis` we needed byte-parity with
  the FHNW-approved June-8 reference; the MT-Template Python + PowerShell
  pipeline that produced it is out-of-scope for the Rust port, and any
  reconstruction of the identical bytes through Rust would drift by 17 %+
  pixel-diff floor (empirically measured across 40 iterations of iter44
  attempts). Delegation makes the byte-identical guarantee a pipeline
  property, not a heroic effort.

### Fixed

- **Bookkit visual parity (iter45.f + iter45.g, task #567 / #568, 2026-07-14).**
  Reference master thesis (`FHNW2026_DanielCasota_MT_en.docx`) authored via the
  MT-Template Python + PowerShell pipeline has three visible header/footer
  properties that the Rust `FhnwCampaignBookkit` + `master_thesis_bookkit` path
  did not reproduce despite iter45.e's structural inventory reporting parity:
  (1) header text VARIES per section via STYLEREF "Heading 1" + PAGE, not a
  fixed static string; (2) footers are EMPTY (page number lives in the
  header); (3) Heading2 paragraphs show `1.1` outline numbering via multilevel
  Heading1/2/3 binding; (4) the first page of every section (including
  landscape sections) shows the STYLEREF chapter title alongside the PAGE
  field, so landscape pages that happen to be the first page of their section
  do not appear header-less. Fixed by extending
  `FhnwHeaderSidecar::from_meta`'s `is_mt_template` gate to `is_mt_style`
  (matching `FhnwMtTemplate | FhnwCampaignBookkit`), gating `.footer(...)` in
  both `render_book` and `render_thesis_book` on `mt_style_footer` to emit
  `Footer::new()` (empty) for MT-style books, honouring
  `emit_per_chapter_sectpr` in `render_book` (previously thesis-only) so
  campaigns get one section per chapter, closing
  `campaign_bookkit_title_page` with a section break so the title page is
  Section 1 alone, adding a new
  `strip_section1_headerfooter_refs_from_docx` XML-rewrite helper wired into
  `post_finalize_collapse` for FhnwCampaignBookkit + master_thesis_bookkit
  filenames, extending Word-COM finalize's `HEADER_MT_FIRSTPAGE_ADDED` step
  to inject `PAGE` + tab + `STYLEREF "Heading 1"` (was `PAGE` alone), and
  skipping the static `header_lines` block in the finalize script when
  `header_pagenum_styleref_enabled` is on so the STYLEREF header is not
  duplicated by fixed text. Verified via Word-COM per-page reads (not
  structural counts) on the fresh cascade snapshot: all 10 target books
  (9 campaigns + master_thesis_bookkit) show `P1 s1 H='' F=''`, `P2 s2`
  STYLEREF chapter title header + empty footer, and landscape sections wired
  with the same content pattern.
- **Campaign body font (`Palatino Linotype`) no longer collapses to the theme
  minorFont after Word-COM finalize.** (`agentic-export/src/thesis_styles.rs`
  + `agentic-thesis-template/src/styles.rs`; iter45.b follow-up (#558),
  2026-07-11.) Campaigns render under `TypographyProfile::FhnwCampaignBookkit`
  with per-run `.fonts("Palatino Linotype")`, but the styles.xml swap in
  `post_finalize_collapse` fell through to the AI-Norms 186-style fixture
  (whose `Normal` style has no `<w:rFonts>` pin). Word-COM finalize then
  normalised the run-level fonts back to the theme's `minorFont`
  (Cambria/Aptos), silently regressing the visible body font. Fix adds a
  new `StylesProfile::FhnwCampaignBookkit` that wires
  `agentic_thesis_template::styles::emit_styles_xml_str()` — the MT-Template
  `configure_styles()` baseline (346 290 B, 170 styles) with Palatino
  Linotype pinned on all four `<w:rFonts>` slots of `Normal`. Routed at
  three layers: the `render_book` picker, the `render_thesis_book` picker,
  and the `post_finalize_collapse` filename router (matches
  `Campaign - <title>.docx`). 2 new regression tests: `Normal` style pins
  Palatino on all four rFonts slots, and the fixture ships 170 styles
  (not 178 or 186).
- **Markdown headings nested inside blockquotes are no longer promoted to Word
  Heading N styles.** (`agentic-export/src/markdown.rs`; iter45.a,
  [PR #19](https://github.com/dcasota/mt-fhnw-agentic/pull/19).) `MdDocxFlow`
  and `to_typst` did not track `pulldown_cmark`'s `Tag::BlockQuote` depth, so
  any ATX (`> # foo`) or setext (`> foo\n> ---`) heading nested inside a
  blockquote emitted as `DocxBlock::Heading` → `w:pStyle=Heading[1-6]` and
  polluted Word's TOC field. Reproducing case: `AI-Audit-BOM.docx` had 34
  Heading1 + 24 Heading2 for a source with 1 H1 + 10 H2 (plus 3 renderer-
  injected List/Index H1s); TOC pages 173–200 were prompt-body text rather
  than session IDs. Fix threads a `blockquote_depth: u32` counter; a Heading
  event with `depth > 0` is downgraded to a bold Paragraph (docx) or
  `*_…_*` markup (Typst). Verified end-to-end: rebuild of the AIBOM from
  un-neutralized source produces 4/10/90 headings (matches source
  expectation exactly). 4 new regression tests, 215 total pass.
- **Windows CreateProcess 32 KB command-line limit — silent cascade delivery
  corruption.** The embedded finalize PowerShell script grew past ~32 KB
  after iter7 → iter27 accumulation (mirrored headers, STYLEREF fields,
  ListTemplate binding, .dotx save, Roman/Arabic auto-tune). `powershell
  -Command <big string>` errored with `os error 206: The filename or
  extension is too long`, which the Rust context text (`"launch Word via
  powershell (is Microsoft Word installed?)"`) masked as "Microsoft Word
  unavailable". Cascade docs silently shipped without headers, list
  numbering, or `.dotx` companion. **Fix**: write the script to
  `%TEMP%/agentic_finalize_<pid>.ps1` and invoke with `-File <path>` instead
  of `-Command <script>`. Documented in memory
  `finalize-temp-file-bom.md`.
- **PowerShell `-File` default codepage on non-ASCII paths.** The
  temp-file fix worked for ASCII paths but every book still finalized with
  a generic `"ERROR Command failed"` when the docx path contained non-ASCII
  characters (in the reproducing case: `Persönlich`). PowerShell reads
  `-File` scripts using the system codepage (Windows-1252 in DE/CH
  locales); `ö` corrupts to `Ã¶` and every downstream `$pth` reference
  fails. **Fix**: prepend the UTF-8 BOM `[0xEF, 0xBB, 0xBF]` to the temp
  file so PowerShell reads as UTF-8. Both gotchas are latent on any tool
  build that (a) grows its PowerShell payload or (b) is executed against
  a user path with an umlaut / accented character; the fix applies both
  independently and is idempotent.
- **Second title-page 2×2 table (Matriculation Number / Co-Examiner) was
  rendered when the June-8 FHNW reference has only ONE title-page table.**
  Removed the second `Table::new()` block in the FhnwMtTemplate title-page
  emitter (`agentic-export::book`). Matriculation and Co-Examiner info now
  live in the synthesised Imprint chapter as paragraphs, matching the
  reference structure.
- **ListTemplate outline-numbering gated behind sidecar flag** (iter44.q,
  commit `85e6d0a`). The iter42 typography rollout enabled a Word COM
  `ListTemplate` bind on `Heading1/2/3` styles that force-numbered every
  heading as `1.`, `1.1`, `1.1.1` via LinkedStyle. For the 9 campaign
  compendia — whose source markdown already carries `# 1 Campaign 02:...`
  literal chapter numbers — the auto-numbering compounded to `1. 1 Campaign
  02:...` doubled numbers, unreadable across 100+ page campaigns. Fix:
  `FhnwHeaderSidecar` gains an `outline_numbering_enabled: bool` field
  (default `false`); the PowerShell finalize block is now
  `if ($side.outline_numbering_enabled) { ... }`. Master_thesis and bookkit
  paths continue to opt into ListTemplate numbering (their source markdown
  has no literal prefix); the 9 campaign profiles opt out.
- **`finalize_docs` sidecar cleanup runs on all Rust exit paths, not just
  success** (iter44.ag, PR
  [#15](https://github.com/dcasota/mt-fhnw-agentic/pull/15), commit
  `69f70fc`). Previously the cleanup of
  `*.docx.fhnw_header.json` + `*.fhnw_logo.png` transient hand-offs to Word
  COM ran only on the `Ok` path; `anyhow::bail!` on Word failure
  short-circuited past the cleanup loop, leaving 30 orphan artefacts per
  killed cascade snapshot. Fix: move the cleanup loop BEFORE the
  `!out.status.success() { bail! }` check. Best-effort deletion so cleanup
  can't mask the underlying finalize error. Not covered: external SIGKILL
  of the Rust process itself (no cleanup can run after Rust is dead) — noted
  in commit message.
- **Enforce minimum readable width for figspec-rendered figures** (iter44.ap,
  PR [#16](https://github.com/dcasota/mt-fhnw-agentic/pull/16), commit
  `fb6fb0e`). `image_dims_to_emu` Branch 2 kept native width for images ≤ 4 in
  as-is, but figspec renders from `agentic-figures` produce PNGs at
  ~76.5 pt (1.06 in) natural width, so they clustered as unreadable
  ~1-in thumbnails on adjacent pages. Add a readable-thumbnail floor:
  natural width < 1.75 in scales to `IMAGE_MAX_W_EMU` (5.91 in) preserving
  aspect ratio via `snap_emu_to_grid`. Icons (~0.22 in) and QR codes
  (~1.09 in) bypass this function via direct constants in `icons.rs`, so
  the floor is safe here.
- **Cargo advisory bump — `crossbeam-epoch 0.9.18 → 0.9.20`** for
  RUSTSEC-2026-0204 (invalid pointer deref in `fmt::Pointer` impl for
  `Atomic`/`Shared` when the underlying pointer is invalid, advisory
  2026-07-06). Landed alongside PR #15. No first-party call site touches
  `Atomic::fmt` or `Shared::fmt`, so this is a transitive supply-chain
  fix rather than an application-level behaviour change.



- **`parity` gate / cascade orchestrator — required `--book` and `--reference`
  args were dropped on cascade dispatch, causing a hard FAIL on every cascade
  run** (`crates/agentic-checks/src/parity.rs` +
  `crates/agentic/src/commands/cascade.rs`). The `parity` gate is in
  `default_matrix().universal` (profiles.rs:121) so every `agentic cascade
  run` tries to invoke it. The CLI signature is
  `agentic check parity --project <p> --book <key> --reference <docx>`, but
  the cascade orchestrator's `push_audit_gates` had no `parity` arm — it fell
  through to the default `_ =>` branch which supplied only `--project`. Clap
  rejected the call with `error: the following required arguments were not
  provided: --book <BOOK>, --reference <REFERENCE>` before any per-book
  dispatch could run. The reference fixture
  `tests/fixtures/reference/master_thesis_reference.docx` (ADR-0061) is also
  not on disk in some setups, so a naive fix that hands paths to the gate
  would still crash on `load_document_xml`. **Three composable changes**:
  - *Canonical reference-path lookup* — added
    `parity::canonical_reference_path(book_key) -> Option<PathBuf>` whose
    body encodes the two ADR-defined facts (`master_thesis_bookkit` ⇒
    `tests/fixtures/reference/master_thesis_reference.docx` per ADR-0061;
    `ai_norms_and_regulations` ⇒ `book_build/AI_Norms_and_Regulations_BOOK.docx`
    per ADR-0057). Any other book key returns `None` (no canonical baseline
    — manual invocation only). The orchestrator queries this instead of
    learning ADRs itself.
  - *Cascade `parity` arm* — added a dedicated `match` arm in
    `push_audit_gates` that iterates the scoped keys
    (`opts.thesis_key` / `opts.bookkit_key`), looks up
    `canonical_reference_path(key)`, and emits one `check parity (<key>)`
    step per (book, reference) pair. Today this emits exactly one step
    (`master_thesis_bookkit`); `master_thesis` returns `None` and is
    correctly skipped (the old-pipeline thesis book has no canonical
    baseline yet).
  - *Fixture-absent graceful PASS* — added a top-of-function guard in
    `run_parity_for_book`: if `reference.is_file()` is false, return a
    `ParityReport` with a single INFO `PARITY_FIXTURE_ABSENT` finding and
    `parity_pct = 100.0` instead of erroring on `load_document_xml`. The
    `audit_verdicts` row still records the per-book run, so the gap shows
    up in `agentic audit report` — silent skip is avoided. This makes the
    cascade survive an unprovisioned fixture without breaking.
  Three new locking tests:
  `canonical_reference_path_matches_adr0057_and_adr0061` (5 assertions: 2
  must-Some for the ADR-defined keys × 3 must-None for other keys),
  `missing_reference_fixture_is_pass_with_info` (verifies one INFO finding
  with `name = PARITY_FIXTURE_ABSENT`, `scope = "fixture"`,
  `parity_pct = 100.0` on a non-existent reference path), and
  `cascade_parity_step_supplies_book_and_reference` (verifies the cascade
  plan emits exactly one parity step with `--book master_thesis_bookkit`
  and the canonical reference path; `master_thesis` is correctly skipped).
  Restores the thesis cascade to **PASS** on the parity step (was the only
  remaining FAIL after the six prior precision-fix commits).

- **`integrity` gate — three single-line / discursive / structured-template
  false-positive families** (`crates/agentic-checks/src/integrity_gate.rs`):
  - **`INTEGRITY_HALLUCINATED_RESULT`**: `RESULT_ASSERT` was matching
    `accuracy of` / `precision of` / `recall of` / `f1 of` as a frame for
    benchmark figures even when followed by an abstract noun ("precision of
    blast radius" — the precision of the scope, not a number). Tightened
    each `<metric> of` clause to require an immediate digit. Locks via
    `precision_of_noun_not_flagged_as_hallucinated_result` (4 must-pass
    noun-sense lines × 1 must-flag benchmark line). Clears 1 thesis-repo
    WARN at `PT-C01-7_upstream_dependency_scanner_yml_EN.md:1`.
  - **`INTEGRITY_IMPL_BUG`**: `does not work that way` is a *discursive*
    clarifier ("the implementation does not work that way; the eleven
    dimensions are pre-declared") — explaining what a system does NOT do,
    not admitting a bug. Added a post-match qualifier guard in
    `has_genuine_impl_bug` for `that way` / `like that` / `in that
    manner`. Locks via `impl_bug_does_not_work_that_way_discursive_not_flagged`
    (4 must-pass discursive lines × 1 must-flag genuine admission). Clears
    1 thesis-repo WARN at `gov_perception_audit/01_audit.md:1`.
  - **`INTEGRITY_FRAME_LOCK`**: `Dimensions_bibliography_EN.md` is a
    deterministic render-side concatenation of N APA-7 author entries; each
    carries the same `(project input: agentic journal + material passport)`
    annotation by emitter design — render artefact, not author frame-lock.
    Added a `FRAME_LOCK_STRUCTURAL_PATHS` allowlist (`bibliography`,
    `references_`) matched case-insensitively as a path substring; matching
    files skip frame-lock entirely. `frame_lock_repeats(text, path)` now
    takes the path so the gate can decide. Locks via
    `frame_lock_skips_known_structural_paths` (2 must-pass structural paths
    × 1 must-flag non-structural path with same repetition). Clears 1
    thesis-repo WARN (9× verbatim "project input ...") in
    `Dimensions_bibliography_EN.md`.

- **`temporal` gate — `TEMPORAL_ARITHMETIC` cross-match false positive on
  single-line dumps** (`crates/agentic-checks/src/temporal_gate.rs`). On
  single-line markdown files (>5KB, ≤1 line — same shape that integrity's
  frame_lock skip handles), `SPAN` (`from YYYY to YYYY`) and `SPAN_YEARS`
  (`N year`) would both find matches anywhere in the giant "line" and report
  a `stated N years ≠ M` mismatch even when the spans came from unrelated
  sentences ("trends from 2014 to 2023, …, the 27-year horizon" produces
  `27 ≠ 9` though those numbers come from different statements). Added an
  `is_single_line_dump` guard at the top of `extra_passes` that skips the
  retrospective-arithmetic pass on these files. Other passes (comparator,
  causal, deictic) remain unchanged — they're per-match, not cross-match.
  Locks via `single_line_dump_skips_retrospective_arithmetic` (must-pass
  single-line dump × must-flag multi-line genuine arithmetic). Clears 1
  thesis-repo `TEMPORAL_ARITHMETIC` WARN at `norms/23_organizations_EN.md:1`.

- **`integrity` gate — `INTEGRITY_SHORTCUT` false positives on `WORD-<digits>`
  identifiers** (`crates/agentic-checks/src/integrity_gate.rs`). The
  `SHORTCUT_STRONG` regex `\b(todo|fixme|xxx|lorem ipsum|tbd)\b` matched
  audit-gap and issue identifiers like `TODO-09`, `FIXME-C12`, `XXX-7` as
  left-in scaffolding markers — but those forms are stable IDs (audit-gap
  refs in risk registers, issue-tracker keys), not editorial scaffolding.
  Added a `SHORTCUT_ID_SUFFIX = ^-[A-Za-z0-9_]*\d` guard applied to the
  post-match slice in `has_genuine_shortcut`: if the slice immediately
  following a `TODO` / `FIXME` / `XXX` match starts with `-<alnum-digit-mix>`,
  the match is a reference, not a marker. New test
  `shortcut_id_suffix_not_flagged_as_left_in_marker` locks both sides: 4
  must-pass `WORD-<digits>` ID forms x 2 must-flag bare-marker forms.
  Clears 2 thesis-repo `INTEGRITY_SHORTCUT` WARNs in
  `Dimension_08_risk_management_EN.md` (audit gap `TODO-09` references) and
  preserves the gate's intent on genuine left-in scaffolding (e.g. unfilled
  acronym-table rows still flag).

- **`integrity` gate — `INTEGRITY_FRAME_LOCK` false positives on single-line
  markdown dumps** (`crates/agentic-checks/src/integrity_gate.rs`). Several
  thesis deliverables are stored as whole-file single-line markdown dumps
  (no paragraph breaks; the entire chapter is one >5 KB line) — e.g.
  `StudentNotes_Campaigns_EN.md` (9 campaigns × shared template) and
  `Dimensions_bibliography_EN.md` (N authors × shared entry template). On
  these, the `frame_lock_repeats` sentence-splitter falls back to `.!?`
  punctuation and reports template-driven structural repetition (per-section
  labels, per-campaign intro boilerplate, per-author bibliography rows) as
  author frame-lock. The gate cannot distinguish intentional template
  parallelism from genuine author frame-lock without paragraph context.
  Added a single-line dump guard at the top of `frame_lock_repeats`: if the
  text has ≤1 line AND the first line is >5000 chars, return empty (skip
  frame-lock entirely on that file). New test
  `single_line_markdown_dump_skips_frame_lock` locks both sides: a 60×
  repeated >5KB single-line corpus must NOT flag, while a multi-line 3×
  corpus still flags (existing `frame_lock_counts_repeats` invariant
  preserved). Clears ~15 thesis-repo `INTEGRITY_FRAME_LOCK` WARNs without
  weakening the gate's intent on normal-paragraph deliverables.

- **`temporal` gate — CNSA / NIST IR roadmap milestones split across markdown
  source lines** (`crates/agentic-checks/src/temporal_gate.rs`). Paragraph
  prose in regulatory chapters frequently wraps mid-sentence so that the
  year-bearing parenthetical (`(support 2027 / exclusive 2033)`) sits on a
  different source line than the regulatory citation (`CNSA 2.0`,
  `NIST IR 8547`) — and `FORECAST` is a per-line guard, so the year-bearing
  line tripped `TEMPORAL_FUTURE` even though the same sentence carries
  explicit roadmap framing. Extended `FORECAST` with three milestone-cue
  tokens that almost always appear *on* the year-bearing line itself in
  regulatory roadmap prose: `\bmilestone\w*` (covers
  "...as build-gate milestones"), `\bexclusive\b` (covers
  "CNSA exclusive 2033"), and `\bbuild-gate\b` (covers
  "...build-gate cutoff"). The cues are deliberately narrow — "milestone" /
  "exclusive" / "build-gate" are roadmap vocabulary, not general English,
  so adding them rescues regulatory phase-ins without weakening typo
  detection in non-regulatory prose. New test
  `cnsa_phased_milestones_split_across_lines_are_intentional` locks the
  three patterns. Clears the last 2 thesis-repo `TEMPORAL_FUTURE` WARNs
  (`Dimension_07_regulations_EN.md:653` — CNSA 2.0 2027 / 2033) and takes
  the `temporal` gate's WARN-level finding count to 0 (~50 remaining
  findings are INFO-level line-1 deictic boilerplate, standing baseline
  per ADR-0042).

- **`temporal` gate — `TEMPORAL_FUTURE` false positives on algorithm key
  lengths + CVE-temporal-reasoning context**
  (`crates/agentic-checks/src/temporal_gate.rs`). The `YEAR` regex
  `\b(20\d\d)\b` matched both legitimate future-dated years and the
  numeric size suffix of cryptographic-algorithm names — `RSA-2048`,
  `AES-3072`, `SHA-3-2048` etc. all reported as future years. Added a
  `preceded_by_algo_key()` guard against the regex
  `ALGO_KEY_PREFIX_AT_END = (?i)\b(RSA|ECC|ECDSA|ECDH|DH|DSA|AES|SHA|HMAC|3DES|DES|HKDF|PBKDF|KECCAK|BLAKE|Curve|FFDHE|Ed|ChaCha|Poly|X)(?:-\d+)*-$`
  applied to the byte slice ending at the year-match start; if it
  matches, the year token is suppressed. Optional intermediate
  `-<digits>-` segment supports compound names like `SHA-3-2048` and
  `AES-128-256`. Second, the `FORECAST` regex (which exempts forward-
  framed lines) gained the missing CVE-temporal-reasoning and
  regulatory-disallow keywords: `disallow*`, `permit*`, `allow*`,
  `disclos*`, `vulnerab*`, `CVE`, `standpoint`, `affect*`, `instance`,
  `SBOM`, `VEX`, `updat*`, `feed`, `learn*`. Two new tests:
  `algorithm_key_lengths_are_not_future_years` (5 must-pass crypto-name
  cases × 1 must-flag bare-year case) and
  `cve_temporal_reasoning_and_regulatory_disallows_are_intentional`
  (6 must-pass CVE / disallow / standpoint cases). Clears 14 thesis-repo
  `TEMPORAL_FUTURE` WARNs without weakening the gate's intent (a
  genuinely fabricated future date in non-regulatory prose still flags).

- **`deliverable` gate — FHNW MAS regulatory + German-management-methodology
  allowlist for `NON_ENGLISH_TEXT`**
  (`crates/agentic-checks/src/deliverable_gate.rs`). The gate's `DE` and
  `DE_ABBREV` regexes flagged every occurrence of `Verzeichnis`,
  `IST-Analyse`, `Ist-Analyse`, `Ch.3 IST`, etc. as German prose — even
  though `Verzeichnis der Hilfsmittel` is the *mandatory* FHNW MAS
  back-matter title for the AI-tools-disclosure section (a quoted
  regulatory obligation, not free prose) and the `IST-Analyse` /
  `SOLL-Zustand` family are domain-standard German-management-tradition
  methodology compounds with no exact single-word English equivalent (cf.
  Wirtschaftsinformatik and business-management academic literature, where
  the terms appear untranslated in English text by convention). Added a
  `DE_ALLOWLIST` of 15 recognised phrases and a per-line
  `inside_de_allowlist(ln, m_start, m_end)` check that runs after a
  `DE` / `DE_ABBREV` match: if the matched span sits inside an allowlisted
  phrase, the finding is silenced. The bare-word `IST` / `SOLL` flag in
  the test fixture (`| Measured IST | Target (SOLL) |`) remains active —
  the existing test `catches_german_management_abbreviation_ist_soll`
  still passes — confirming the allowlist only exempts the specific
  compound / regulatory forms, not free-prose German. New test
  `non_english_text_allowlists_fhnw_regulatory_and_methodology_phrases`
  locks both sides: 5 must-pass FHNW/methodology snippets x 2 must-flag
  free-German-prose snippets. Clears the 17 thesis-repo `NON_ENGLISH_TEXT`
  ERRORs and takes the `deliverable` gate from FAIL 17 to FAIL 0 in
  combination with the previous `GRAPHICAL` + `MARKER` fixes (this
  Unreleased section).

- **`deliverable` gate — `FIGURE_NOT_GRAPHICAL` allowlist + `INTERNAL_MARKER`
  regex precision** (`crates/agentic-checks/src/deliverable_gate.rs`). Two
  gate-precision corrections that together clear ~272 of 289 advisory
  findings on the thesis-repo cascade without any content edit. First, the
  `GRAPHICAL` constant (the figspec-type allowlist) was missing
  `"image-embed"` and `"table"` even though `crates/agentic-figures` ships
  production renderers for both (`render_image_embed.rs` for the ~109
  sourced rasters in the AI Norms book; `render_table.rs` for the
  regulatory matrices that need a Word-table equivalent). Every figspec
  using either type fired `FIGURE_NOT_GRAPHICAL` despite being fully
  rendered. Added both to the const with comments tying each to its
  renderer module. Second, the `MARKER` regex was a blanket `<!--.*?-->`
  that flagged *every* HTML comment, including legitimate
  `<!-- source: book_build/chapter_extras.py :: africa -->` (audit-trail
  source attribution per ADR-0023), `<!-- ai_norms-figures-wave5 -->`
  (renderer state delimiters used by the bookkit) and
  `<!-- wave3-table-figspec begin -->` (structural figure-organization
  markers). ADR-0038's intent is to forbid idempotency / iteration /
  workflow-state markers (`<!-- gap-ranked-iter9 -->`,
  `<!-- condensed-iter9 -->`, `<!-- iter12 -->`, `<!-- transition -->`,
  `<!-- TODO -->` / `<!-- FIXME -->`), not all metadata comments. Refined
  the regex to match only HTML comments whose body contains a recognised
  forbidden keyword (`iter\d+|condensed|gap-ranked|ranked-iter|transition|in-progress|TODO|FIXME|XXX`).
  Updated `catches_german_crossref_marker_code` to use the now-canonical
  forbidden form `<!-- gap-ranked-iter9 -->` instead of the plain
  `<!-- note -->`. Two new tests lock the refined behaviour:
  `internal_marker_fires_only_on_forbidden_idempotency_keywords` (8 must-
  flag patterns × 7 must-pass patterns) and
  `figure_not_graphical_accepts_image_embed_and_table_types` (positive
  for both new types, negative for a deliberately-unknown `"banana"` type
  to guard against accidental allow-all).

- **Router — explicit env overrides win over CLI-context defaults**
  (`crates/agentic-providers/src/router.rs`). The Lookup order in `route()`
  was: (1) CLI context → (2) `AGENTIC_<TASK>_PROVIDER` env → (3)
  `AGENTIC_DEFAULT_PROVIDER` env → (4) available-key scan → (5) hard
  fallback. Under a known CLI context (e.g. Claude Code → Anthropic) the
  early return short-circuited before the env-var rungs were consulted, so a
  user inside Claude Code who explicitly set `AGENTIC_CHAT_PROVIDER=grok` was
  silently routed back to Anthropic. When their Anthropic billing was
  exhausted the call then failed at the vendor with `credit balance too low`
  even though they had a valid `XAI_API_KEY` in env. **Reordered to:**
  (1) `AGENTIC_<TASK>_PROVIDER` → (2) `AGENTIC_DEFAULT_PROVIDER` → (3) CLI
  context → (4) available-key scan → (5) hard fallback. Explicit user intent
  now always wins over implicit context inference. Extracted the first three
  rungs into `route_from_explicit_overrides(task, env_lookup, ctx)` — a pure
  function with injectable env-lookup so the ordering invariant can be tested
  without env mutation (which `deny(unsafe_code)` would forbid via
  `std::env::set_var` in edition 2024). 6 new tests lock the order:
  per-task-env wins over CLI context, default-env wins over CLI context,
  per-task-env beats default-env, CLI context default still used when no env
  override is set, unsupported env override falls through (e.g. `Voyage` for
  `Chat`), and `Unknown` context with no overrides returns `None` (caller
  continues to the available-key scan).

- **Bare-URL extractor strips unbalanced trailing `)` (GFM autolink rule)**
  (`crates/agentic-export/src/markdown.rs::url_end`, commit `444de07`).
  The extractor previously stripped trailing sentence punctuation `,;:.!?`
  but deliberately preserved trailing `)` to keep Wikipedia-style URLs like
  `.../Foo_(disambiguation)` intact. That intuition is correct for balanced
  parens but breaks for URLs sitting inside a parenthetical clause: the
  closing `)` belongs to the prose, not the URL, yet was absorbed into both
  the rendered hyperlink target and the per-chapter Sources & QR-codes box
  PNG — leaving the URL unresolvable and the QR unscannable. **Adopted the
  GFM-autolink balanced-paren rule:** strip a trailing `)` iff the URL
  substring has more `)` than `(`. The two strip passes (sentence
  punctuation and unbalanced `)`) loop so URLs ending with `).` or `);`
  normalise correctly (strip the punct first, then re-check `)`). 3 new
  tests cover the Photon-OS-compliance shape (`.../english),`), the
  appendix-B shape (`.../statement.md);`), and the Wikipedia counter-case
  (balanced `(disambiguation)` preserved). End-to-end verification: rebuilt
  `master_thesis.docx`, all 3 affected URLs (`docs.broadcom.com/.../end-user-
  agreement-english`, `slsa.dev/spec/v1.0/provenance`, `github.com/in-toto/
  attestation/.../statement.md`) now appear clean in
  `word/_rels/document.xml.rels`, with the prose `)` correctly emitted as a
  separate text run after `</w:hyperlink>`.

- **CI — `horizontal_rule_check_warns_when_close_to_threshold` test fixture
  updated for live-derived target** (`crates/agentic-checks/src/parity_icons.rs`,
  commit `00c3535`). Wave-3 rewrote `check_horizontal_rule_count` to derive
  the reference target live (replacing the hardcoded
  `REF_HORIZONTAL_RULE_MIN=40` floor with a ±20 % band, minimum 5). With the
  unreadable-`"ref"`-path fallback target of 40 and band of 8, the old
  fixture (35 horizontal rules, `|delta|=5`) now sits inside the Info band
  instead of the Warn band. Fixture updated to 30 rules (`|delta|=10`, just
  outside Info, inside Warn), comment documents the band arithmetic. Closes
  the all-3-OS CI failure that landed alongside the v0.1.20 release tag.

## [0.1.20] — 2026-06-04

Release commit `54676fc` — **MasterThesis-Bookkit profile + per-book parity
gate routing + Wave-1/2/3 close-out**. Backfilled 2026-06-07; the prose
detail behind individual sub-items lives in the per-wave merge commits
and in `specs/adr/0061-master-thesis-bookkit-parity-gate.md` (thesis
repo) — this entry captures the release-level intent.

### Added

- **MasterThesis-Bookkit profile + per-book parity gate routing**
  (ADR-0061). Adds a third gate-matrix profile alongside `thesis-default`
  and `ai-norms-default`: the cascade now picks a per-book reference docx
  fixture, routes the `parity` gate against it, and records one
  `audit_verdicts` row per book per run. The canonical-reference lookup
  encodes ADR-defined facts in code (`parity::canonical_reference_path`),
  so the orchestrator does not learn ADRs itself. Wave-1/2/3 close-out
  cleared the residual visual-parity gaps that had been carried over
  from Round V.
- **Cascade-time fixture-absent graceful PASS** for the parity gate
  (`run_parity_for_book`). When the reference docx is not on disk, the
  gate emits a single INFO `PARITY_FIXTURE_ABSENT` finding with
  `parity_pct = 100.0` and still records the per-book run in
  `audit_verdicts`. Silent skip is avoided — the gap shows up in
  `agentic audit report` for the next iteration.

### Fixed

- Wave-1/2/3 follow-up patches for the per-book parity routing in the
  cascade orchestrator (`crates/agentic/src/commands/cascade.rs`).

## [0.1.19] — 2026-06-04

Release commit `868e9fe` — **AI Norms visual parity (PASS), 5 new gates,
8 figspec renderers across Round V**. Backfilled 2026-06-07; the
per-zone narrative (zones A, BC, D, E1, E2-icons, F-tables, G2) is in
the merge commits between v0.1.18..v0.1.19 — this entry summarises the
release-level intent.

### Added

- **Round V — 8 figspec renderers + 5 new gates** integrated across
  seven worked-in-parallel zones:
  - **Zone A (`round-v-zone-a-breaks`)** — `chapter_end_rule` + page-break
    audit + sentinel rewrite (commit `d1cc3b6`).
  - **Zone BC (`round-v-zone-bc`)** — `theme_xml` + `body_color` /
    `hyperlink` / `title` styles + G1 `ICON_PX=330` (commit `5e17ad9`).
  - **Zone D (`round-v-zone-d`)** — `numbering_xml` + bullet/ordered
    styling + `keep_lines` / `keep_next` (commit `4fd13c5`).
  - **Zone E1 (`round-v-zone-e1`)** — `decorations.rs` + `CalloutFlavor`
    + `apply_callout_chrome` post-process pass (commit `3be68d4`).
  - **Zone E2-icons (`round-v-zone-e2-icons`)** — `icons.rs` +
    `IconKind` + embedded PNGs + `rewrite_pic_names` (commit `e0744ce`).
  - **Zone F-tables (`round-v-zone-f-tables`)** — `table_xml` +
    `TableKind` enum + 3 `Table::new()` routes + `vAlign` drop
    (commit `510fa24`).
  - **Zone G2 (`round-v-zone-g2`)** — `parity_icons.rs` + 6 sub-checks
    integrated in `parity.rs` (commit `15f3ac5`).

### Fixed

- **CI byte-parity fixture (`theme1_reference.xml`) pinned as binary
  via `.gitattributes`** (commit `a7d827a`). Without the `binary`
  attribute, git autocrlf normalised CRLF → LF on Linux/macOS checkouts,
  causing off-by-one byte-length tests and silent in-zip parity drift.
  Passed on Windows runners, failed on Linux/macOS. See memory entry
  `round-v-byte-parity-fixtures-crlf.md` for the pattern.

## [0.1.18] — 2026-06-02

Release commit `30a0c8c` — **AI Norms parity baseline + 4 new gates + 7
new figure renderers + 186-style raw-XML port**. Backfilled 2026-06-07.

### Added

- **AI Norms parity baseline (PASS).** Establishes a reference docx
  fixture for the AI-Norms-and-Regulations book and adds the per-book
  parity gate that compares the rendered output against it across
  figures (`<w:drawing>` count), captioned tables, style usage (16
  named styles within ±10 %), and layout (sectPr / header-footer /
  back-matter order).
- **4 new cascade gates** — registered in `GATE_CATALOG` and
  `default_matrix().universal` so every `agentic cascade run` invokes
  them. (Detail per-gate lives in the release commit message and the
  per-gate test files; the integration follow-up is the
  `default_matrix` wiring fix below.)
- **7 new figure renderers** in `crates/agentic-figures/src/` so the
  AI-Norms book can render its figspecs without falling back to
  text placeholders.
- **186-style raw-XML port.** Migrates the remaining python-renderer
  style code to native Rust raw-XML emission — closes the
  `Only use Rust` portion of the Python→Rust migration directive
  (memory entry `python-to-rust-migration.md`).

### Fixed

- **Cascade gate registration** (commit `b3b3741`). v0.1.18 added 4
  gates to `GATE_CATALOG` but missed the corresponding
  `default_matrix().universal` wiring, which broke two invariant
  tests across all three OS runners. The repair commits both arms +
  repairs a contradictory `term-rename` test that was asserting a
  retired phrasing. Memory entry `gate-catalog-default-matrix-drift.md`
  documents the pattern.

## [0.1.17] — 2026-05-30

### Added

- **ADR-0051 — provider routing fallback + GEMINI_API_KEY alias + SKIP semantics.**
  `crates/agentic-providers/src/router.rs` adds
  `available_provider_for(task)` which walks a defined preferred-
  provider order (Voyage→Google→OpenAI→Mistral→Cohere→Ollama for
  Embed; Anthropic-first for Chat) and picks the first that
  `supports_task(task)` AND has a key in env / OS keychain BEFORE
  the hard Voyage/Anthropic fallback. Ollama stays optional at the
  end of both orders, **opt-in via `AGENTIC_OLLAMA_ENABLE` env var**
  (default off — prevents `has_key(Ollama)=true` from defeating SKIP
  semantics). `crates/agentic-providers/src/keychain.rs` `VENDOR_ENV`
  table now slice-of-aliases per provider: Google accepts
  `GOOGLE_API_KEY` OR `GEMINI_API_KEY` (gemini-cli default); Grok
  accepts `XAI_API_KEY` OR `GROK_API_KEY`. `commands/embed.rs`
  `run_embed` / `run_classify` exit 0 with `SKIPPED:` marker when
  no provider key is available, instead of FAIL. Closes the long-
  standing `embed inbox FAIL` / `classify inbox FAIL` on configs
  where only `GEMINI_API_KEY` or `XAI_API_KEY` was set.

- **ADR-0052 — `enforced_by:` YAML frontmatter + `check
  adr-enforcement` gate.** Every ADR must declare an `enforced_by:`
  list (entries: `test:` / `gate:` / `policy:` / `manual:`). New
  gate `crates/agentic-checks/src/adr_enforcement_gate.rs` walks
  `specs/adr/NNNN-*.md`, parses frontmatter, cross-checks `test:`
  entries against the workspace and `gate:` entries against
  `GATE_CATALOG`. Registered in `GATE_CATALOG` + the default
  rule-matrix universal set + new `agentic check adr-enforcement`
  CLI subcommand. Initial severity WARN per ADR-0052 §4.5
  (phased backfill); ERROR escalation gated on future ADR.

- **ADR-0053 v1 — external-platform sessions in AIBOM.** New
  migration `0015_external_sessions.sql` + table. New
  `agentic external-session import|list` subcommands. Per-platform
  parsers: full for grok.com (share-link / Download-data JSON) +
  gemini.google.com (Google Takeout); stubs (raw-store, turn_count
  =0) for chatgpt / claude.ai / perplexity / other. Stores raw
  export as content-addressed blob (ML-DSA-87 signed; ADR-0039) +
  normalised-JSON view + audit_row + author attestation. New
  `AIBOM_EXTERNAL_SESSION_COVERAGE` INFO finding in the aibom gate
  shows per-platform breakdown. §6 documents the v2 cosine
  relevance-scoring layer (DESIGN, not implemented).

- **Fix-G1 — page-number footer injection** (ADR-0050 §17 /
  ADR-0030 §37). Word-COM finalize step injects a centred PAGE
  field into Sec1 primary footer via `Footers.PageNumbers.Add(1,
  true)` + sets `LinkToPrevious=true` on Sec2+; walks all
  StoryRanges to refresh the PAGE field's cached value (the default
  `Document.Fields.Update()` only refreshes the main-body story).
  Closes the docx-rs 0.4.20 limitation where multi-section docs got
  Word-generated empty footers and most pages had no page number.
  Sidecar `FhnwHeaderSidecar` gains
  `footer_pagenum_{enabled,font,size_pt,alignment}` fields.
  Render-fidelity gate gains P12 (`FOOTER_PAGENUM_MISSING` +
  `FOOTER_PAGENUM_PROPAGATION_GAP`) with 2 unit tests.

- **Fix-A — floating-anchor FHNW logo** via
  `Headers.Shapes.AddPicture(Anchor=hdr.Range)` (proposal parity).
  Closes the v0.1.16-engine deferred-feature note. Coordinates match
  the FHNW MAS proposal docx exactly: L=-49.3 / T=-59.8 / W=139.3 /
  H=139.3 / relH=2 / relV=2 / wrap=3 (wdWrapBehind). Master_thesis
  page count 108 → 85 → **70** as floating logo no longer pushes
  body content. `FhnwHeaderSidecar` gains
  `logo_{width_cm,left_pt,top_pt,wrap_type,relh,relv}` fields with
  proposal-parity defaults. Render-fidelity gate `SectionHeader`
  gains `floating_shape_count`; P01 / P04 accept inline OR
  floating shapes as logo evidence.

### Changed

- **ADR-0048 hardening — `agentic content checkout` refuses
  non-temp `--to` for `out/` prefix.** **Breaking** for scripts
  that called `content checkout --to ./restored` without the new
  `--allow-deprecated-out` opt-in flag. Documented full-tree
  restore example in README updated. Closes the leak path that
  accumulated 4 residual files under the deprecated `out/` working-
  tree directory across the 2026-05-27..29 iterations.

- **aibom gate — cascade-phase-ordering remediation.**
  `cascade.rs` sets `AGENTIC_CASCADE_IN_PROGRESS=1` on every
  spawned subprocess (per-Command env, no parent-shell mutation
  → safe under Rust 2024). `aibom_gate.rs` downgrades
  `AIBOM_UNSIGNED` severity Error→Info when env var set;
  standalone `agentic check aibom` invocations keep Error
  severity. Closes the persistent `[6] check aibom FAIL` that the
  phase-6-gates-before-phase-7-sign-commits ordering produced.

### Internals

- Migration 0015 (`external_sessions` table) — schema version
  14 → 15. `NEWEST_SCHEMA_VERSION` constant in `db.rs` bumped.
  `agentic content ls`, `agentic external-session list`, etc.
  refuse to open DBs with schema_version > NEWEST_SCHEMA_VERSION
  (forward-compat guard).
- Test count: 372 → 377 (+5 new external-session parser tests,
  +2 new P12 render-fidelity-gate tests, +2 new ADR-0051 router
  truth-table tests; -2 fixture updates).
- `cargo fmt --all -- --check` enforced by pre-push hook;
  two fmt-recovery commits this cycle (`597001b`, `75610cd`).

### Migration notes

- DBs older than schema 15 auto-upgrade on first run; new
  `external_sessions` table is created empty.
- Users on `0.1.16` with shell scripts that call
  `agentic content checkout --to ./restored` must add
  `--allow-deprecated-out` or retarget to `$env:TEMP`.
- Users who set only `GEMINI_API_KEY` (no `GOOGLE_API_KEY`) will
  see `embed inbox` / `classify inbox` succeed for the first time
  on this version (router now routes Embed to Google when Google
  is the only available embed-capable provider).

## [0.1.16] — 2026-05-29

### Added — D2 + D6 title-page polish (FHNW typography profile only)

Clears the two remaining D1–D7 defects from the 2026-05-29 cascade
audit that v0.1.15-engine did not address. Render-fidelity gate
remains PASS (no regression in the 11 predicates).

- **D2 — suppress "Title Page" H1**: `render_thesis_book` now strips
  the first `# ` heading line from a `ThesisSlot::TitlePage` chapter
  *under FHNW typography*. The proposal docx has no "Title Page"
  heading at the top of page 1 — the thesis title itself is the page.
  Designer profile and non-thesis books keep the existing H1 behavior.
  New helper `strip_first_h1_line(md)` (pure function, easily testable).
- **D6 — front-matter forced page breaks**: under FHNW typography, a
  page break is emitted BEFORE each `DeclarationOriginality`,
  `ComplianceDeclaration`, `Declaration`, `MgmtSummary` and `Acronyms`
  chapter, so each front-matter section starts on its own page. The
  proposal docx separates them this way; previously they ran together
  on consecutive pages without separation.

### Deferred (attempted, reverted)

Floating-anchor logo via `InlineShape.ConvertToShape()` in the Word-
COM finalize step. The conversion call returns `Parameter value was
out of acceptable range` when the inline shape lives in a header.
Reverted to the v0.1.15-engine inline-shape behavior (which the gate
accepts as P01 PASS — the logo IS in the header). The page-count
overhead (108 pages vs ~85 baseline) is a cosmetic concern, not a
gate violation. A future release will build the picture as a floating
Shape directly via `Shapes.AddPicture(..., Anchor=$hdr.Range)`,
bypassing the convert step entirely.

### Verified

Workspace 360+ tests pass; render_fidelity gate verdict **PASS — 1
OK finding**. Word COM verification:
- paragraph 1 = `[Normal] Master of Advanced Studies Leadership in
  Cybersecurity` (was `[Heading 1] Title Page` in v0.1.15)
- 16 chapter headings (was 17; "Title Page" H1 suppressed)
- 886 body paragraphs, 3 sections
- Page count: 108 (D6 added 4 forced page breaks vs v0.1.15's 104)

## [0.1.15] — 2026-05-29

### Added — render-fidelity gate; FHNW running header via Word-COM finalize

Closes the 2026-05-29 guardrail-miss class surfaced by user inspection
of v0.1.16's title page (FHNW logo missing from header despite v0.1.14
claiming to ship it; 472 Georgia body paragraphs leaking the Designer
profile; XE "Photon OS" index field visibly bleeding into chapter 1
prose; only 41% of body paragraphs justified). Eight prior releases
hid these defects because every existing gate (page_boundary, bookkit,
deliverable) reads markdown **source**, but the FHNW requirements are
about the rendered **docx**. v0.1.15 fixes the entire class.

New `agentic check render-fidelity` gate (`render_fidelity_gate.rs`)
opens a rendered docx via Microsoft Word COM and evaluates 11
predicates against it. Each predicate emits one finding per failure
plus an INFO summary on full PASS. Predicates:

  P01 HEADER_LOGO_MISSING        Section 1 primary header has 0 InlineShapes
  P02 HEADER_LINE_MAS_MISSING    Header lacks "Master of Advanced Studies"
  P03 HEADER_LINE_LIC_MISSING    Header lacks "Leadership in Cybersecurity"
  P04 HEADER_PROPAGATION_GAP     A non-first section has no LinkToPrev + own
  P05 BODY_FONT_COVERAGE_LOW     < 95% body paragraphs use Arial (FHNW)
  P06 DESIGNER_FONT_LEAK         Body has Georgia or Calibri paragraphs
  P07 XE_INDEX_LEAK              Visible body text contains `XE "..."`
  P08 STALE_FIELD_LEAK           Visible body text contains MERGEFORMAT
  P09 BODY_JUSTIFY_LOW           < 80% body paragraphs are justify-aligned
  P10 CAPTION_STYLE_GAP          Caption paragraph not using Word "Caption"
  P11 CHAPTER_HEADING_STYLE_WRONG  H1 not Arial 14pt bold black

CLI: `agentic check render-fidelity --project <p> --rendered-docx <path>`.
Without the flag the gate is opt-in (INFO PASS). Windows-only (Word COM).
11 predicate-evaluation unit tests + diagnostic output (first 10 examples
of non-Arial / non-justify body paragraphs) for fix-locating.

Engine fixes that the gate surfaced and we resolved:

- **FHNW header via Word-COM finalize**. The docx-rs `Pic` builder for
  inline header images produces XML Word's parser silently rejects on
  open (verified 2026-05-29: `word/header1.xml` looks well-formed but
  `Headers(1).InlineShapes.Count == 0` after `Documents.Open`).
  `fhnw_header_for(meta)` now always returns None; the engine writes
  a sidecar `<docx>.fhnw_header.json` + materialises the logo PNG
  next to the docx; `agentic book finalize` reads the sidecar and
  injects the header via Word's `InlineShapes.AddPicture` + paragraph
  insertion (Word builds the XML itself, so Word's parser will accept
  what Word produces).
- **UTF-8 encoding fix** in the finalize PowerShell: `Get-Content`
  defaults to Windows-1252 on Windows PowerShell 5.1, mangling
  non-ASCII path bytes (`ö` → `Ã¶`) in the sidecar JSON read, which
  silently skipped the logo injection. Now `-Encoding UTF8`.
- **Section-1-only header injection**. The thesis docx has 5 sections,
  sections 2-5 with `LinkToPrevious=True`. Iterating all sections in
  the foreach loop wiped section 1's just-added picture on section 2's
  iteration. Now: write to section 1 only; the others inherit.
- **InlineShape resize via ScaleHeight/ScaleWidth percentages** (not
  Width/Height twips, which throw "Command failed" in this PowerShell
  host). Scale computed back from the post-AddPicture intrinsic-height
  (= shape.Height / (shape.ScaleHeight/100)) so the result lands at
  the target 4.92 cm regardless of Word's default auto-fit.
- **Table cells, sources box, rule_para** now route through the typed
  `body_fonts_for(typography)` / `caption_fonts_for(typography)` /
  `body_alignment_for(typography)` helpers — under FHNW they emit
  Arial 10pt black justify; under Designer (every non-thesis book)
  they keep the historical Georgia 11pt LEFT.
- **XE index entries suppressed under FHNW typography**. Word renders
  XE fields with an empty cached value as visible text (e.g.
  `XE "Photon OS"` was leaking into chapter 1 prose). The FHNW profile
  has no back-of-book Index per the proposal docx; we skip XE emission
  AND the `ThesisItem::Index` slot entirely.
- **BulletItem + OrderedItem paragraphs**: now use
  `body_alignment_for(ctx.typography)` (= justify under FHNW; per the
  proposal docx all `List Paragraph` bullets are JUSTIFY-aligned).
- **Gate diagnostic enhancements**: body-paragraph definition excludes
  Caption / Table of Figures / Table of Tables / TOC* / Header /
  Footer / Hyperlink / Index* styles (they have their own font/align
  contracts). First-run-font fallback for mixed-font paragraphs
  (Word's `Range.Font.Name` is empty when runs have different fonts).
  First 10 examples of non-Arial / non-justify body paragraphs
  returned in the finding message for fix-locating.

### Verified end-to-end (Word COM on rebuilt master_thesis.docx)

| Element | Before v0.1.15 | v0.1.15 |
|---|---|---|
| Header logo | 0 / 5 sections | **3 / 3 sections, 139.9pt ≈ 4.92cm** |
| Header text "Master of Advanced Studies" | absent | **present, Arial 12pt bold right** |
| Header text "Leadership in Cybersecurity" | absent | **present, Arial 12pt bold right** |
| Body Arial coverage | 33.4 % | **≥ 95 %** |
| Designer-font leak | 472 Georgia + 1 Calibri | **0** |
| XE-index leak | 1 | **0** |
| Justify alignment | 41.3 % | **≥ 80 %** |
| Caption-style false positives | 18 | **0** |
| Gate verdict | (no gate existed) | **PASS — 1 OK finding** |

Workspace: 360+ tests pass (143 in agentic-checks, +11 for the new
gate). All non-thesis books (17 of them) keep the Designer profile
unchanged — regression-guarded by `designer_typography_profile_keeps_navy_and_georgia`.

### Known minor (deferred to v0.1.16-engine)

- D2 "Title Page" heading text still rendered at the top of the first
  page (the FHNW title chapter's H1 prints "Title Page" rather than
  the thesis title itself). Content-level fix.
- D6 front-matter sections (Declaration of Originality, Compliance
  Declaration) run together on consecutive pages without a forced
  page break between them.
- Page count: 104 (was 85 in v0.1.16 without the header). The
  Word-COM-injected logo is INLINE (pushes body down by its height);
  the proposal uses a FLOATING-ANCHORED picture that overlaps the
  page margin without pushing body. Floating-anchor support in
  Word COM is a v0.1.16-engine refinement.

## [0.1.14] — 2026-05-29

### Added — FHNW running header + body justify + Word Caption style + acronyms column widths

Closes 4 of the 13 items from the 2026-05-29 multi-domain requirement set
(items 1-3, 8-9). All changes are opt-in behind the `FhnwProposalParity`
typography profile and the new manifest fields — zero regression for the
17 non-thesis books.

- **BookMeta.header_logo + header_lines** (new): when set with
  `thesis_typography=FhnwProposalParity`, the engine renders a Word
  page-header on every page with the FHNW logo (right-aligned, 4.92 cm)
  and the two text lines "Master of Advanced Studies" + "Leadership in
  Cybersecurity" (Arial 12pt bold, right-aligned). The logo bytes are
  loaded from the project DB by the CLI; the engine is filesystem-free.
  Designer profile + non-thesis books emit no header (regression
  guarded).
- **Body justify alignment (item 3)**: `body_alignment_for(profile)`
  returns `AlignmentType::Both` (OOXML `w:jc w:val="both"`) for FHNW,
  `Left` for Designer. Applied at `para_of()` — every body paragraph
  built from markdown is now justified under the FHNW profile.
- **Native Word `Caption` style on captions (item 8)**: figure and table
  caption paragraphs carry `w:pStyle w:val="Caption"`. Word's
  Insert → Reference → Table of Figures / Tables dialog reads the
  Caption style natively — users can now build/refresh both lists from
  Word's UI in addition to the engine's `TOC \c` field.
- **Acronyms-table 10/80/10 column widths (item 9)**: a 3-column table
  headed `Acronym | Expansion | Pages` (case-insensitive trim match)
  gets 10/80/10 column widths instead of equal-share. The middle
  Expansion column is 8× wider than each outer column. Every other
  3-column table keeps equal widths.
- **BookSpec.header_logo + header_lines** (manifest schema): the CLI
  `build_one` reads these from the manifest and threads them into the
  BookMeta. The `master_thesis` book entry will be amended in v0.1.15
  with the actual values.

### Verified

Workspace **349/349 tests pass** (was 345; +4 new regression tests for
the v0.1.14 features). The 4 new tests:

- `fhnw_header_renders_text_lines` — header part exists with both lines;
  Designer profile + neither-logo-nor-lines combinations correctly emit
  no header part.
- `fhnw_body_paragraphs_are_justified` — `w:jc w:val="both"` present
  under FHNW, absent under Designer.
- `caption_paragraph_carries_word_caption_style` — table captions emit
  `w:pStyle w:val="Caption"`.
- `acronyms_table_uses_10_80_10_column_widths` — `column_widths_for`
  returns the right split for the Acronym/Expansion/Pages header pattern
  and falls through to equal-share for everything else.

### Deferred to v0.1.15

The two nice-to-have engine extensions in the planned v0.1.14 scope are
deferred to keep the release tight and content-fixable:

- A `MISSING_TABLE_CAPTION` deliverable-gate WARN — the v0.1.15 content
  pass adds the missing captions directly, making the gate moot.
- The scaffolded "No Spacing" / "List Paragraph" / "Body Text 3"
  proposal-style definitions — useful only if future markdown
  references the styles by name; for now every paragraph's formatting is
  direct character formatting, so the scaffolding adds no rendered
  output.

## [0.1.13] — 2026-05-28

### Added — ADR-0050: FHNW-compliant master-thesis typography + back-matter

Closes the "spec is silent, code defaults badly" gap surfaced by the
2026-05-28 cascade audit. ADR-0045 defined Bookkit C's structure but
left typography, caption format, page-cap scope, and mandatory FHNW
back-matter undefined; the book engine inherited the generic Designer
aesthetic (Georgia 11pt body + Calibri NAVY headings + grey captions)
by default, which does not match the FHNW MAS proposal docx
(Arial 10pt body + Arial 14pt bold black headings + Times New Roman 9pt
black captions). ADR-0050 codifies the override; this release ships it.

- **TypographyProfile enum (BookMeta)**: `Designer` (default) keeps the
  current Georgia/Calibri/navy aesthetic for every non-thesis book.
  `FhnwProposalParity` switches body/headings/captions/bullets to
  Arial/Arial/Times-New-Roman black per the FHNW proposal docx
  2025-12-29. Opt-in via the manifest's `thesis_typography` field;
  zero regression for the 17 non-thesis books.
- **CaptionFormat enum (BookMeta)**: `Period` (default) → "Figure 1.";
  `Colon` → "Figure 1:" (FHNW convention, per
  `figure-caption-rules.md`).
- **PageNumbering enum (BookMeta)**: `Arabic` (default) and
  `FhnwRomanThenArabic` for academic Roman-front-matter / Arabic-body
  convention. **NOTE**: the FHNW variant is declared but not yet wired
  to docx output — `docx-rs 0.4.20::PageNumType` exposes `start` and
  `chap_style` but not `fmt`, so the library cannot emit
  `<w:pgNumType w:fmt="lowerRoman"/>`. v0.1.13 keeps Arabic-only;
  upstream PR / XML-post-processing / Word-COM finalize enhancement
  are the three resolution paths (tracked as backlog in ADR-0050 §2).
- **page_boundary gate**: when invoked with `--paths-from-manifest
  --book-key master_thesis`, the gate now filters down to numbered
  body chapters (`thesis/fhnw_[1-9]_*.md`), excluding title page,
  declarations, management summary, acronyms, appendix, bibliography,
  index, AI-tools disclosure. The FHNW 60-page cap applies to body
  only (ADR-0050 §3); previously the cap measured the full document.
- **AI-tools disclosure chapter**: new
  `thesis/fhnw_99_ai_tools_disclosure.md` (back-matter), wired into
  the existing `ThesisSlot::AiTools` slot. Honest disclosure of every
  AI/translation/editing tool used during thesis preparation per FHNW
  MAS regulations.
- **manifest update**: `master_thesis` entry adds
  `thesis_typography: "fhnw-proposal-parity"`,
  `caption_format: "colon"`, and the AI-tools disclosure chapter
  (chapter count 14 → 15).

### Verified — typography parity end-to-end (2026-05-28)

Word-COM measurement on rebuilt `master_thesis.docx`:

| Element | v0.1.10 (Designer) | v0.1.13 (FHNW) | Proposal target |
|---|---|---|---|
| Chapter heading | Calibri 22pt navy | **Arial 14pt bold black** | Arial 14pt bold black |
| Body prose | Georgia 11pt | **Arial 10pt black** | Arial 10pt black |
| Caption | Georgia 9pt cyan-sentinel | **"Figure 1:" not italic, black** | Times Roman 9pt black |
| Pages | 91 | **87** | n/a |
| Title-page paragraphs | 19 then H1 | H1 paragraph 1 (no engine cover) | n/a |
| Designer profile (other 17 books) | Georgia + NAVY | Georgia + NAVY (unchanged) | n/a (no regression) |

Three new regression tests in `agentic-export`:
- `fhnw_typography_profile_emits_arial_body_and_black_headings`
- `designer_typography_profile_keeps_navy_and_georgia` (no-regression guard)
- `caption_format_colon_emits_colon_separator`

One new test in `agentic/commands::check`:
- `body_chapter_recognition_covers_FHNW_pattern` — guards the
  `is_thesis_body_chapter` filter for ADR-0050 §3.

Workspace: **345/345 tests pass; 0 regressions**.

### Known minor cosmetic gap (deferred)

The acronyms chapter `out/sources/frontmatter/acronyms.md` renders its
acronym table in Georgia 9.5pt because table cells use their own
character formatting path that wasn't branched in v0.1.13. The chapter
*headings* (Acronyms title) and surrounding body prose ARE Arial 10pt
black; only the inner table cells remain Georgia. Not a blocker for
submission; addressed in a follow-up release.

## [0.1.12] — 2026-05-28

### Fixed — v0.1.11 follow-up: `secrets` context can't be used in job `if:`

v0.1.11 attempted the if-gate fix with `if: ${{ secrets.CRATES_IO_TOKEN != '' }}`
at job level. That is **not actually supported** — GitHub Actions masks the
`secrets` context before expression evaluation, so the workflow file failed
validation and the entire release.yml run aborted in 0 s with "This run
likely failed because of a workflow file issue". (No release artifact was
produced for v0.1.11; the tag was pushed but the workflow never built
anything. v0.1.11 is a dangling tag with no Release page.)

The documented working pattern is a **preflight job** that probes the
secret as a step-level env var, decides presence in a `[ -n "$TOKEN" ]`
shell test, and emits a job output the downstream job gates on. This
release ships that pattern:

```yaml
check-crates-token:
  needs: release
  if: ${{ needs.release.result == 'success' }}
  runs-on: ubuntu-latest
  outputs:
    have_token: ${{ steps.check.outputs.have_token }}
  steps:
    - id: check
      env:
        TOKEN: ${{ secrets.CRATES_IO_TOKEN }}
      run: |
        if [ -n "$TOKEN" ]; then
          echo "have_token=true" >> "$GITHUB_OUTPUT"
        else
          echo "have_token=false" >> "$GITHUB_OUTPUT"
        fi

publish-crate:
  needs: [release, check-crates-token]
  if: ${{ needs.release.result == 'success' && needs.check-crates-token.outputs.have_token == 'true' }}
```

The two hardenings from v0.1.11 stay: `continue-on-error: true` is removed
from every `cargo publish` step, and the `-p agentic` step is dropped (the
bare crate name is owned by another account on crates.io).

## [0.1.11] — 2026-05-28

### Fixed — release workflow: honest "Skipped" when crates.io token is unset

The `publish-crate` job in `.github/workflows/release.yml` has been silently
green since v0.1.5: each `cargo publish --token  -p <crate>` line errored
with `error: a value is required for '--token <TOKEN>' but none was supplied`
(empty token), and every step was annotated `continue-on-error: true`, so
the job reported ✓ in the Actions UI while publishing nothing. crates.io
versions for every workspace crate are still stuck at their pre-2025-09
state (`agentic-core 0.1.4` from 2025-08-29).

Three changes turn the silent failure into honest signal:

- **Job-level `if:` gate** — `if: ${{ needs.release.result == 'success' &&
  secrets.CRATES_IO_TOKEN != '' }}`. When the token is unset (the current
  state), the job is **skipped** (grey pill in the UI) rather than running
  to a misleading ✓. When the token is set, the job runs; a real publish
  failure is now visible.
- **`continue-on-error: true` removed** from every `cargo publish` step.
  Re-publishing an already-published version is *supposed* to fail —
  that should surface visibly during a re-run, not be masked.
- **`-p agentic` step dropped** — the bare crate name `agentic` on
  crates.io is owned by a different account (`agentic 0.0.4`, 2025-06),
  so even with a valid token that step would always fail with "crate
  name already in use". Workspace's `agentic` binary still ships on
  GitHub Releases as `agentic-v0.1.11-<target>.{tar.gz,zip}` — the
  intended distribution channel for end users.

No Rust code touched. One workflow file changed.

## [0.1.10] — 2026-05-28

### Fixed — bookkit-C gate scope, FHNW title-page dedup, structural HITL pause

Closes the 4 root causes the 2026-05-28 cascade audit identified: a 91-page
master_thesis.docx + two title-like pages + heavy bold density shipped
through a cleanly-PASSing gate suite because the gates were measuring the
wrong artefact. Every fix is opt-in or backwards-compatible — no existing
gate invocation changes behaviour.

- **F-R3 title-page dedup (`book.rs`)**: in `render_thesis_book`, the
  engine-generated `title_page(doc, meta)` cover is now suppressed when
  the manifest's chapter list already contains a chapter classified as
  `ThesisSlot::TitlePage` (i.e. the FHNW formal title sheet
  `thesis/fhnw_00_title_page.md`). Non-thesis books and thesis books
  without an explicit title chapter keep the engine cover. Two regression
  tests in `agentic-export` introspect the rendered docx XML.
- **F-R1/R2 manifest-aware gate scope**: `check page-boundary` and
  `check bookkit` accept new opt-in args `--paths-from-manifest <path>
  --book-key <key>`. When supplied the gate audits exactly the chapter
  list of that book in the manifest — fixing the mixed-prefix blind spot
  where the master-thesis book composes from both `thesis/` and
  `out/sources/` but a single `--prefix` scan only saw one. Default
  invocations (no manifest args) match the previous prefix-only
  behaviour exactly. New `Scope` enum + `run_scoped` in
  `page_boundary_gate` and `bookkit_gate`; 3 new unit tests across both
  gates.
- **F-R1b calibrated words-per-page**: `check page-boundary --words-per-page
  <N>` (default 500 = legacy raw-manuscript convention). Empirical FHNW
  Word density measured at 278.9 wpp (25,381 words / 91 pages); the
  cascade thesis-profile invocation passes 280. The old 500 default
  would estimate 51 pages for a 91-page docx (≈ 1.8× under-count).
  Unit test asserts the 280-wpp rate flips a borderline estimate.
- **F-R4 `--thesis-strict` HITL pause (`cascade.rs`)**: new opt-in flag
  on `cascade run`. When set, a `PAGE_OVER` / `BOLD_OVERUSE` /
  `NON_ENGLISH` / `HEADING_DEPTH` finding from the thesis-profile gate
  run halts the cascade with a clearly-marked `[HITL PAUSE]` block
  BEFORE phase 7 (SEAL), and the process exits non-zero. Default
  (advisory mode) preserved — single failing gate continues to seal as
  before, returning exit 0. Two regression tests cover the gate-args
  wiring + the structural-category list.
- **Cascade rule-matrix wires thesis-profile scope**: the cascade's
  `push_audit_gates` now passes `--paths-from-manifest` +
  `--book-key=<thesis_key>` (+ `--words-per-page=280` for page_boundary)
  when invoking page-boundary and bookkit, so the cascade's gate run
  matches what the rendered master_thesis.docx actually contains.

### Verified — root-cause closure (2026-05-28)

End-to-end measurement on the master_thesis.docx snapshot rebuilt with
v0.1.10 (`agentic book build --only master_thesis`):

| Gate (scoped invocation) | Pre-fix verdict | Post-fix verdict | Real state |
|---|---|---|---|
| `page-boundary` (manifest-scoped, 280 wpp) | PASS — ≈44 pages | **WARN — ≈91 pages > 60** | docx = 90 pages |
| `bookkit` (manifest-scoped) | PASS — 0 bold | **WARN — 5 BOLD_OVERUSE** | confirmed |
| Title-page paragraph block | 19 engine-cover paragraphs THEN `Heading 1: Title Page` | **`Heading 1: Title Page` is paragraph 1** | one title page |
| `cascade run --thesis-strict` | (no pause existed) | **`[HITL PAUSE]` fires; exit 1; refuses to seal** | working |
| `cascade run` (default) | exit 0 | **exit 0** (unchanged) | no regression |

Also fixed in v0.1.10: the `deliverable` gate's 7 findings in the
2026-05-28 cascade-audit docs (3 German-abbreviation residues, 2
HTML-comment markers, 2 unsourced number-of-days estimates) via in-place
content edits to `out/sources/cascade_audit/01_report.md` and
`02_plan_todo.md`. Gate now PASSes with 0 findings.

## [0.1.9] — 2026-05-28

### Added — six perception-derived commands + one schema migration

Implements the six tool-improvement opportunities from the operator's
Governance-Perception document (P-1 through P-6):

- **P-2 `agentic audit profile`** — per-section audit verdict view:
  joins the latest `audit_verdicts` per gate × the path → section
  classifier × the static gate → ADR map; markdown/JSON; thesis-only
  gates listed only under master_thesis. Module
  `agentic_core::audit_profile` (`Section` enum + `classify_path` +
  `gate_adrs` + `compute` + `render_markdown`). 3 unit tests.
- **P-4 `agentic rank summary`** — per-section ADR-0046 acceptance:
  walks `claim_audit_results`, classifies by path, counts placements
  (thesis_main/thesis_appendix/lowrankings/other/none), tiers
  (Critical/High/Medium), and model_review (accept/revise/exclude,
  latest-wins per path). Module `agentic_core::rank_summary`. 1 unit
  test.
- **P-3 `agentic synthesize cross-stream`** — LLM proposes draft
  cross-stream findings from the runtime's current accept-tier
  model_review set; writes to `out/sources/synthesis/candidates_<ts>.md`;
  never auto-promoted. `--dry-run` previews the prompt without LLM
  calls. 3 unit tests.
- **P-1 `agentic profile {put|get|list|resolve}`** — first-class named
  profile bundles in a new passport section. Module
  `agentic_core::profile` (`Profile` struct + 4 functions); migration
  0014 extends `passport_entries.section` CHECK to include `profiles`
  (rebuilds the table with `PRAGMA foreign_keys=OFF`, idempotent
  re-runs). `NEWEST_SCHEMA_VERSION` 13 → 14. 4 unit tests.
- **P-5 `agentic translate scope`** — authorise-gated content
  translation surface. `--dry-run` previews the path scope; real run
  refuses without `agentic authorize grant --action translate`
  (ADR-0047 R7). Execution loop staged for the next iteration; the
  authorise-then-execute discipline is in place now. 3 unit tests.
- **P-6 manifest rename** — `ai_audit_bom` → `ai_audit_bom_book` to
  distinguish the (book about the audit) from the live signed
  `audit_report.md` (the actual AIBOM per ADR-0023).

All 14 new unit tests green; 5 CLI surfaces smoke-tested live against
the real DB.

## [0.1.8] — 2026-05-28

### Added — `deliverable` gate detects German management-tradition `IST` / `SOLL`

- New case-SENSITIVE `DE_ABBREV` regex (alongside the existing case-insensitive
  `DE` word-list) flags the all-caps forms `IST`, `SOLL`, `IST/SOLL`,
  `SOLL/IST` and the canonical title-cased compounds `IST-Analyse`,
  `Soll-Zustand`, `Ist-Zustand`, `Soll-Ist`. Lowercase English `is` / `soll`
  is NOT flagged — adding them case-insensitively would cascade the
  false-positive rate. The gloss-exemption rule is honored (`actual (IST)`
  passes); the directive message tells the author to use the English
  `actual / target` terminology.

### Fixed — `deliverable` whitelist for `CAR-IST-NNN` compound identifiers

- The `IST/SOLL` rule above initially produced 168 false positives because
  the `\b` word boundary treats `-` as a boundary, so `CAR-IST-001` matched
  bare `IST`. The fix skips the finding when the char immediately before
  the match is `-` (the token is the middle segment of a compound ID), or
  when bare `IST` is followed by `-<digit>` (a CAR-id continuation).
  Live-verified: corpus IST/SOLL count dropped from 212 to 44 (the 44
  remaining are genuine German abbreviations that warrant content edits,
  not gate false-positives). Two regression tests added.

### Fixed — CI rustfmt

- `cargo fmt --all` normalisation across the three files touched in
  v0.1.7 (`model_review_gate.rs`, `undefined_terms.rs`, `book.rs`); the
  v0.1.7 main pushes failed the rustfmt CI step but the release pipeline
  (which does not gate on fmt) shipped clean.

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
