//! Slugs: the URL names of tenants, products, categories and pages. Every
//! context keeps its own length, fallback and refusal; the rules are one.

/// The shortest slug.
pub const MIN_SLUG_LEN: usize = 2;
/// The random suffix [`suffixed_slug`] adds, in characters.
pub const SLUG_SUFFIX_LEN: usize = 5;

/// Lowercases and checks a slug: `[a-z0-9]` with `-` inside, from
/// [`MIN_SLUG_LEN`] to `max_len` bytes; `None` otherwise.
pub fn normalize_slug(slug: &str, max_len: usize) -> Option<String> {
    let slug = slug.trim().to_ascii_lowercase();
    let len = slug.len();
    let valid = (MIN_SLUG_LEN..=max_len).contains(&len)
        && slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !slug.starts_with('-')
        && !slug.ends_with('-');
    valid.then_some(slug)
}

/// The slug a name gives: ASCII-transliterated, lowercased, runs of anything
/// else collapsed into one dash, cut to `max_len`. A name with nothing usable
/// gives `fallback`.
pub fn slugify(name: &str, max_len: usize, fallback: &str) -> String {
    let ascii = deunicode::deunicode(name).to_ascii_lowercase();
    let mut slug = String::with_capacity(ascii.len());
    for c in ascii.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            slug.push(c);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = truncate_slug(&slug, max_len);
    if slug.len() < MIN_SLUG_LEN {
        fallback.to_owned()
    } else {
        slug
    }
}

/// `slug` with a short random suffix (`blue-mug-k3x9p`), kept within
/// `max_len`. For when the plain slug is taken.
pub fn suffixed_slug(slug: &str, max_len: usize) -> String {
    let id = ulid::Ulid::generate().to_string().to_ascii_lowercase();
    let suffix = &id[id.len() - SLUG_SUFFIX_LEN..];
    let base = truncate_slug(slug, max_len - SLUG_SUFFIX_LEN - 1);
    format!("{base}-{suffix}")
}

/// Cuts a slug to `max` bytes without leaving a dangling dash.
pub fn truncate_slug(slug: &str, max: usize) -> String {
    slug[..slug.len().min(max)].trim_matches('-').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_are_made_checked_cut_and_suffixed() {
        assert_eq!(slugify("Thé vert — bio!", 64, "product"), "the-vert-bio");
        assert_eq!(slugify("???", 64, "product"), "product");
        assert_eq!(slugify("a-very-long-name-indeed", 10, "x"), "a-very-lon");
        assert_eq!(
            normalize_slug(" Blue-Mug ", 64).as_deref(),
            Some("blue-mug")
        );
        for bad in ["", "a", "-mug", "mug-", "blue mug", "blue_mug"] {
            assert_eq!(normalize_slug(bad, 64), None, "{bad:?}");
        }
        assert_eq!(normalize_slug("abcdefghij", 8), None);
        let suffixed = suffixed_slug("blue-mug", 12);
        assert_eq!(suffixed.len(), 12, "{suffixed}");
        assert!(suffixed.starts_with("blue-m-"), "{suffixed}");
        assert_ne!(suffixed_slug("blue-mug", 64), suffixed_slug("blue-mug", 64));
    }
}
