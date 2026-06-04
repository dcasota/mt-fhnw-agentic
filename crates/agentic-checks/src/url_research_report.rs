//! Rust port of `MT-Template/dist/_build_url_report.py` — turn the output of
//! the bibliography URL-research workflow (35 parallel WebSearch/WebFetch
//! agents) into a markdown summary table.
//!
//! Wave-2 Agent C (Python→Rust migration, 2026-06-04). The Python script
//! consumed a single-shot workflow JSON file produced by an external tool;
//! this Rust port keeps the same input contract (`{result: {results: [...]}}`)
//! but is callable as a library function so the agentic CLI can render the
//! report without shelling out to Python.
//!
//! Confidence bucketing matches the Python script verbatim:
//!   * `high`   — `suggestedUrl != "NOT FOUND"`, ranked first;
//!   * `medium` — same predicate, ranked second;
//!   * `low`    — same predicate, ranked third;
//!   * `nf`     — `suggestedUrl == "NOT FOUND"`;
//!   * `mis`    — any item whose `reasoning` contains a misattribution
//!                marker phrase (one of: "misattribut", "not authored by",
//!                "rather than", "not by ", "appears to misattribute").
//!
//! i18n: the table headings ("Outcome", "Count", "Ref", "Reference",
//! "Suggested URL", "Reasoning") and the rendered section titles ("High-
//! confidence URL suggestions (safe to apply)" …) currently stay English to
//! match the Python output byte-for-byte. New i18n keys are flagged in the
//! Wave-2 Agent C report for W2-D to seed: `url_report_outcome`,
//! `url_report_count`, `url_report_high`, `url_report_medium`,
//! `url_report_low`, `url_report_not_found`, `url_report_misattributions`,
//! `url_report_total_researched`.

use serde::{Deserialize, Serialize};

/// A single research result for one bibliography entry — mirrors the
/// `result.results[i]` element produced by the URL-research workflow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UrlResearchItem {
    pub r#ref: BibRef,
    pub result: UrlResult,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BibRef {
    #[serde(rename = "Num")]
    pub num: String,
    #[serde(rename = "Surname", default)]
    pub surname: String,
    #[serde(rename = "Text")]
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UrlResult {
    #[serde(rename = "suggestedUrl")]
    pub suggested_url: String,
    pub confidence: String,
    pub reasoning: String,
}

/// Top-level workflow output `{result: {results: [...]}}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowOutput {
    pub result: WorkflowResultEnvelope,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowResultEnvelope {
    pub results: Vec<UrlResearchItem>,
}

/// Ranked buckets ready for rendering.
#[derive(Debug, Default)]
pub struct Buckets<'a> {
    pub high: Vec<&'a UrlResearchItem>,
    pub medium: Vec<&'a UrlResearchItem>,
    pub low: Vec<&'a UrlResearchItem>,
    pub not_found: Vec<&'a UrlResearchItem>,
    pub misattributions: Vec<&'a UrlResearchItem>,
}

/// Sort key matching the Python `num_key` lambda: strip brackets, parse int,
/// fall back to a sentinel that sinks unparsable refs to the bottom.
#[must_use]
pub fn num_key(item: &UrlResearchItem) -> u32 {
    let n = item.r#ref.num.trim_matches(['[', ']']);
    n.parse::<u32>().unwrap_or(9999)
}

/// The five misattribution marker phrases — case-insensitive substring match.
const MISATTRIBUTION_MARKERS: &[&str] = &[
    "misattribut",
    "not authored by",
    "rather than",
    "not by ",
    "appears to misattribute",
];

/// Group results into the 5 buckets (high / medium / low / not_found /
/// misattributions). Items appear in `misattributions` IN ADDITION to their
/// confidence bucket — the Python script did the same.
#[must_use]
pub fn bucketize(results: &[UrlResearchItem]) -> Buckets<'_> {
    let mut sorted: Vec<&UrlResearchItem> = results.iter().collect();
    sorted.sort_by_key(|i| num_key(i));

    let mut b = Buckets::default();
    for r in sorted {
        let found = r.result.suggested_url != "NOT FOUND";
        match r.result.confidence.as_str() {
            "high" if found => b.high.push(r),
            "medium" if found => b.medium.push(r),
            "low" if found => b.low.push(r),
            _ => {}
        }
        if !found {
            b.not_found.push(r);
        }
        let why = r.result.reasoning.to_lowercase();
        if MISATTRIBUTION_MARKERS.iter().any(|m| why.contains(m)) {
            b.misattributions.push(r);
        }
    }
    b
}

