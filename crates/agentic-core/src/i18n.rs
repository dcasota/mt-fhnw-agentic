//! Runtime i18n for the engine "chrome" — DB-free, compile-time string table.
//!
//! The book renderer has no DB connection, so localisation of engine-generated
//! chrome (admonition labels, caption prefixes, list/section headings) is backed
//! by a static `match` table here. Content (chapter text) is never touched — it
//! stays in whatever language the chapters are authored in.
//!
//! Lookup contract (never panics, never renders empty):
//! - `lang` is matched case-insensitively on its primary subtag;
//! - [`t`] falls back through English to the key itself;
//! - [`has`] reports only EXPLICIT entries (no fallback) so a regression gate
//!   can catch keys that would silently leak English chrome.
//!
//! NOTE: the `rm` (Romansh) and `hi` (Hindi/Devanagari) translations are
//! best-effort SEED VALUES pending native-speaker review.

/// All engine chrome keys backed by the compile-time table.
///
/// A language-interoperability gate (`agentic check i18n`) iterates this list
/// to assert that every key has an EXPLICIT translation in a target language.
pub const KEYS: &[&str] = &[
    "fig_prefix",
    "table_prefix",
    "list_of_figures",
    "list_of_tables",
    "sources_box",
    "edition_disclaimer",
    "conventions_title",
    "note",
    "tip",
    "warning",
];

/// Supported language codes, English first.
pub const LANGS: &[&str] = &["en", "de", "fr", "it", "rm", "hi"];

/// Translate an engine chrome `key` for the given `lang`.
///
/// Returns a `&'static str`: a localised value, the English fallback, or the
/// key itself when the key is unknown.
#[must_use]
pub fn t(lang: &str, key: &str) -> &'static str {
    // Explicit entry → English fallback → the key itself (so nothing renders
    // empty). `has()` distinguishes the explicit case from the fallback.
    lookup(lang, key)
        .or_else(|| lookup("en", key))
        .unwrap_or_else(|| {
            // A `&'static str` cannot borrow the caller's `key`, so promote it
            // to a leaked static. This path is unreachable for the engine's
            // literal keys; it exists only to honour the "unknown key → key"
            // contract for tests / misuse. Leaking a handful of short strings
            // is acceptable for that.
            Box::leak(key.to_string().into_boxed_str())
        })
}

/// Is there an EXPLICIT translation for `key` in `lang`?
///
/// Unlike [`t`], this does NOT fall back to English: it reports whether the
/// table carries a real entry for the (normalised) language. The i18n gate
/// uses it to catch keys that would silently leak English chrome.
#[must_use]
pub fn has(lang: &str, key: &str) -> bool {
    lookup(lang, key).is_some()
}

/// Case-insensitively reduce a lang tag to one of the supported codes, or "en".
fn normalise_lang(lang: &str) -> &'static str {
    // Only inspect the primary subtag (e.g. "de-CH" → "de").
    let primary = lang.split(['-', '_']).next().unwrap_or("");
    match primary.to_ascii_lowercase().as_str() {
        "de" => "de",
        "fr" => "fr",
        "it" => "it",
        "rm" => "rm",
        "hi" => "hi",
        "en" => "en",
        _ => "",
    }
}

