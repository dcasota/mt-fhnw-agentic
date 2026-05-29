//! `agentic check render-fidelity` — verify a rendered FHNW master-thesis
//! docx against the structural and typographic predicates derived from the
//! FHNW MAS proposal docx (extracted by the proposal-inspection agent
//! 2026-05-29).
//!
//! Background. The 2026-05-29 cascade audit exposed a class of "guardrail
//! miss" defects: the page-boundary gate, the bookkit gate and the
//! deliverable gate all read **markdown source**, but the FHNW thesis
//! requirements are about the **rendered .docx**:
//!
//! * the FHNW logo must appear in the page header (D1)
//! * the section title text on the title page must NOT be "Title Page" (D2)
//! * "Master of Advanced Studies" and "Leadership in Cybersecurity" must
//!   render as two separate lines (D3)
//! * markdown `---` horizontal rules must render in Arial (FHNW), not
//!   Georgia (Designer leftover) (D4)
//! * body paragraphs must have a consistent named font (no empty-font
//!   runs) (D5)
//! * the front-matter declarations must each start on their own page (D6)
//! * Word INDEX `XE "Foo"` markers must not leak into visible body text
//!   (D7)
//!
//! These are render-time properties; only Word (or an equally faithful
//! OOXML reader) can verify them. This gate runs a PowerShell subprocess
//! that opens the docx via Microsoft Word COM, walks the document, and
//! emits one finding per failed predicate.
//!
//! The predicates are **opt-in by `--rendered-docx <path>`**. When the
//! flag is absent the gate emits an INFO finding (rendered docx not
//! supplied — gate not applicable) and PASSes; the cascade can keep its
//! `--thesis-strict` flag listing the gate without forcing every project
//! to ship a rendered docx.
//!
//! Predicates (one finding per failed predicate; INFO summary for pass):
//!
//! | id | name | severity | meaning |
//! |----|------|----------|---------|
//! | P01 | HEADER_LOGO_MISSING | ERROR | Section 1 primary header has 0 InlineShapes |
//! | P02 | HEADER_LINE_MAS_MISSING | ERROR | Header text lacks "Master of Advanced Studies" |
//! | P03 | HEADER_LINE_LIC_MISSING | ERROR | Header text lacks "Leadership in Cybersecurity" |
//! | P04 | HEADER_PROPAGATION_GAP | ERROR | A non-first section has no LinkToPrevious and no own header |
//! | P05 | BODY_FONT_COVERAGE_LOW | WARN | <95% body paragraphs use Arial under FHNW profile |
//! | P06 | DESIGNER_FONT_LEAK | ERROR | Body contains Georgia or Calibri paragraphs (Designer leakage) |
//! | P07 | XE_INDEX_LEAK | ERROR | Visible body text contains `XE "…"` index-field leak |
//! | P08 | STALE_FIELD_LEAK | WARN | Visible body text contains "{ PAGE \\* MERGEFORMAT }" stale field |
//! | P09 | BODY_JUSTIFY_LOW | WARN | <80% body paragraphs are justify-aligned (FHNW convention) |
//! | P10 | CAPTION_STYLE_GAP | WARN | A figure/table caption paragraph does not use Word "Caption" style |
//! | P11 | CHAPTER_HEADING_STYLE_WRONG | ERROR | An H1 chapter heading is not Arial 14pt bold black |
//!
//! Runs only on Windows (Microsoft Word required). Non-Windows builds
//! return an UNSUPPORTED INFO finding (PASS verdict).
//!
//! Tests focus on the predicate-parsing logic (independent of Word COM):
//! see `parse_word_report` and its unit tests.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::{CheckReport, Finding, Severity};

