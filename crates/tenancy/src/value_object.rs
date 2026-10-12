//! Identifiers, limits and the slug rules.

/// The role every tenant has from its creation: unrestricted, and the only
/// one that can rename the tenant. Custom roles come later; this name is
/// reserved.
pub const ROLE_OWNER: &str = "owner";

pub const MAX_NAME_LEN: usize = 80;
pub const MIN_SLUG_LEN: usize = 2;
pub const MAX_SLUG_LEN: usize = 32;

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
