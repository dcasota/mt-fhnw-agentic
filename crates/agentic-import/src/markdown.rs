//! Markdown passthrough: import a `.md` / `.markdown` file as-is.

use std::path::Path;

use anyhow::{Context, Result};

/// Read the file at `path` as UTF-8 text.
pub fn extract_text(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    String::from_utf8(bytes).with_context(|| format!("{} is not valid UTF-8", path.display()))
}
