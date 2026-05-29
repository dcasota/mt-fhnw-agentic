# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
