//! End-to-end smoke: render a small book with `body_render_use_bk_styles =
//! true`, unzip the resulting docx, and assert `word/styles.xml` contains
//! exactly 186 `<w:style>` elements (Wave-2 AI-Norms parity, 2026-06-03).

use std::io::{Cursor, Read};
use std::path::Path;

use agentic_export::book::{BookMeta, render_book};

#[test]
fn rendered_book_with_bk_styles_flag_emits_186_styles_in_zip() {
    let meta = BookMeta {
        title: "Wave2 Parity Smoke".into(),
        body_render_use_bk_styles: true,
        ..Default::default()
    };
    let md = "# Smoke\n\nA paragraph.\n".to_string();
    let bytes = render_book(&meta, &[("c1".into(), md)], Path::new(".")).expect("render");
    assert_eq!(&bytes[..4], b"PK\x03\x04");

    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).expect("open zip");
    let mut styles = String::new();
    zip.by_name("word/styles.xml")
        .expect("styles.xml present")
        .read_to_string(&mut styles)
        .expect("read styles.xml");

    let n = styles.matches("<w:style ").count();
    assert_eq!(
        n, 186,
        "rendered docx should contain 186 styles when body_render_use_bk_styles=true; got {n}"
    );
}

#[test]
fn rendered_book_without_flag_keeps_docx_rs_default_styles_xml() {
    // Backward compat: a book that does NOT set the flag must keep the
    // historical docx-rs-emitted styles.xml (fewer than 186 styles). We
    // assert strictly less than 186 to catch accidental opt-in regressions.
    let meta = BookMeta {
        title: "Wave2 Compat Smoke".into(),
        ..Default::default()
    };
    let md = "# Smoke\n\nA paragraph.\n".to_string();
    let bytes = render_book(&meta, &[("c1".into(), md)], Path::new(".")).expect("render");
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).expect("open zip");
    let mut styles = String::new();
    zip.by_name("word/styles.xml")
        .expect("styles.xml present")
        .read_to_string(&mut styles)
        .expect("read styles.xml");

    let n = styles.matches("<w:style ").count();
    assert!(
        n < 186,
        "default-flag rendering unexpectedly produced {n} styles (>=186); did the flag wiring leak?"
    );
}
