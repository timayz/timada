//! The languages a shop may word its mails in.

/// The language a shop mails in until it picks its own, and the one the
/// built-in wording is written in.
pub const DEFAULT_LANGUAGE: &str = "en";

/// The languages a shop may pick, as ISO 639-1 codes with their English
/// names, in the order the pickers list them.
pub const LANGUAGES: &[(&str, &str)] = &[
    ("en", "English"),
    ("fr", "French"),
    ("de", "German"),
    ("es", "Spanish"),
    ("it", "Italian"),
    ("pt", "Portuguese"),
    ("nl", "Dutch"),
    ("pl", "Polish"),
    ("sv", "Swedish"),
    ("da", "Danish"),
    ("fi", "Finnish"),
    ("no", "Norwegian"),
    ("cs", "Czech"),
    ("sk", "Slovak"),
    ("hu", "Hungarian"),
    ("ro", "Romanian"),
    ("bg", "Bulgarian"),
    ("el", "Greek"),
    ("hr", "Croatian"),
    ("sl", "Slovenian"),
    ("et", "Estonian"),
    ("lv", "Latvian"),
    ("lt", "Lithuanian"),
    ("ca", "Catalan"),
    ("tr", "Turkish"),
    ("uk", "Ukrainian"),
    ("ru", "Russian"),
    ("ar", "Arabic"),
    ("he", "Hebrew"),
    ("hi", "Hindi"),
    ("id", "Indonesian"),
    ("vi", "Vietnamese"),
    ("th", "Thai"),
    ("zh", "Chinese"),
    ("ja", "Japanese"),
    ("ko", "Korean"),
];

/// The English name of a language a shop may pick; `None` for any other code.
pub fn language_name(code: &str) -> Option<&'static str> {
    LANGUAGES
        .iter()
        .find(|(known, _)| *known == code)
        .map(|(_, name)| *name)
}

/// Whether `code` is a language a shop may pick.
pub fn is_language(code: &str) -> bool {
    language_name(code).is_some()
}

/// Where `code` comes in [`LANGUAGES`], for a stable order; unknown codes last.
pub fn language_rank(code: &str) -> usize {
    LANGUAGES
        .iter()
        .position(|(known, _)| *known == code)
        .unwrap_or(LANGUAGES.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_names_each_language_once() {
        let mut codes: Vec<_> = LANGUAGES.iter().map(|(code, _)| *code).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), LANGUAGES.len());
        assert!(
            LANGUAGES
                .iter()
                .all(|(code, _)| code.len() == 2 && code.bytes().all(|b| b.is_ascii_lowercase()))
        );
        assert_eq!(language_name(DEFAULT_LANGUAGE), Some("English"));
        assert_eq!(language_name("fr"), Some("French"));
        assert_eq!(language_name("FR"), None);
        assert!(!is_language("zz"));
        assert!(language_rank("en") < language_rank("fr"));
        assert_eq!(language_rank("zz"), LANGUAGES.len());
    }
}
