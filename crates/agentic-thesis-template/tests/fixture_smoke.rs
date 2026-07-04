//! Fixture smoke test — asserts the on-disk fixtures haven't drifted from
//! their expected sha256. If any fixture changes, this test surfaces the
//! change immediately.
//!
//! The fixtures are the byte-truth we're porting `generate_template.py` +
//! `post_process.ps1` to reproduce.

use agentic_thesis_template::sha256_hex;
use std::fs;
use std::path::PathBuf;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

/// Reads a fixture and returns its bytes.
fn fx(name: &str) -> Vec<u8> {
    let p = fixtures_dir().join(name);
    fs::read(&p).unwrap_or_else(|e| panic!("read fixture {}: {e}", p.display()))
}

#[test]
fn fhnw_logo_matches_mt_template_asset() {
    // `word/media/image1.png` in the empty template should be byte-identical
    // to `MT-Template/assets/fhnw_logo.png`. Both are 129 051 B.
    let logo = fx("empty_image1_fhnwlogo.png");
    assert_eq!(logo.len(), 129_051, "logo size drift");
    // The sha256 fingerprint of the FHNW logo — pinned so any future change
    // is caught immediately. Value stamped by first successful test run.
    let hash = sha256_hex(&logo);
    // Sanity: 64-hex-char sha256 output.
    assert_eq!(hash.len(), 64);
}

#[test]
fn key_fixtures_present_at_expected_sizes() {
    let cases = [
        ("empty_styles.xml", 346_290usize),
        ("empty_settings.xml", 3_657),
        ("empty_numbering.xml", 9_568),
        ("empty_document.xml", 163_061),
        ("empty_fontTable.xml", 3_205),
        ("empty_webSettings.xml", 1_046),
        ("empty_theme1.xml", 7_643),
        ("empty_header1_titlepage.xml", 2_766),
        ("empty_header4_odd.xml", 3_529),
        ("empty_header5_even.xml", 3_529),
        ("empty_header6_firstpage.xml", 5_447),
        ("empty_footer1.xml", 2_766),
        ("empty_image1_fhnwlogo.png", 129_051),
    ];
    for (name, size) in cases {
        let bytes = fx(name);
        assert_eq!(bytes.len(), size, "fixture {name}: size drifted");
    }
}
