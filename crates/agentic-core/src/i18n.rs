//! Runtime i18n for the engine "chrome" — DB-free, compile-time string table.
//!
//! The book renderer has no DB connection, so localisation of engine-generated
//! chrome (admonition labels, caption prefixes, list/section headings) is backed
//! by a static `match` table here. Content (chapter text) is never touched — it
//! stays in whatever language the chapters are authored in.
//!
//! Lookup contract (never panics, never renders empty):
//! - `lang` is matched case-insensitively;
//! - an unknown `lang` falls back to `"en"`;
//! - an unknown `key` falls back to the key itself.
//!
//! NOTE: the `rm` (Romansh) and `hi` (Hindi/Devanagari) translations are
//! best-effort SEED VALUES pending native-speaker review.

/// Translate an engine chrome `key` for the given `lang`.
///
/// Returns a `&'static str`: a localised value, the English fallback, or the
/// key itself when the key is unknown.
#[must_use]
pub fn t(lang: &str, key: &str) -> &'static str {
    let lang = normalise_lang(lang);
    // `lookup` returns `None` only for an unknown key (every known key has a
    // value for every supported lang, and unknown langs already collapsed to
    // "en"). Fall back to the key itself so nothing renders empty.
    lookup(lang, key).unwrap_or_else(|| {
        // A `&'static str` cannot borrow the caller's `key`, so promote it to a
        // leaked static. This path is unreachable for the engine's literal keys;
        // it exists only to honour the "unknown key → key" contract for tests /
        // misuse. Leaking a handful of short strings is acceptable for that.
        Box::leak(key.to_string().into_boxed_str())
    })
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
        _ => "en",
    }
}

/// The compile-time table. `lang` is already normalised to a supported code.
/// Returns `None` for an unknown key so the caller can fall back to the key.
fn lookup(lang: &str, key: &str) -> Option<&'static str> {
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
    use super::t;

    #[test]
    fn german_note() {
        assert_eq!(t("de", "note"), "Hinweis");
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
