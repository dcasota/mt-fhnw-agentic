//! `agentic check i18n` — language-interoperability coverage gate.
//!
//! A regression guard (it does NOT translate): for a target language, every
//! engine chrome i18n key must carry an EXPLICIT translation. A missing key
//! would silently fall back to English and leak English chrome into a
//! non-English build, so any gap is an ERROR.
//!
//! The chrome table is compile-time (see `agentic_core::i18n`), so this gate
//! needs no DB connection.

use agentic_core::i18n;

use crate::{CheckReport, Finding, Severity};

/// Run the i18n coverage gate.
///
/// * `Some(lang)` → check just that language (an unsupported code yields one
///   `I18N_UNSUPPORTED_LANG` error).
/// * `None` → check every supported language except English (the fallback
///   baseline).
#[must_use]
pub fn run(lang: Option<&str>) -> CheckReport {
    let mut findings = Vec::new();

    let mut langs: Vec<&str> = Vec::new();
    match lang {
        // A single target language: it must be supported, else this is a hard
        // configuration error (the gate would otherwise vacuously pass).
        Some(l) if i18n::LANGS.contains(&l) => langs.push(l),
        Some(l) => findings.push(Finding {
            category: "I18N_UNSUPPORTED_LANG".into(),
            severity: Severity::Error,
            message: format!(
                "'{l}' is not a supported language (expected one of {:?})",
                i18n::LANGS
            ),
            location: Some("i18n".into()),
        }),
        // No target: check every supported language except the English baseline.
        None => langs.extend(i18n::LANGS.iter().copied().filter(|l| *l != "en")),
    }

    let mut missing = 0usize;
    for l in &langs {
        for key in i18n::KEYS {
            if !i18n::has(l, key) {
                missing += 1;
                findings.push(Finding {
                    category: "I18N_MISSING".into(),
                    severity: Severity::Error,
                    message: format!(
                        "no '{l}' translation for chrome key '{key}' — English would leak"
                    ),
                    location: Some("i18n".into()),
                });
            }
        }
    }

    findings.push(Finding {
        category: "I18N_SUMMARY".into(),
        severity: Severity::Info,
        message: format!(
            "i18n: {} chrome keys x {} language(s) checked; {missing} missing",
            i18n::KEYS.len(),
            langs.len()
        ),
        location: Some("i18n".into()),
    });

    CheckReport::new("i18n", findings)
}

#[cfg(test)]
mod tests {
    use super::run;
    use crate::Verdict;
    use agentic_core::i18n;

    #[test]
    fn de_passes() {
        let report = run(Some("de"));
        assert_eq!(report.verdict, Verdict::Pass);
        assert!(!report.findings.iter().any(|f| f.category == "I18N_MISSING"));
    }

    #[test]
    fn unsupported_lang_fails() {
        let report = run(Some("xx"));
        assert_eq!(report.verdict, Verdict::Fail);
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.category == "I18N_UNSUPPORTED_LANG")
        );
    }

    #[test]
    fn all_langs_all_keys_seeded() {
        // Guarantees the seed table is complete: the gate's happy path holds
        // for every supported language and every chrome key.
        for lang in i18n::LANGS {
            for key in i18n::KEYS {
                assert!(i18n::has(lang, key), "missing explicit {lang}/{key}");
            }
        }
    }

    #[test]
    fn all_langs_pass() {
        let report = run(None);
        assert_eq!(report.verdict, Verdict::Pass);
        assert!(!report.findings.iter().any(|f| f.category == "I18N_MISSING"));
        // None ⇒ every supported language except the English baseline (5).
        let summary = report
            .findings
            .iter()
            .find(|f| f.category == "I18N_SUMMARY")
            .expect("summary finding");
        assert!(
            summary.message.contains("x 5 language(s)"),
            "summary was: {}",
            summary.message
        );
    }
}
