//! Identifiers, limits and the slug rules.

/// The role every tenant has from its creation: unrestricted, and the only
/// one that can rename the tenant. Custom roles come later; this name is
/// reserved.
pub const ROLE_OWNER: &str = "owner";

pub const MAX_NAME_LEN: usize = 80;
pub const MIN_SLUG_LEN: usize = 2;
pub const MAX_SLUG_LEN: usize = 32;
/// Length of the random suffix added when a derived slug is already taken.
pub const SLUG_SUFFIX_LEN: usize = 5;
/// The slug of a name with no usable characters.
pub const FALLBACK_SLUG: &str = "tenant";

/// The index stream of a slug.
pub fn tenant_slug_id(slug: &str) -> String {
    evento::hash_ids(vec!["tenant-slug", slug])
}

/// Lowercases and validates a slug: `[a-z0-9]`, `-` inside, 2 to 32 characters.
pub fn normalize_slug(slug: &str) -> Result<String, crate::TenancyError> {
    let slug = slug.trim().to_ascii_lowercase();
    let len = slug.len();
    let valid = (MIN_SLUG_LEN..=MAX_SLUG_LEN).contains(&len)
        && slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !slug.starts_with('-')
        && !slug.ends_with('-');
    if valid {
        Ok(slug)
    } else {
        Err(crate::TenancyError::InvalidSlug)
    }
}

/// The slug a name gives: ASCII-transliterated, lowercased, runs of anything
/// else collapsed into one dash, cut to [`MAX_SLUG_LEN`]. A name with nothing
/// usable gives [`FALLBACK_SLUG`].
pub fn slugify(name: &str) -> String {
    let ascii = deunicode::deunicode(name).to_ascii_lowercase();
    let mut slug = String::with_capacity(ascii.len());
    for c in ascii.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            slug.push(c);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = truncate_slug(&slug, MAX_SLUG_LEN);
    if slug.len() < MIN_SLUG_LEN {
        FALLBACK_SLUG.to_owned()
    } else {
        slug
    }
}

/// `slug` with a short random suffix (`acme-k3x9p`), kept within
/// [`MAX_SLUG_LEN`]. Used when the plain slug is taken.
pub fn suffixed_slug(slug: &str) -> String {
    let id = ulid::Ulid::generate().to_string().to_ascii_lowercase();
    let suffix = &id[id.len() - SLUG_SUFFIX_LEN..];
    let base = truncate_slug(slug, MAX_SLUG_LEN - SLUG_SUFFIX_LEN - 1);
    format!("{base}-{suffix}")
}

/// Cuts a slug to `max` bytes without leaving a dangling dash.
fn truncate_slug(slug: &str, max: usize) -> String {
    let mut slug: &str = &slug[..slug.len().min(max)];
    slug = slug.trim_matches('-');
    slug.to_owned()
}

/// Trims a display name and checks it is 1 to 80 characters.
pub fn normalize_name(name: &str) -> Result<String, crate::TenancyError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME_LEN {
        return Err(crate::TenancyError::InvalidName);
    }
    Ok(name.to_owned())
}

/// The consensus group a tenant's database runs in, derived from its id so
/// every node agrees without being told: FNV-1a over the id, never `0`
/// (the platform's own group).
pub fn group_of(tenant_id: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in tenant_id.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_are_lowercased_and_checked() {
        assert_eq!(normalize_slug(" Acme-Shop ").unwrap(), "acme-shop");
        assert_eq!(normalize_slug("a1").unwrap(), "a1");
        for bad in ["", "a", "-acme", "acme-", "ac me", "acmé", &"a".repeat(33)] {
            assert!(normalize_slug(bad).is_err(), "{bad:?}");
        }
        assert!(normalize_slug(&"a".repeat(32)).is_ok());
    }

    #[test]
    fn slugs_derive_from_names() {
        assert_eq!(slugify("Acme Shop"), "acme-shop");
        assert_eq!(slugify("  Épicerie du Coin !  "), "epicerie-du-coin");
        assert_eq!(slugify("Straße 7 / Köln"), "strasse-7-koln");
        assert_eq!(slugify("--Acme--"), "acme");
        assert_eq!(slugify("!!!"), FALLBACK_SLUG);
        assert_eq!(slugify("a"), FALLBACK_SLUG);
        let long = slugify(&"ab-".repeat(20));
        assert!(long.len() <= MAX_SLUG_LEN && !long.ends_with('-'), "{long}");
        assert!(normalize_slug(&long).is_ok());
        assert!(normalize_slug(&slugify("日本語の店")).is_ok());
    }

    #[test]
    fn suffixed_slugs_stay_valid_and_short() {
        let s = suffixed_slug("acme");
        assert_eq!(s.len(), "acme-".len() + SLUG_SUFFIX_LEN, "{s}");
        assert!(s.starts_with("acme-"));
        assert!(normalize_slug(&s).is_ok());
        let long = suffixed_slug(&"a".repeat(MAX_SLUG_LEN));
        assert_eq!(long.len(), MAX_SLUG_LEN, "{long}");
        assert!(normalize_slug(&long).is_ok());
        assert_ne!(suffixed_slug("acme"), suffixed_slug("acme"));
    }

    #[test]
    fn names_are_trimmed_and_bounded() {
        assert_eq!(normalize_name("  Acme  ").unwrap(), "Acme");
        assert!(normalize_name("   ").is_err());
        assert!(normalize_name(&"é".repeat(80)).is_ok());
        assert!(normalize_name(&"é".repeat(81)).is_err());
    }

    #[test]
    fn groups_are_stable_and_never_zero() {
        assert_eq!(group_of("01J9"), group_of("01J9"));
        assert_ne!(group_of("a"), group_of("b"));
        assert_ne!(group_of(""), 0);
    }

    #[test]
    fn slug_ids_are_hashed() {
        assert_eq!(tenant_slug_id("acme").len(), 64);
        assert_ne!(tenant_slug_id("acme"), tenant_slug_id("acme2"));
    }
}
