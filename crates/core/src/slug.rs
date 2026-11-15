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

/// How many former slugs are remembered for one thing: the oldest is
/// forgotten past that, so a thing renamed for years keeps a short list.
pub const MAX_FORMER_SLUGS: usize = 20;

/// A slug something went by before a rename, and when it left it (unix
/// seconds): what an old address is answered from, with a redirect to the
/// new one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormerSlug {
    pub slug: String,
    pub left_at: u64,
}

/// Notes that the thing left `old` for `new` at `at`: `old` joins `former`
/// (once, at its latest leaving), `new` leaves it when the thing went by it
/// before, and the oldest go past [`MAX_FORMER_SLUGS`]. The same slug is
/// no move.
pub fn leave_slug(former: &mut Vec<FormerSlug>, old: &str, new: &str, at: u64) {
    if old == new {
        return;
    }
    former.retain(|was| was.slug != old && was.slug != new);
    former.push(FormerSlug {
        slug: old.to_owned(),
        left_at: at,
    });
    if former.len() > MAX_FORMER_SLUGS {
        former.drain(..former.len() - MAX_FORMER_SLUGS);
    }
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

    #[test]
    fn a_left_slug_is_remembered_once_and_forgotten_when_taken_back() {
        let mut former = Vec::new();
        leave_slug(&mut former, "mug", "mug", 1);
        assert!(former.is_empty(), "the same slug is no move");
        leave_slug(&mut former, "mug", "cup", 2);
        leave_slug(&mut former, "cup", "pot", 3);
        let slugs = |former: &[FormerSlug]| -> Vec<(String, u64)> {
            former.iter().map(|f| (f.slug.clone(), f.left_at)).collect()
        };
        assert_eq!(
            slugs(&former),
            [("mug".to_owned(), 2), ("cup".to_owned(), 3)]
        );
        // Back to an old one: it is the slug again, not a former one.
        leave_slug(&mut former, "pot", "mug", 4);
        assert_eq!(
            slugs(&former),
            [("cup".to_owned(), 3), ("pot".to_owned(), 4)]
        );
        // Left again: remembered at its latest leaving.
        leave_slug(&mut former, "mug", "cup", 5);
        assert_eq!(
            slugs(&former),
            [("pot".to_owned(), 4), ("mug".to_owned(), 5)]
        );

        for n in 0..(MAX_FORMER_SLUGS as u64 + 5) {
            leave_slug(
                &mut former,
                &format!("s{n}"),
                &format!("s{}", n + 1),
                10 + n,
            );
        }
        assert_eq!(former.len(), MAX_FORMER_SLUGS);
        assert_eq!(
            former.last().unwrap().slug,
            format!("s{}", MAX_FORMER_SLUGS + 4)
        );
    }
}
