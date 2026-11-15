//! What a page tells search engines about itself: a title and a
//! description of its own, apart from the ones a visitor reads.

/// What search engines show of a title before cutting it.
pub const MAX_META_TITLE_LEN: usize = 70;
/// What search engines show of a description before cutting it.
pub const MAX_META_DESCRIPTION_LEN: usize = 160;

/// Trims the search engines' title, puts their description on one line and
/// checks both lengths; `None` when either is too long. Empty is fine: what
/// the page shows a visitor stands in.
pub fn normalize_seo(meta_title: &str, meta_description: &str) -> Option<(String, String)> {
    let meta_title = meta_title.trim();
    let meta_description = meta_description
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let fits = meta_title.chars().count() <= MAX_META_TITLE_LEN
        && meta_description.chars().count() <= MAX_META_DESCRIPTION_LEN;
    fits.then(|| (meta_title.to_owned(), meta_description))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_description_is_put_on_one_line() {
        assert_eq!(
            normalize_seo(" Mats | Acme ", "  Soft  mats\nfor cats. "),
            Some(("Mats | Acme".to_owned(), "Soft mats for cats.".to_owned()))
        );
        assert_eq!(normalize_seo("", ""), Some((String::new(), String::new())));
    }

    #[test]
    fn what_a_search_engine_would_cut_is_refused() {
        assert!(normalize_seo(&"x".repeat(MAX_META_TITLE_LEN), "").is_some());
        assert!(normalize_seo(&"x".repeat(MAX_META_TITLE_LEN + 1), "").is_none());
        assert!(normalize_seo("", &"é".repeat(MAX_META_DESCRIPTION_LEN)).is_some());
        assert!(normalize_seo("", &"é".repeat(MAX_META_DESCRIPTION_LEN + 1)).is_none());
    }
}
