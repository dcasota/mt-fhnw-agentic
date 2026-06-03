//! Wave-6 AI-Norms parity (ADR-0054 v1, 2026-06-03): body headings emit
//! `pStyle="BkH{1..4}"` when [`BookMeta::body_render_use_bk_styles`] is
//! `true`, and the historical `pStyle="Heading{1..4}"` otherwise. Both
//! tests render a small markdown sample, unzip the resulting docx, and
//! scan `word/document.xml` for the expected `<w:pStyle w:val="…"/>`
//! references on heading paragraphs.

use std::io::{Cursor, Read};
use std::path::Path;

use agentic_export::book::{render_book, BookMeta};

fn document_xml(bytes: Vec<u8>) -> String {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).expect("open zip");
    let mut xml = String::new();
    zip.by_name("word/document.xml")
        .expect("document.xml present")
        .read_to_string(&mut xml)
        .expect("read document.xml");
    xml
}

/// A two-heading sample so we can assert BkH1 (chapter H1) AND BkH2
/// (sub-heading) under the AI-Norms parity flag.
const TWO_HEADING_MD: &str = "# Chapter One\n\n\
A body paragraph.\n\n\
## Sub Heading\n\n\
Another paragraph.\n";

#[test]
fn body_styles_emit_bkh_when_flag_set() {
    let meta = BookMeta {
        title: "Wave6 BkH Flag On".into(),
        body_render_use_bk_styles: true,
        ..Default::default()
    };
    let bytes = render_book(&meta, &[("c1".into(), TWO_HEADING_MD.into())], Path::new("."))
        .expect("render");
    let xml = document_xml(bytes);

    assert!(
        xml.contains("w:pStyle w:val=\"BkH1\""),
        "expected BkH1 pStyle on body H1 under flag=true; got document.xml without it"
    );
    assert!(
        xml.contains("w:pStyle w:val=\"BkH2\""),
        "expected BkH2 pStyle on body H2 under flag=true; got document.xml without it"
    );
    // Backward-compat safety: under flag=true the docx-rs default
    // `Heading{1..2}` ids must NOT leak into the body — only the
    // reference style ids should be present.
    assert!(
        !xml.contains("w:pStyle w:val=\"Heading1\""),
        "flag=true unexpectedly emitted Heading1 in body — flag wiring incomplete"
    );
    assert!(
        !xml.contains("w:pStyle w:val=\"Heading2\""),
        "flag=true unexpectedly emitted Heading2 in body — flag wiring incomplete"
    );
}

#[test]
fn body_styles_emit_heading_when_flag_false() {
    let meta = BookMeta {
        title: "Wave6 BkH Flag Off".into(),
        // body_render_use_bk_styles defaults to false.
        ..Default::default()
    };
    let bytes = render_book(&meta, &[("c1".into(), TWO_HEADING_MD.into())], Path::new("."))
        .expect("render");
    let xml = document_xml(bytes);

    assert!(
        xml.contains("w:pStyle w:val=\"Heading1\""),
        "expected Heading1 pStyle on body H1 under flag=false (backward compat)"
    );
    assert!(
        xml.contains("w:pStyle w:val=\"Heading2\""),
        "expected Heading2 pStyle on body H2 under flag=false (backward compat)"
    );
    // Under flag=false the BkH style ids must NOT appear in the body —
    // otherwise non-AI-Norms books would silently switch style sets.
    assert!(
        !xml.contains("w:pStyle w:val=\"BkH1\""),
        "flag=false unexpectedly emitted BkH1 — opt-in regression"
    );
    assert!(
        !xml.contains("w:pStyle w:val=\"BkH2\""),
        "flag=false unexpectedly emitted BkH2 — opt-in regression"
    );
}