/// Escape a markdown table cell — pipes + newlines + CRs.
fn esc(s: &str) -> String {
    s.replace('|', r"\|").replace('\n', " ").replace('\r', " ")
}

/// Truncate to a max char count, appending "..." when cut.
fn ellipsis(s: &str, max: usize) -> String {
    if s.chars().count() > max {
        let head: String = s.chars().take(max).collect();
        format!("{head}...")
    } else {
        s.to_string()
    }
}

/// Render the workflow output as the markdown report. Identical structure
/// to the Python script's `target_md` so existing downstream tooling reads
/// it without changes.
#[must_use]
pub fn render(workflow: &WorkflowOutput) -> String {
    let b = bucketize(&workflow.result.results);
    let total = workflow.result.results.len();
    let mut out = Vec::<String>::new();
    out.push("# Bibliography URL research report".into());
    out.push(String::new());
    out.push(
        "Generated by a Workflow of 35 parallel research agents (WebSearch + WebFetch).".into(),
    );
    out.push(String::new());
    out.push("## Summary".into());
    out.push(String::new());
    out.push("| Outcome | Count |".into());
    out.push("|---|---|".into());
    out.push(format!(
        "| URLs found (high confidence) | {} |",
        b.high.len()
    ));
    out.push(format!(
        "| URLs found (medium confidence) | {} |",
        b.medium.len()
    ));
    out.push(format!("| URLs found (low confidence) | {} |", b.low.len()));
    out.push(format!("| NOT FOUND | {} |", b.not_found.len()));
    out.push(format!(
        "| Agent-flagged misattributions | {} |",
        b.misattributions.len()
    ));
    out.push(format!("| **Total researched** | **{total}** |"));
    out.push(String::new());

    if !b.misattributions.is_empty() {
        out.push("## Misattributions flagged by agents".into());
        out.push(String::new());
        out.push(
            "Worth a careful read — the agents found that the cited author may be wrong for these refs:"
                .into(),
        );
        out.push(String::new());
        out.push("| Ref | Issue |".into());
        out.push("|---|---|".into());
        for r in &b.misattributions {
            let reason = esc(&r.result.reasoning);
            let text_short = esc(&ellipsis(&r.r#ref.text, 80));
            out.push(format!(
                "| `{}` {}... | {} |",
                r.r#ref.num, text_short, reason
            ));
        }
        out.push(String::new());
    }

    render_section(
        &mut out,
        &b.high,
        "High-confidence URL suggestions (safe to apply)",
    );
    render_section(
        &mut out,
        &b.medium,
        "Medium-confidence URL suggestions (review before applying)",
    );
    render_section(
        &mut out,
        &b.low,
        "Low-confidence URL suggestions (read carefully)",
    );
    render_section(&mut out, &b.not_found, "NOT FOUND");

    out.join("\n") + "\n"
}

fn render_section(out: &mut Vec<String>, rows: &[&UrlResearchItem], label: &str) {
    out.push(format!("## {label}"));
    out.push(String::new());
    if rows.is_empty() {
        out.push("_None._".into());
        out.push(String::new());
        return;
    }
    out.push("| Ref | Reference (truncated) | Suggested URL | Reasoning |".into());
    out.push("|---|---|---|---|".into());
    for r in rows {
        let text = esc(&ellipsis(&r.r#ref.text, 110));
        let url = esc(&r.result.suggested_url);
        let why = esc(&ellipsis(&r.result.reasoning, 220));
        out.push(format!(
            "| `{}` | {} | `{}` | {} |",
            r.r#ref.num, text, url, why
        ));
    }
    out.push(String::new());
}

/// Render the per-item "applied" JSON the Python script also emits — a flat
/// list of `{Num, Surname, Text, suggestedUrl, confidence, reasoning}` used
/// by downstream apply scripts.
///
/// Pretty-prints with 2-space indent to match the Python `json.dumps(..,
/// indent=2)` default.
#[must_use]
pub fn applied_json(workflow: &WorkflowOutput) -> serde_json::Value {
    workflow
        .result
        .results
        .iter()
        .map(|r| {
            serde_json::json!({
                "Num": r.r#ref.num,
                "Surname": r.r#ref.surname,
                "Text": r.r#ref.text,
                "suggestedUrl": r.result.suggested_url,
                "confidence": r.result.confidence,
                "reasoning": r.result.reasoning,
            })
        })
        .collect::<Vec<_>>()
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(num: &str, url: &str, conf: &str, reason: &str) -> UrlResearchItem {
        UrlResearchItem {
            r#ref: BibRef {
                num: num.into(),
                surname: "Doe".into(),
                text: "Doe (2024). A long bibliography line that we want to truncate.".into(),
            },
            result: UrlResult {
                suggested_url: url.into(),
                confidence: conf.into(),
                reasoning: reason.into(),
            },
        }
    }

    fn workflow(items: Vec<UrlResearchItem>) -> WorkflowOutput {
        WorkflowOutput {
            result: WorkflowResultEnvelope { results: items },
        }
    }

    #[test]
    fn num_key_strips_brackets_and_parses_int() {
        assert_eq!(num_key(&item("[12]", "x", "high", "ok")), 12);
        assert_eq!(num_key(&item("3", "x", "high", "ok")), 3);
        assert_eq!(num_key(&item("foo", "x", "high", "ok")), 9999);
    }

    #[test]
    fn bucketize_routes_by_confidence_and_found_predicate() {
        let items = vec![
            item("[1]", "https://a", "high", "ok"),
            item("[2]", "https://b", "medium", "ok"),
            item("[3]", "https://c", "low", "ok"),
            item("[4]", "NOT FOUND", "low", "ok"),
        ];
        let wf = workflow(items);
        let b = bucketize(&wf.result.results);
        assert_eq!(b.high.len(), 1);
        assert_eq!(b.medium.len(), 1);
        assert_eq!(b.low.len(), 1);
        assert_eq!(b.not_found.len(), 1);
        // "NOT FOUND" must not appear in any confidence bucket.
        assert!(
            b.high
                .iter()
                .chain(&b.medium)
                .chain(&b.low)
                .all(|r| r.result.suggested_url != "NOT FOUND")
        );
    }

    #[test]
    fn bucketize_detects_misattribution_markers() {
        let items = vec![
            item(
                "[1]",
                "https://a",
                "high",
                "appears to MISATTRIBUTE the work",
            ),
            item("[2]", "https://b", "high", "all looks fine"),
            item("[3]", "https://c", "low", "Not authored by Doe."),
        ];
        let wf = workflow(items);
        let b = bucketize(&wf.result.results);
        assert_eq!(b.misattributions.len(), 2);
        assert_eq!(b.misattributions[0].r#ref.num, "[1]");
        assert_eq!(b.misattributions[1].r#ref.num, "[3]");
    }

    #[test]
    fn render_emits_summary_table_and_section_headings() {
        let wf = workflow(vec![
            item("[1]", "https://a", "high", "ok"),
            item("[2]", "NOT FOUND", "high", "no source"),
        ]);
        let md = render(&wf);
        assert!(md.contains("# Bibliography URL research report"));
        assert!(md.contains("| URLs found (high confidence) | 1 |"));
        assert!(md.contains("| NOT FOUND | 1 |"));
        assert!(md.contains("## High-confidence URL suggestions (safe to apply)"));
        assert!(md.contains("## NOT FOUND"));
        // Total line.
        assert!(md.contains("| **Total researched** | **2** |"));
    }

    #[test]
    fn render_empty_section_shows_none_marker() {
        let wf = workflow(vec![item("[1]", "https://a", "high", "ok")]);
        let md = render(&wf);
        assert!(
            md.contains("## Medium-confidence URL suggestions (review before applying)\n\n_None._")
        );
    }

    #[test]
    fn render_escapes_pipes_and_truncates_long_text() {
        let mut bad = item(
            "[1]",
            "https://a|with-pipe",
            "high",
            "x".repeat(300).as_str(),
        );
        bad.r#ref.text = "Author | with pipes ".repeat(20);
        let wf = workflow(vec![bad]);
        let md = render(&wf);
        // pipes escaped
        assert!(md.contains(r"\|"));
        // truncation marker present
        assert!(md.contains("..."));
    }

    #[test]
    fn applied_json_round_trips_fields() {
        let wf = workflow(vec![item("[7]", "https://x", "high", "ok")]);
        let v = applied_json(&wf);
        assert_eq!(v[0]["Num"], "[7]");
        assert_eq!(v[0]["suggestedUrl"], "https://x");
        assert_eq!(v[0]["confidence"], "high");
    }
}