/// The compile-time table. Returns `None` for an unknown key OR an unsupported
/// `lang` (so callers can distinguish an explicit entry from a fallback).
fn lookup(lang: &str, key: &str) -> Option<&'static str> {
    let lang = normalise_lang(lang);
    if lang.is_empty() {
        return None;
    }
    // Each arm lists en/de/fr/it/rm/hi in that order.
    let row: [&'static str; 6] = match key {
        "fig_prefix" => [
            "Figure ",
            "Abbildung ",
            "Figure ",
            "Figura ",
            "Figura ",
            "चित्र ",
        ],
        "table_prefix" => [
            "Table ",
            "Tabelle ",
            "Tableau ",
            "Tabella ",
            "Tabella ",
            "तालिका ",
        ],
        "list_of_figures" => [
            "List of Figures",
            "Abbildungsverzeichnis",
            "Liste des figures",
            "Elenco delle figure",
            "Glista da las figuras",
            "चित्र सूची",
        ],
        "list_of_tables" => [
            "List of Tables",
            "Tabellenverzeichnis",
            "Liste des tableaux",
            "Elenco delle tabelle",
            "Glista da las tabellas",
            "तालिका सूची",
        ],
        "sources_box" => [
            "Sources & QR codes",
            "Quellen & QR-Codes",
            "Sources et codes QR",
            "Fonti e codici QR",
            "Funtaunas & codes QR",
            "स्रोत और QR कोड",
        ],
        "edition_disclaimer" => [
            "Edition & Disclaimer",
            "Ausgabe & Haftungsausschluss",
            "Édition et avertissement",
            "Edizione e avvertenze",
            "Ediziun & disclaimer",
            "संस्करण और अस्वीकरण",
        ],
        "conventions_title" => [
            "Conventions Used in This Book",
            "In diesem Buch verwendete Konventionen",
            "Conventions utilisées dans ce livre",
            "Convenzioni usate in questo libro",
            "Convenziuns duvradas en quest cudesch",
            "इस पुस्तक में प्रयुक्त परंपराएँ",
        ],
        "note" => ["Note", "Hinweis", "Note", "Nota", "Nota", "टिप्पणी"],
        "tip" => ["Tip", "Tipp", "Astuce", "Suggerimento", "Tip", "सुझाव"],
        "warning" => [
            "Warning",
            "Warnung",
            "Avertissement",
            "Avviso",
            "Avertiment",
            "चेतावनी",
        ],
        _ => return None,
    };
    let idx = match lang {
        "de" => 1,
        "fr" => 2,
        "it" => 3,
        "rm" => 4,
        "hi" => 5,
        _ => 0,
    };
    Some(row[idx])
}

#[cfg(test)]
mod tests {
    use super::{KEYS, LANGS, has, t};

    #[test]
    fn german_note() {
        assert_eq!(t("de", "note"), "Hinweis");
    }

    #[test]
    fn has_true_for_explicit_de_entry() {
        assert!(has("de", "note"));
        // Case-insensitive on the primary subtag.
        assert!(has("DE-CH", "fig_prefix"));
    }

    #[test]
    fn has_true_even_when_translation_equals_english() {
        // fr `fig_prefix` is literally "Figure " (same bytes as en) — but it is
        // still an EXPLICIT seeded entry, so `has` must report true.
        assert_eq!(t("fr", "fig_prefix"), "Figure ");
        assert!(has("fr", "fig_prefix"));
    }

    #[test]
    fn has_false_for_unknown_key() {
        assert!(!has("de", "nonexistent_key"));
    }

    #[test]
    fn has_false_for_unsupported_lang() {
        assert!(!has("xx", "note"));
    }

    #[test]
    fn keys_and_langs_constants_complete() {
        assert_eq!(LANGS, &["en", "de", "fr", "it", "rm", "hi"]);
        for lang in LANGS {
            for key in KEYS {
                assert!(has(lang, key), "missing explicit {lang}/{key}");
            }
        }
    }

    #[test]
    fn case_insensitive_lang() {
        assert_eq!(t("DE", "fig_prefix"), "Abbildung ");
    }

    #[test]
    fn unknown_lang_falls_back_to_english() {
        assert_eq!(t("xx", "note"), "Note");
    }

    #[test]
    fn unknown_key_returns_key() {
        assert_eq!(t("de", "nonexistent_key"), "nonexistent_key");
    }

    #[test]
    fn all_supported_langs_have_values() {
        for lang in ["en", "de", "fr", "it", "rm", "hi"] {
            for key in [
                "fig_prefix",
                "table_prefix",
                "list_of_figures",
                "list_of_tables",
                "sources_box",
                "edition_disclaimer",
                "conventions_title",
                "note",
                "tip",
                "warning",
            ] {
                assert!(!t(lang, key).is_empty(), "{lang}/{key} empty");
            }
        }
    }
}
