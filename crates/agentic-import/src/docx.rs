//! DOCX text extraction (paragraph text only, no formatting).

use std::path::Path;

use anyhow::{Context, Result};

/// Extract paragraph text from a .docx file.
pub fn extract_text(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let doc = docx_rs::read_docx(&bytes).map_err(|e| anyhow::anyhow!("read_docx: {e:?}"))?;
    let mut out = String::new();
    for child in &doc.document.children {
        if let docx_rs::DocumentChild::Paragraph(p) = child {
            for run in &p.children {
                if let docx_rs::ParagraphChild::Run(r) = run {
                    for inner in &r.children {
                        if let docx_rs::RunChild::Text(t) = inner {
                            out.push_str(&t.text);
                        }
                    }
                }
            }
            out.push('\n');
        }
    }
    Ok(out)
}
