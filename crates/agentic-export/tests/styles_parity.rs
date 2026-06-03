//! Wave-2 styles-parity test (AI-Norms reference, ADR-0054 v1, 2026-06-03).
//!
//! Asserts that [`agentic_export::styles_xml::emit_styles_xml`] returns the
//! reference `word/styles.xml` byte-for-byte and that all 16 body-USED styles
//! resolve. The full-corpus byte-for-byte check is the strongest gate
//! available — any drift (whitespace, attribute reorder, encoded entities)
//! fails this test, which is the desired behaviour for a parity contract.

use std::fs;
use std::path::PathBuf;

use agentic_export::styles_xml::{
    USED_STYLE_IDS, all_used_styles_present, count_styles, emit_styles_xml,
};

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("styles_reference.xml")
}

#[test]
fn emitted_styles_xml_matches_reference_byte_for_byte() {
    let emitted = emit_styles_xml();
    let reference = fs::read_to_string(fixture_path()).expect("read fixture");
    assert_eq!(
        emitted.len(),
        reference.len(),
        "emitted styles.xml length differs from reference"
    );
    assert_eq!(
        emitted, reference,
        "emitted styles.xml is not byte-identical to reference"
    );
}

#[test]
fn emitted_styles_xml_declares_exactly_186_styles() {
    assert_eq!(count_styles(emit_styles_xml()), 186);
}

#[test]
fn emitted_styles_xml_contains_all_used_styles() {
    let xml = emit_styles_xml();
    assert!(
        all_used_styles_present(xml),
        "at least one USED style id is missing from the emitted styles.xml"
    );
    // Individual presence assertions for the body-USED set so a failure
    // log names the offending style id, not just "all_used_styles_present
    // returned false". Mirrors the Wave-2 inventory exactly.
    for id in USED_STYLE_IDS {
        let needle = format!("w:styleId=\"{id}\"");
        assert!(
            xml.contains(&needle),
            "USED style id `{id}` not found in emitted styles.xml"
        );
    }
}

#[test]
fn used_style_ids_constant_lists_16_entries() {
    // Wave-2 inventory locked at 16 body-USED styles. If this changes the
    // inventory must be re-run and the constant updated together with the
    // parity assertion — fail loudly if they drift.
    assert_eq!(USED_STYLE_IDS.len(), 16);
}
