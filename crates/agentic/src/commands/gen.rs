//! `agentic gen` — generation-prompt assembly (Rust port of `prompt_rules.py`
//! and the `gen_*.py` family). Holds the mandatory generation rules
//! (ADR-0036/0037/0038, figure-standards, Karpathy guidelines) as the single
//! source of truth and assembles a rule-prefixed prompt per artefact kind. The
//! emitted prompt is piped to the chosen LLM CLI (e.g. `claude -p`); the
//! deterministic prompt logic now lives in Rust, not Python.

use anyhow::{Context, Result};

use crate::cli::GenAction;

/// Per-tool front-matter adapters for the generated mission-control agent-defs.
/// The adapter (front-matter) is code; the body is the canonical data file.
const CLAUDE_FRONTMATTER: &str = "---\ndescription: Portfolio orchestrator (SDD-governed mission-control). Manages inbox processing, iteration lifecycle, rule enforcement, git versioning, overlap detection, adversarial arbitration, new-project governance, content ownership. Spectrum: balanced.\ntools: Read, Glob, Grep, LS, Bash, Edit, Write\n---";

const FACTORY_FRONTMATTER: &str = "---\nname: mission-control\ndescription: >-\n  SDD-governed orchestrator for an N-project research / writing portfolio.\n  Manages inbox processing, iteration lifecycle, rule enforcement, git\n  versioning, overlap detection, adversarial arbitration, new-project\n  governance, and content ownership registry.\nmodel: inherit\ntools: [\"Read\", \"LS\", \"Grep\", \"Glob\", \"Edit\", \"Create\", \"Execute\"]\n---";

pub const PREAMBLE: &str = r#"=== MANDATORY GENERATION RULES (do not violate) ===
1. VERIFY, DON'T ASSUME (ADR-0036, Karpathy). Every reference, author, result,
   quote and NUMBER must be real and independently verifiable (DOI / arXiv /
   Crossref / Semantic Scholar / authoritative primary URL). NEVER invent a
   citation, author, finding, statistic or figure. If you cannot verify
   something now, OMIT it and write `NEEDS-VERIFICATION: <what>` instead.
   - Numbers specifically: cite the primary source inline. Do NOT carry forward
     numbers from memory or prior drafts.
2. ENGLISH ONLY (ADR-0037). No German anywhere — body, headings, figure/table
   captions, cell text, labels. (A German term of art may appear once, in
   parentheses, as a gloss.)
3. NO BUILD-PATH CROSS-REFERENCES OR MARKERS (ADR-0038). Never write any
   "out/...docx" path or any HTML comment (<!-- ... -->). Reference other work
   only as a normal academic citation.
4. SIMPLICITY + SURGICAL (Karpathy). Minimal, concrete, sourced prose; no
   padding, no vague grandeur, no speculative claims dressed as fact.
5. FULL-WORD LABELS EVERYWHERE (prose tables AND figures). Prediction-by-mode
   cells MUST be full words: "Decrease / Normal" … "Increase / Catastrophic".
   NEVER codes like "D-N"/"D.E"/"I-C". Never abbreviate project names to "P1".
"#;

pub const FIGSPEC_RULES: &str = r#"=== FIGURE RULES (figure-standards) ===
- Use only real graphical figspec types: bar | hbar | line | matrix | flow |
  quadrant. A heading is not a figure.
- Use `quadrant` for SWOT (quadrants tl/tr/bl/br with title + items).
- FULL-WORD labels; never cryptic codes. matrix cells short (a word or two);
  add a "legend" field if a header needs explaining.
- CAPTIONS ARE SHORT: a figspec "caption" is <= 12 words (a label phrase only);
  put interpretation in the body prose that references the figure.
- schema: {"id":"figX_NN","type":"...","title":"<short>","caption":"<<=12 words>",
  "palette":"wong","data":{...}}. The renderer places the caption BELOW the figure.
"#;

fn body_for(kind: &str, topic: &str, extra: &str) -> String {
    let base = match kind {
        "dimension" => format!(
            "Write the full Dimension source for: {topic}.\nStructure: Theory (state of the art, sourced) then Solution (what Photon OS governance should do). Include 6-9 graphical figspec figures. Trace every claim to a verifiable source.",
        ),
        "campaign" => format!(
            "Write the Campaign source for: {topic}.\nInclude: framing, the projects/tools (P1..Pn with real short names), engineering risk assessment, prediction-by-mode ranking (full-word cells), ISO/IEC 42001 touchpoints, SDD trace, dependencies, summary. Graphical figspec figures only.",
        ),
        "project" => format!(
            "Write the Project/Tool source for: {topic}.\nInclude: what it is, why, owner, effort, HITL touchpoints, operating-mode (normal/escalation/catastrophic), provenance. Concrete and sourced.",
        ),
        "condense" => format!(
            "Condense the following into eligible findings ranked on the prediction-by-mode taxonomy (full-word cells), keeping provenance: {topic}.",
        ),
        _ => format!("Write a sourced, English-only artefact on: {topic}."),
    };
    if extra.is_empty() {
        base
    } else {
        format!("{base}\n\nAdditional instructions:\n{extra}")
    }
}

pub fn run(action: GenAction, _json: bool) -> Result<()> {
    match action {
        GenAction::Rules => {
            print!("{PREAMBLE}\n{FIGSPEC_RULES}");
        }
        GenAction::Prompt { kind, topic, extra } => {
            let body = body_for(&kind, &topic, extra.as_deref().unwrap_or(""));
            print!("{PREAMBLE}\n{FIGSPEC_RULES}\n\n=== TASK ===\n{body}\n");
        }
        GenAction::AgentDefs { root } => {
            use agentic_core::govdoc::{
                CANONICAL_MISSION_CONTROL, GENERATED_AGENT_DEFS, GENERATED_MARKER,
            };
            let canon = root.join(CANONICAL_MISSION_CONTROL);
            let body = std::fs::read_to_string(&canon)
                .with_context(|| format!("reading canonical body {}", canon.display()))?;
            let body = body.trim_end();
            for rel in GENERATED_AGENT_DEFS {
                let fm = if rel.contains(".factory") {
                    FACTORY_FRONTMATTER
                } else {
                    CLAUDE_FRONTMATTER
                };
                let path = root.join(rel);
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&path, format!("{fm}\n\n{GENERATED_MARKER}\n\n{body}\n"))
                    .with_context(|| format!("writing {}", path.display()))?;
                println!("generated {rel}");
            }
        }
    }
    Ok(())
}
