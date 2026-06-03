//! `parity_report` — render a [`ParityReport`] as a single self-contained
//! HTML page. Used by `agentic check parity --html-report <path>`.
//!
//! The page has three layers:
//!
//! 1. **Header banner** — overall verdict (PASS/WARN/FAIL), parity %,
//!    reference & current paths.
//! 2. **Per-scope summary cards** — figures / tables / styles / layout,
//!    each showing PASS/WARN/FAIL counts.
//! 3. **Findings table** — every sub-check with severity colour, expected
//!    / actual / delta / evidence.
//!
//! The HTML embeds its own CSS (no external dependencies) so the file is
//! checkable into a snapshot / repro lock and survives offline review.

use std::path::Path;

use anyhow::{Context, Result};

use crate::parity::{summarise_by_scope, ParityReport};
use crate::Severity;

/// Write the parity report to `out_path` as a self-contained HTML page.
pub fn write_html(report: &ParityReport, out_path: &Path) -> Result<()> {
    let html = render_html(report);
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating parent dir for {}", out_path.display()))?;
    }
    std::fs::write(out_path, html)
        .with_context(|| format!("writing parity HTML report to {}", out_path.display()))?;
    Ok(())
}

/// Build the HTML string. Public for tests.
#[must_use]
pub fn render_html(report: &ParityReport) -> String {
    let (overall_class, overall_label) = overall_verdict(report);
    let summary = summarise_by_scope(report);

    let mut out = String::new();
    out.push_str(HTML_HEAD);
    out.push_str(&format!(
        r#"
<body>
<div class="container">
  <header class="banner banner-{overall_class}">
    <h1>Visual / Structural Parity Gate</h1>
    <p class="verdict">Overall verdict: <strong>{overall_label}</strong>
       — parity = <strong>{pct:.1}%</strong> of reference</p>
    <p class="paths">
      <span><strong>Reference:</strong> <code>{reference}</code></span><br/>
      <span><strong>Current:</strong> <code>{current}</code></span>
    </p>
  </header>
"#,
        pct = report.parity_pct,
        reference = escape(&report.reference),
        current = escape(&report.current),
    ));

    out.push_str("<section class=\"summary\">\n<h2>Per-scope summary</h2>\n<div class=\"cards\">\n");
    for (scope, (pass, warn, fail)) in &summary {
        let card_class = if *fail > 0 {
            "card-fail"
        } else if *warn > 0 {
            "card-warn"
        } else {
            "card-pass"
        };
        out.push_str(&format!(
            r#"<div class="card {card_class}">
  <h3>{scope}</h3>
  <p class="card-counts">
    <span class="pill pill-pass">{pass} PASS</span>
    <span class="pill pill-warn">{warn} WARN</span>
    <span class="pill pill-fail">{fail} FAIL</span>
  </p>
</div>
"#,
            scope = escape(scope),
        ));
    }
    out.push_str("</div>\n</section>\n");

    out.push_str("<section class=\"findings\">\n<h2>Findings</h2>\n<table>\n<thead><tr>");
    out.push_str(
        "<th>Scope</th><th>Sub-check</th><th>Severity</th>\
         <th>Expected</th><th>Actual</th><th>Delta</th><th>Evidence</th>",
    );
    out.push_str("</tr></thead>\n<tbody>\n");

    for f in &report.findings {
        let sev = severity_class(f.severity);
        let sev_label = severity_label(f.severity);
        out.push_str(&format!(
            r#"<tr class="row-{sev}">
  <td>{scope}</td>
  <td><code>{name}</code></td>
  <td><span class="pill pill-{sev}">{sev_label}</span></td>
  <td>{expected}</td>
  <td>{actual}</td>
  <td>{delta:+}</td>
  <td class="evidence">{evidence}</td>
</tr>
"#,
            scope = escape(&f.scope),
            name = escape(&f.name),
            expected = escape(&f.expected),
            actual = escape(&f.actual),
            delta = f.delta,
            evidence = escape(&f.evidence),
        ));
    }

    out.push_str("</tbody>\n</table>\n</section>\n</div>\n</body>\n</html>\n");
    out
}

fn overall_verdict(report: &ParityReport) -> (&'static str, &'static str) {
    let has_fail = report
        .findings
        .iter()
        .any(|f| matches!(f.severity, Severity::Error | Severity::Blocking));
    let has_warn = report
        .findings
        .iter()
        .any(|f| matches!(f.severity, Severity::Warn));
    if has_fail {
        ("fail", "FAIL")
    } else if has_warn {
        ("warn", "WARN")
    } else {
        ("pass", "PASS")
    }
}

fn severity_class(sev: Severity) -> &'static str {
    match sev {
        Severity::Info => "pass",
        Severity::Warn => "warn",
        Severity::Error | Severity::Blocking => "fail",
    }
}

fn severity_label(sev: Severity) -> &'static str {
    match sev {
        Severity::Info => "PASS",
        Severity::Warn => "WARN",
        Severity::Error => "FAIL",
        Severity::Blocking => "BLOCK",
    }
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