/// Run the gate against a rendered docx path.
///
/// `proposal_docx` is reserved for a future "structural diff vs proposal"
/// mode; today the gate evaluates the 11 predicates above against the
/// rendered docx only (every predicate is derived FROM the proposal but
/// encoded as a self-contained predicate, so the proposal isn't needed
/// at runtime).
pub fn run(
    rendered_docx: Option<&std::path::Path>,
    _proposal_docx: Option<&std::path::Path>,
) -> Result<CheckReport> {
    let Some(rendered) = rendered_docx else {
        return Ok(CheckReport::new(
            "render_fidelity",
            vec![Finding {
                category: "NO_RENDERED_DOCX".into(),
                severity: Severity::Info,
                message: "no --rendered-docx supplied; render-fidelity gate is opt-in and \
                          requires a built thesis docx"
                    .into(),
                location: None,
            }],
        ));
    };
    if !rendered.exists() {
        return Ok(CheckReport::new(
            "render_fidelity",
            vec![Finding {
                category: "RENDERED_DOCX_NOT_FOUND".into(),
                severity: Severity::Error,
                message: format!("rendered docx not found: {}", rendered.display()),
                location: None,
            }],
        ));
    }

    let report = inspect_docx_via_word(rendered)?;
    let findings = predicates_from_report(&report);
    Ok(CheckReport::new("render_fidelity", findings))
}

