//! Format detection from path extension.

use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Markdown,
    Docx,
    Pdf,
}

impl Format {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Markdown => "markdown",
            Self::Docx => "docx",
            Self::Pdf => "pdf",
        }
    }
}

/// Detect format from the path's extension. Returns `None` for unsupported types.
#[must_use]
pub fn from_path(path: &Path) -> Option<Format> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "md" | "markdown" => Format::Markdown,
        "docx" => Format::Docx,
        "pdf" => Format::Pdf,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn detects_known_extensions() {
        assert_eq!(from_path(&PathBuf::from("a.md")), Some(Format::Markdown));
        assert_eq!(
            from_path(&PathBuf::from("a.markdown")),
            Some(Format::Markdown)
        );
        assert_eq!(from_path(&PathBuf::from("a.MD")), Some(Format::Markdown));
        assert_eq!(from_path(&PathBuf::from("a.docx")), Some(Format::Docx));
        assert_eq!(from_path(&PathBuf::from("a.pdf")), Some(Format::Pdf));
    }

    #[test]
    fn rejects_unknown_extensions() {
        assert_eq!(from_path(&PathBuf::from("a.txt")), None);
        assert_eq!(from_path(&PathBuf::from("a")), None);
    }
}