const HTML_HEAD: &str = r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8"/>
<title>Parity Gate Report</title>
<style>
  :root {
    --pass: #2e7d32;
    --warn: #f9a825;
    --fail: #c62828;
    --bg: #fafafa;
    --fg: #1f1f1f;
    --card-bg: #ffffff;
    --border: #e0e0e0;
  }
  body { font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
         background: var(--bg); color: var(--fg); margin: 0; }
  .container { max-width: 1100px; margin: 0 auto; padding: 16px; }
  .banner { padding: 20px 24px; border-radius: 8px; color: #fff; margin-bottom: 24px; }
  .banner-pass { background: var(--pass); }
  .banner-warn { background: var(--warn); color: #1f1f1f; }
  .banner-fail { background: var(--fail); }
  .banner h1 { margin: 0 0 8px 0; font-size: 1.4rem; }
  .banner .verdict { margin: 0; font-size: 1.05rem; }
  .banner .paths { margin-top: 12px; font-size: 0.9rem; }
  .banner code { background: rgba(255,255,255,0.2); padding: 2px 6px; border-radius: 3px; }
  h2 { font-size: 1.1rem; border-bottom: 1px solid var(--border); padding-bottom: 4px; }
  .cards { display: grid; grid-template-columns: repeat(auto-fit, minmax(180px, 1fr));
           gap: 12px; margin: 12px 0 24px 0; }
  .card { background: var(--card-bg); border: 1px solid var(--border);
          padding: 12px 16px; border-radius: 6px; }
  .card h3 { margin: 0 0 8px 0; font-size: 1rem; text-transform: capitalize; }
  .card-pass { border-left: 4px solid var(--pass); }
  .card-warn { border-left: 4px solid var(--warn); }
  .card-fail { border-left: 4px solid var(--fail); }
  .card-counts { display: flex; gap: 6px; flex-wrap: wrap; margin: 0; }
  .pill { padding: 2px 8px; border-radius: 10px; font-size: 0.8rem; font-weight: 600;
          color: #fff; display: inline-block; }
  .pill-pass { background: var(--pass); }
  .pill-warn { background: var(--warn); color: #1f1f1f; }
  .pill-fail { background: var(--fail); }
  table { width: 100%; border-collapse: collapse; background: var(--card-bg);
          border: 1px solid var(--border); border-radius: 6px; overflow: hidden;
          font-size: 0.9rem; }
  th, td { padding: 8px 10px; text-align: left; border-bottom: 1px solid var(--border);
           vertical-align: top; }
  th { background: #f0f0f0; }
  .row-pass { background: rgba(46,125,50,0.05); }
  .row-warn { background: rgba(249,168,37,0.07); }
  .row-fail { background: rgba(198,40,40,0.07); }
  .evidence { font-family: ui-monospace, "Cascadia Mono", Menlo, Consolas, monospace;
              font-size: 0.82rem; color: #444; word-break: break-all; }
  code { font-family: ui-monospace, "Cascadia Mono", Menlo, Consolas, monospace; }
</style>
</head>
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parity::{ParityFinding, ParityReport};

    fn sample_report() -> ParityReport {
        ParityReport {
            reference: "ref.docx".into(),
            current: "cur.docx".into(),
            findings: vec![
                ParityFinding {
                    scope: "figures".into(),
                    name: "figure_count_parity".into(),
                    severity: Severity::Error,
                    expected: "133".into(),
                    actual: "0".into(),
                    delta: -133,
                    evidence: "ref vs cur".into(),
                    message: "no figures".into(),
                },
                ParityFinding {
                    scope: "layout".into(),
                    name: "footer_page_field".into(),
                    severity: Severity::Info,
                    expected: "true".into(),
                    actual: "true".into(),
                    delta: 0,
                    evidence: "ref vs cur".into(),
                    message: "ok".into(),
                },
            ],
            parity_pct: 50.0,
        }
    }

    #[test]
    fn html_includes_overall_fail_when_any_error() {
        let html = render_html(&sample_report());
        assert!(html.contains("FAIL"));
        assert!(html.contains("banner-fail"));
    }

    #[test]
    fn html_renders_per_scope_cards() {
        let html = render_html(&sample_report());
        assert!(html.contains("figures"));
        assert!(html.contains("layout"));
        assert!(html.contains("1 FAIL"));
        assert!(html.contains("1 PASS"));
    }

    #[test]
    fn html_escapes_angle_brackets() {
        let mut r = sample_report();
        r.findings[0].evidence = "<unsafe>".into();
        let html = render_html(&r);
        assert!(html.contains("&lt;unsafe&gt;"));
        assert!(!html.contains("<unsafe>"));
    }

    #[test]
    fn write_html_creates_parent_dirs(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let out = tmp.path().join("nested/sub/dir/report.html");
        write_html(&sample_report(), &out)?;
        assert!(out.exists());
        let body = std::fs::read_to_string(&out)?;
        assert!(body.contains("Parity Gate"));
        Ok(())
    }
}