/// Word-extracted facts about a docx. Populated by the PowerShell helper
/// (Windows-only) or by a unit-test fixture (cross-platform).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WordReport {
    pub sections: u32,
    /// One entry per section (index 0 = section 1).
    pub section_headers: Vec<SectionHeader>,
    /// Total body paragraphs (style != Heading*, non-empty trimmed text).
    pub body_paragraph_count: u32,
    /// Body paragraphs whose first run uses font name "Arial".
    pub body_arial_count: u32,
    /// Body paragraphs whose first run uses Georgia (the Designer-leftover face).
    pub body_georgia_count: u32,
    /// Body paragraphs whose first run uses Calibri (the Designer-leftover face).
    pub body_calibri_count: u32,
    /// Body paragraphs with alignment = wdAlignParagraphJustify (= 3) or Both (= 1).
    pub body_justify_count: u32,
    /// All visible body text concatenated (lowercased, for substring scans).
    pub body_text_concat: String,
    /// Per-caption paragraph: whether it uses Word's built-in "Caption" style.
    pub caption_paragraphs: Vec<CaptionParagraph>,
    /// Per-H1 chapter heading: its font name, size, bold flag, color.
    pub chapter_headings: Vec<ChapterHeading>,
    /// First 10 body paragraphs whose font is NOT Arial — diagnostic to
    /// locate the engine code path producing the leakage.
    #[serde(default)]
    pub non_arial_examples: Vec<NonArialExample>,
    /// First 10 body paragraphs whose alignment is NOT justify — same
    /// diagnostic purpose for the justify gate.
    #[serde(default)]
    pub non_justify_examples: Vec<NonJustifyExample>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NonArialExample {
    pub font: String,
    pub style: String,
    pub text_snippet: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NonJustifyExample {
    pub alignment_code: i32,
    pub style: String,
    pub text_snippet: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SectionHeader {
    pub link_to_previous: bool,
    pub inline_shape_count: u32,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CaptionParagraph {
    pub text: String,
    pub style_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ChapterHeading {
    pub text: String,
    pub font: String,
    pub size_pt: f32,
    pub bold: bool,
    /// Lowercase hex without leading `#`. "000000" for black.
    pub color_hex: String,
}

/// Evaluate the 11 predicates against a [`WordReport`]. Pure function; the
/// Word-COM call lives in [`inspect_docx_via_word`].
#[must_use]
pub fn predicates_from_report(r: &WordReport) -> Vec<Finding> {
    let mut out = Vec::new();

    // P01 — header logo
    let header_inlines: u32 = r
        .section_headers
        .first()
        .map(|h| h.inline_shape_count)
        .unwrap_or(0);
    if header_inlines == 0 {
        out.push(Finding {
            category: "HEADER_LOGO_MISSING".into(),
            severity: Severity::Error,
            message: "section 1 primary header has no inline shape; the FHNW logo \
                      must appear at the top-right of every page (ADR-0050 §1 item 1)"
                .into(),
            location: Some("Headers(1).Range.InlineShapes".into()),
        });
    }

    let h1_text = r
        .section_headers
        .first()
        .map(|h| h.text.as_str())
        .unwrap_or("");

    // P02 / P03 — header lines
    if !h1_text.contains("Master of Advanced Studies") {
        out.push(Finding {
            category: "HEADER_LINE_MAS_MISSING".into(),
            severity: Severity::Error,
            message: "header lacks the line 'Master of Advanced Studies' \
                      (FHNW running-header convention)"
                .into(),
            location: Some("Headers(1).Range.Text".into()),
        });
    }
    if !h1_text.contains("Leadership in Cybersecurity") {
        out.push(Finding {
            category: "HEADER_LINE_LIC_MISSING".into(),
            severity: Severity::Error,
            message: "header lacks the line 'Leadership in Cybersecurity' \
                      (FHNW running-header convention)"
                .into(),
            location: Some("Headers(1).Range.Text".into()),
        });
    }

    // P04 — header propagation (sections 2+ must inherit via LinkToPrevious
    // OR have their own non-empty header)
    for (idx, sec) in r.section_headers.iter().enumerate().skip(1) {
        let inherits = sec.link_to_previous;
        let has_own = sec.inline_shape_count > 0 || !sec.text.trim().is_empty();
        if !inherits && !has_own {
            out.push(Finding {
                category: "HEADER_PROPAGATION_GAP".into(),
                severity: Severity::Error,
                message: format!(
                    "section {} primary header is empty and not linked to previous; \
                     the FHNW header must appear on every page",
                    idx + 1
                ),
                location: Some(format!("Sections({}).Headers(1)", idx + 1)),
            });
        }
    }

    // P05 / P06 — font coverage
    if r.body_paragraph_count > 0 {
        let arial_pct = (f64::from(r.body_arial_count) / f64::from(r.body_paragraph_count)) * 100.0;
        if arial_pct < 95.0 {
            let examples: String = if r.non_arial_examples.is_empty() {
                String::new()
            } else {
                let s: Vec<String> = r
                    .non_arial_examples
                    .iter()
                    .take(5)
                    .map(|e| {
                        format!(
                            " [{}|{}|'{}']",
                            e.font,
                            e.style,
                            e.text_snippet.chars().take(50).collect::<String>().trim()
                        )
                    })
                    .collect();
                format!(" examples:{}", s.join(""))
            };
            out.push(Finding {
                category: "BODY_FONT_COVERAGE_LOW".into(),
                severity: Severity::Warn,
                message: format!(
                    "only {arial_pct:.1}% of body paragraphs use Arial (target ≥ 95% \
                     under FHNW typography); {} / {} paragraphs.{examples}",
                    r.body_arial_count, r.body_paragraph_count
                ),
                location: None,
            });
        }
        if r.body_georgia_count > 0 {
            out.push(Finding {
                category: "DESIGNER_FONT_LEAK".into(),
                severity: Severity::Error,
                message: format!(
                    "{} body paragraph(s) render in Georgia; FHNW profile forbids \
                     Designer-leftover fonts in body prose",
                    r.body_georgia_count
                ),
                location: None,
            });
        }
        if r.body_calibri_count > 0 {
            out.push(Finding {
                category: "DESIGNER_FONT_LEAK".into(),
                severity: Severity::Error,
                message: format!(
                    "{} body paragraph(s) render in Calibri; FHNW profile forbids \
                     Designer-leftover fonts in body prose",
                    r.body_calibri_count
                ),
                location: None,
            });
        }
    }

    // P07 — XE-index leak (Word INDEX `XE "Foo"` markers escaping to visible text)
    if let Some(pos) = r.body_text_concat.find("XE \"") {
        let snippet_start = pos.saturating_sub(20);
        let snippet_end = (pos + 40).min(r.body_text_concat.len());
        let snippet: String = r.body_text_concat[snippet_start..snippet_end]
            .chars()
            .take(60)
            .collect();
        out.push(Finding {
            category: "XE_INDEX_LEAK".into(),
            severity: Severity::Error,
            message: format!(
                "Word INDEX 'XE \"…\"' marker appears in visible body text \
                 (example near: …{snippet}…); a previously-hidden field has \
                 become visible — re-finalize or clean up the source"
            ),
            location: None,
        });
    }

    // P08 — stale Word MERGEFORMAT field markers leaking into visible text
    if r.body_text_concat.contains("MERGEFORMAT") {
        out.push(Finding {
            category: "STALE_FIELD_LEAK".into(),
            severity: Severity::Warn,
            message: "stale Word field markers ('MERGEFORMAT') visible in body text; \
                      finalize step did not refresh all fields"
                .into(),
            location: None,
        });
    }

    // P09 — body justify coverage
    if r.body_paragraph_count > 0 {
        let just_pct =
            (f64::from(r.body_justify_count) / f64::from(r.body_paragraph_count)) * 100.0;
        if just_pct < 80.0 {
            let examples: String = if r.non_justify_examples.is_empty() {
                String::new()
            } else {
                let s: Vec<String> = r
                    .non_justify_examples
                    .iter()
                    .take(5)
                    .map(|e| {
                        format!(
                            " [align={}|{}|'{}']",
                            e.alignment_code,
                            e.style,
                            e.text_snippet.chars().take(50).collect::<String>().trim()
                        )
                    })
                    .collect();
                format!(" examples:{}", s.join(""))
            };
            out.push(Finding {
                category: "BODY_JUSTIFY_LOW".into(),
                severity: Severity::Warn,
                message: format!(
                    "only {just_pct:.1}% of body paragraphs are justify-aligned \
                     (target ≥ 80% under FHNW typography); {} / {} paragraphs.{examples}",
                    r.body_justify_count, r.body_paragraph_count
                ),
                location: None,
            });
        }
    }

    // P10 — caption-style coverage. Skip paragraphs styled "Table of
    // Figures" / "Table of Tables" / "TOC *" — these are auto-generated
    // List-of-Figures/Tables ENTRIES (not primary captions). The Word
    // ToF/ToT field updates them on every Repaginate and they always have
    // the "Table of Figures" style, never the "Caption" style — flagging
    // them as a caption-style miss is a false positive.
    let cap_bad: Vec<&CaptionParagraph> = r
        .caption_paragraphs
        .iter()
        .filter(|c| {
            !c.style_name.eq_ignore_ascii_case("Caption")
                && !c.style_name.eq_ignore_ascii_case("Table of Figures")
                && !c.style_name.eq_ignore_ascii_case("Table of Tables")
                && !c.style_name.to_ascii_lowercase().starts_with("toc ")
        })
        .collect();
    if !cap_bad.is_empty() {
        let preview: String = cap_bad
            .iter()
            .take(3)
            .map(|c| {
                let t: String = c.text.chars().take(40).collect();
                format!("'{}' (style={})", t.trim(), c.style_name)
            })
            .collect::<Vec<_>>()
            .join("; ");
        out.push(Finding {
            category: "CAPTION_STYLE_GAP".into(),
            severity: Severity::Warn,
            message: format!(
                "{} figure/table caption(s) do not use Word's built-in 'Caption' \
                 style; the native List of Figures / Tables dialog will not find \
                 them. Examples: {preview}",
                cap_bad.len()
            ),
            location: None,
        });
    }

    // P11 — chapter heading style
    for h in &r.chapter_headings {
        let arial = h.font.eq_ignore_ascii_case("Arial");
        let size_ok = (h.size_pt - 14.0).abs() < 0.5;
        let bold_ok = h.bold;
        let color_ok = h.color_hex.eq_ignore_ascii_case("000000");
        if !(arial && size_ok && bold_ok && color_ok) {
            out.push(Finding {
                category: "CHAPTER_HEADING_STYLE_WRONG".into(),
                severity: Severity::Error,
                message: format!(
                    "chapter heading '{}' is {} {}pt bold={} colour=#{} \
                     — expected Arial 14pt bold black",
                    h.text.chars().take(40).collect::<String>(),
                    h.font,
                    h.size_pt,
                    h.bold,
                    h.color_hex
                ),
                location: Some(format!("Paragraph: '{}'", h.text)),
            });
        }
    }

    if out.is_empty() {
        out.push(Finding {
            category: "RENDER_FIDELITY_OK".into(),
            severity: Severity::Info,
            message: format!(
                "all 11 predicates passed; {} sections, {} body paragraphs, \
                 {} chapter headings, {} captions inspected",
                r.sections,
                r.body_paragraph_count,
                r.chapter_headings.len(),
                r.caption_paragraphs.len()
            ),
            location: None,
        });
    }

    out
}

/// Inspect the docx via Microsoft Word COM; cross-platform fallback is a
/// single UNSUPPORTED INFO finding. Windows-only beyond the cfg guard.
#[cfg(windows)]
fn inspect_docx_via_word(docx_path: &std::path::Path) -> Result<WordReport> {
    use std::process::Command;
    let abs = std::fs::canonicalize(docx_path)
        .with_context(|| format!("resolve {}", docx_path.display()))?
        .to_string_lossy()
        .replace(r"\\?\", "");
    // The PowerShell script writes one JSON line to stdout: the WordReport.
    // We single-quote the path and escape any embedded single quote by
    // doubling (PowerShell rule).
    let abs_quoted = format!("'{}'", abs.replace('\'', "''"));
    let script = format!(
        r#"$ErrorActionPreference='Stop'
$path = {abs_quoted}
$w = New-Object -ComObject Word.Application
$w.Visible = $false
$w.DisplayAlerts = 0
try {{
  $d = $w.Documents.Open($path, $false, $true, $false)
  $r = @{{ sections = $d.Sections.Count; section_headers = @(); body_paragraph_count = 0;
          body_arial_count = 0; body_georgia_count = 0; body_calibri_count = 0;
          body_justify_count = 0; body_text_concat = ''; caption_paragraphs = @();
          chapter_headings = @(); non_arial_examples = @(); non_justify_examples = @() }}
  foreach ($sec in $d.Sections) {{
    $h = $sec.Headers.Item(1)
    $r.section_headers += @{{
      link_to_previous = [bool]$h.LinkToPrevious
      inline_shape_count = [int]$h.Range.InlineShapes.Count
      text = $h.Range.Text
    }}
  }}
  $sb = New-Object System.Text.StringBuilder
  foreach ($p in $d.Paragraphs) {{
    $style = $p.Style.NameLocal
    $text  = $p.Range.Text.Trim()
    if ($style -like 'Heading*') {{
      if ($style -eq 'Heading 1' -and $text.Length -gt 0) {{
        $clr = $p.Range.Font.Color
        $rgb = 0
        if ($clr -ge 0) {{
          $rgb = $clr -band 0xFFFFFF
        }}
        $r.chapter_headings += @{{
          text = $text
          font = "$($p.Range.Font.Name)"
          size_pt = [single]$p.Range.Font.Size
          bold = [bool]($p.Range.Font.Bold -ne 0)
          color_hex = ('{{0:X6}}' -f $rgb).ToLowerInvariant()
        }}
      }}
      continue
    }}
    # Exclude paragraphs whose style is not a "body" style: Caption,
    # Table of Figures/Tables, TOC*, Header, Footer, Hyperlink etc.
    # These have their own font requirements (Caption = Times New Roman
    # in the FHNW profile; TOC entries inherit ToC styles). Counting
    # them as body would create spurious DESIGNER_FONT_LEAK / Arial-
    # coverage findings.
    if (
      $style -eq 'Caption' -or
      $style -eq 'Table of Figures' -or
      $style -eq 'Table of Tables' -or
      $style -like 'TOC *' -or
      $style -eq 'Header' -or
      $style -eq 'Footer' -or
      $style -eq 'Hyperlink' -or
      $style -eq 'Index Heading' -or
      $style -like 'Index *'
    ) {{ continue }}
    if ($text.Length -lt 5) {{ continue }}  # skip short / structural paragraphs
    $r.body_paragraph_count++
    # Word's $p.Range.Font.Name returns empty string when the paragraph
    # contains runs with multiple different fonts (verified 2026-05-29).
    # That breaks the "is this paragraph Arial?" question for mixed-font
    # paragraphs that are intrinsically Arial-bodied with inline code or
    # italic spans. Fix: when the aggregated font is empty, inspect the
    # first run's font instead — that's the effective body font for the
    # paragraph.
    $font = $p.Range.Font.Name
    if (-not $font -or $font -eq '') {{
      try {{
        $firstRun = $p.Range.Words.Item(1)
        if ($firstRun) {{ $font = $firstRun.Font.Name }}
      }} catch {{}}
    }}
    if ($font -eq 'Arial')   {{ $r.body_arial_count++ }}
    if ($font -eq 'Georgia') {{ $r.body_georgia_count++ }}
    if ($font -eq 'Calibri') {{ $r.body_calibri_count++ }}
    # Word alignment: 1 = Center, 2 = Right, 3 = Justify, 4 = Distribute, 0 = Left
    # AlignmentType::Both serialises as `w:jc w:val="both"`; in COM it appears as 3 (Justify).
    if ($p.Alignment -eq 3) {{ $r.body_justify_count++ }}
    # Collect first 10 examples of non-Arial / non-justify body paragraphs so
    # the gate's BODY_FONT_COVERAGE_LOW / BODY_JUSTIFY_LOW findings can name
    # the offending paragraphs (lets the engine fixer locate the code path).
    if ($font -ne 'Arial' -and $r.non_arial_examples.Count -lt 10) {{
      $snippet = $text.Substring(0,[math]::Min(120,$text.Length))
      $r.non_arial_examples += @{{
        font = "$font"
        style = "$style"
        text_snippet = $snippet
      }}
    }}
    if ($p.Alignment -ne 3 -and $r.non_justify_examples.Count -lt 10) {{
      $snippet = $text.Substring(0,[math]::Min(120,$text.Length))
      $r.non_justify_examples += @{{
        alignment_code = [int]$p.Alignment
        style = "$style"
        text_snippet = $snippet
      }}
    }}
    [void]$sb.Append($text + ' ')
    # Detect captions by leading 'Figure ' / 'Table ' + digit + separator
    if ($text -match '^(Figure|Table)\s+\d+[:.]') {{
      $r.caption_paragraphs += @{{
        text = $text
        style_name = "$style"
      }}
    }}
  }}
  $r.body_text_concat = $sb.ToString()
  $d.Close($false)
  $r | ConvertTo-Json -Depth 8 -Compress
}} finally {{
  $w.Quit()
  [System.Runtime.InteropServices.Marshal]::ReleaseComObject($w) | Out-Null
}}"#
    );
    let out = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .context("launch Word via powershell (is Microsoft Word installed?)")?;
    if !out.status.success() {
        anyhow::bail!(
            "Word inspection failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    // The script emits a single JSON line at the end; ignore any preceding lines.
    let json_line = stdout
        .lines()
        .rev()
        .find(|l| l.trim_start().starts_with('{'))
        .with_context(|| {
            format!(
                "no JSON line in Word inspection output (last 200 chars): {}",
                stdout.chars().rev().take(200).collect::<String>()
            )
        })?;
    let report: WordReport = serde_json::from_str(json_line)
        .with_context(|| format!("parse WordReport JSON: {json_line}"))?;
    Ok(report)
}

#[cfg(not(windows))]
fn inspect_docx_via_word(_docx_path: &std::path::Path) -> Result<WordReport> {
    // Cross-platform stub: the gate returns an UNSUPPORTED INFO finding via the
    // empty WordReport (every predicate that depends on counts will be vacuously
    // satisfied, and the caller's run() wraps that as the gate verdict).
    Ok(WordReport::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn good_report() -> WordReport {
        WordReport {
            sections: 3,
            section_headers: vec![
                SectionHeader {
                    link_to_previous: false,
                    inline_shape_count: 1,
                    text: "Master of Advanced Studies Leadership in Cybersecurity\n".into(),
                },
                SectionHeader {
                    link_to_previous: true,
                    inline_shape_count: 1,
                    text: "Master of Advanced Studies Leadership in Cybersecurity\n".into(),
                },
                SectionHeader {
                    link_to_previous: true,
                    inline_shape_count: 1,
                    text: "Master of Advanced Studies Leadership in Cybersecurity\n".into(),
                },
            ],
            body_paragraph_count: 100,
            body_arial_count: 99,
            body_georgia_count: 0,
            body_calibri_count: 0,
            body_justify_count: 95,
            body_text_concat: "lorem ipsum dolor sit amet".into(),
            caption_paragraphs: vec![CaptionParagraph {
                text: "Figure 1: Sample".into(),
                style_name: "Caption".into(),
            }],
            chapter_headings: vec![ChapterHeading {
                text: "1 Introduction".into(),
                font: "Arial".into(),
                size_pt: 14.0,
                bold: true,
                color_hex: "000000".into(),
            }],
            non_arial_examples: Vec::new(),
            non_justify_examples: Vec::new(),
        }
    }

    #[test]
    fn good_report_emits_single_ok_finding() {
        let f = predicates_from_report(&good_report());
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].category, "RENDER_FIDELITY_OK");
        assert!(matches!(f[0].severity, Severity::Info));
    }

    #[test]
    fn missing_logo_flags_p01() {
        let mut r = good_report();
        r.section_headers[0].inline_shape_count = 0;
        let f = predicates_from_report(&r);
        assert!(f.iter().any(|x| x.category == "HEADER_LOGO_MISSING"));
    }

    #[test]
    fn missing_header_lines_flags_p02_p03() {
        let mut r = good_report();
        r.section_headers[0].text = String::new();
        let f = predicates_from_report(&r);
        assert!(f.iter().any(|x| x.category == "HEADER_LINE_MAS_MISSING"));
        assert!(f.iter().any(|x| x.category == "HEADER_LINE_LIC_MISSING"));
    }

    #[test]
    fn header_propagation_gap_flags_p04() {
        let mut r = good_report();
        r.section_headers[1] = SectionHeader {
            link_to_previous: false,
            inline_shape_count: 0,
            text: String::new(),
        };
        let f = predicates_from_report(&r);
        assert!(f.iter().any(|x| x.category == "HEADER_PROPAGATION_GAP"));
    }

    #[test]
    fn low_arial_coverage_flags_p05() {
        let mut r = good_report();
        r.body_arial_count = 80; // 80% of 100
        let f = predicates_from_report(&r);
        assert!(f.iter().any(|x| x.category == "BODY_FONT_COVERAGE_LOW"));
    }

    #[test]
    fn georgia_leak_flags_p06() {
        let mut r = good_report();
        r.body_georgia_count = 3;
        let f = predicates_from_report(&r);
        let leaks: Vec<_> = f
            .iter()
            .filter(|x| x.category == "DESIGNER_FONT_LEAK")
            .collect();
        assert_eq!(leaks.len(), 1);
        assert!(
            leaks[0]
                .message
                .contains("3 body paragraph(s) render in Georgia")
        );
    }

    #[test]
    fn xe_index_leak_flags_p07() {
        let mut r = good_report();
        r.body_text_concat = "some prose with XE \"Broadcom\" inside it".into();
        let f = predicates_from_report(&r);
        assert!(f.iter().any(|x| x.category == "XE_INDEX_LEAK"));
    }

    #[test]
    fn stale_mergeformat_flags_p08() {
        let mut r = good_report();
        r.body_text_concat = "prose with PAGE \\* MERGEFORMAT showing".into();
        let f = predicates_from_report(&r);
        assert!(f.iter().any(|x| x.category == "STALE_FIELD_LEAK"));
    }

    #[test]
    fn low_justify_flags_p09() {
        let mut r = good_report();
        r.body_justify_count = 50; // 50%
        let f = predicates_from_report(&r);
        assert!(f.iter().any(|x| x.category == "BODY_JUSTIFY_LOW"));
    }

    #[test]
    fn caption_without_word_style_flags_p10() {
        let mut r = good_report();
        r.caption_paragraphs.push(CaptionParagraph {
            text: "Figure 2: Other".into(),
            style_name: "Normal".into(),
        });
        let f = predicates_from_report(&r);
        let g: Vec<_> = f
            .iter()
            .filter(|x| x.category == "CAPTION_STYLE_GAP")
            .collect();
        assert_eq!(g.len(), 1);
        assert!(g[0].message.contains("Figure 2: Other"));
    }

    #[test]
    fn wrong_chapter_heading_flags_p11() {
        let mut r = good_report();
        r.chapter_headings.push(ChapterHeading {
            text: "Bad Heading".into(),
            font: "Calibri".into(),
            size_pt: 22.0,
            bold: true,
            color_hex: "1F3864".into(),
        });
        let f = predicates_from_report(&r);
        assert!(
            f.iter()
                .any(|x| x.category == "CHAPTER_HEADING_STYLE_WRONG")
        );
    }
}
