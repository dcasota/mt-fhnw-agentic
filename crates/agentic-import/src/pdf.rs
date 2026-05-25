//! PDF text extraction.

use std::path::Path;

use anyhow::{Result, anyhow};

/// Extract plain text from a PDF.
///
/// `pdf_extract` is fragile on malformed/unusual PDFs — it `panic!`s on some
/// content streams (e.g. `operands.len() == 6` assertions). A single bad file
/// must never abort a batch import, so the call is run inside
/// [`std::panic::catch_unwind`] and a panic is converted into an `Err`.
pub fn extract_text(path: &Path) -> Result<String> {
    let p = path.to_path_buf();
    // Silence pdf_extract's panic backtrace (we convert the panic into an Err);
    // restore the previous hook immediately after.
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        pdf_extract::extract_text(&p)
    }));
    std::panic::set_hook(prev_hook);
    match result {
        Ok(Ok(text)) => Ok(text),
        Ok(Err(e)) => Err(anyhow!("pdf_extract on {}: {e}", path.display())),
        Err(_) => Err(anyhow!(
            "pdf_extract panicked on {} (malformed or unsupported PDF stream)",
            path.display()
        )),
    }
}
