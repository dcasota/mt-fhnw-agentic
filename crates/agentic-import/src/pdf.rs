//! PDF text extraction.

use std::path::Path;

use anyhow::{Context, Result};

/// Extract plain text from a PDF.
pub fn extract_text(path: &Path) -> Result<String> {
    pdf_extract::extract_text(path).with_context(|| format!("pdf_extract on {}", path.display()))
}
