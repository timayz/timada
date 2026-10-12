//! Identifiers, limits, the slug and role rules, and the permission catalog.

use std::collections::BTreeSet;

/// The role every tenant has from its creation: unrestricted, implicit (no
/// `RoleDefined` event), and reserved: a custom role cannot take this name.
pub const ROLE_OWNER: &str = "owner";

/// Every permission a custom role can grant, as `resource:action`. The list
/// only grows: a stored role may name any of them. `manage` on a resource
/// implies `read` on it (see [`implies`]).
///
/// - `tenant:manage` — rename the tenant (its settings).
/// - `members:read` — see the members and the pending invitations.
/// - `members:manage` — invite, change roles and remove members (never to or
///   from `owner`: that stays with owners).
/// - `roles:manage` — define, edit and remove custom roles.
/// - `api-keys:manage` — reserved for the store API keys (not built yet).
pub const PERMISSIONS: &[&str] = &[
    PERMISSION_TENANT_MANAGE,
    PERMISSION_MEMBERS_READ,
    PERMISSION_MEMBERS_MANAGE,
    PERMISSION_ROLES_MANAGE,
    PERMISSION_API_KEYS_MANAGE,
];
pub const PERMISSION_TENANT_MANAGE: &str = "tenant:manage";
pub const PERMISSION_MEMBERS_READ: &str = "members:read";
pub const PERMISSION_MEMBERS_MANAGE: &str = "members:manage";
pub const PERMISSION_ROLES_MANAGE: &str = "roles:manage";
pub const PERMISSION_API_KEYS_MANAGE: &str = "api-keys:manage";

pub const MAX_ROLE_LABEL_LEN: usize = 40;
/// How long an invitation link stays valid: seven days.
pub const INVITATION_TTL_SECS: u64 = 7 * 24 * 3600;
pub const MAX_EMAIL_LEN: usize = 254;

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

/// Whether `permission` is one of [`PERMISSIONS`].
pub fn is_permission(permission: &str) -> bool {
    PERMISSIONS.contains(&permission)
}

/// A permission as `(resource, action)` (casbin's object and action), or
/// `None` for anything outside [`PERMISSIONS`].
pub fn split_permission(permission: &str) -> Option<(&str, &str)> {
    is_permission(permission)
        .then(|| permission.split_once(':'))
        .flatten()
}

/// Whether holding `held` grants `wanted`: the same permission, or `manage`
/// on the resource `wanted` only reads.
pub fn implies(held: &str, wanted: &str) -> bool {
    if held == wanted {
        return true;
    }
    match (split_permission(held), split_permission(wanted)) {
        (Some((resource, "manage")), Some((wanted_resource, "read"))) => {
            resource == wanted_resource
        }
        _ => false,
    }
}

/// Deduplicates and checks a permission list against [`PERMISSIONS`].
pub fn normalize_permissions<I>(permissions: I) -> Result<BTreeSet<String>, crate::TenancyError>
where
    I: IntoIterator,
    I::Item: AsRef<str>,
{
    permissions
        .into_iter()
        .map(|p| {
            let p = p.as_ref().trim();
            if is_permission(p) {
                Ok(p.to_owned())
            } else {
                Err(crate::TenancyError::InvalidPermission)
            }
        })
        .collect()
}

/// Lowercases and validates a custom role name: the slug rules, and never
/// [`ROLE_OWNER`].
pub fn normalize_role(role: &str) -> Result<String, crate::TenancyError> {
    let role = normalize_slug(role).map_err(|_| crate::TenancyError::InvalidRole)?;
    if role == ROLE_OWNER {
        return Err(crate::TenancyError::ReservedRole);
    }
    Ok(role)
}

/// Trims a role label and checks it is 1 to 40 characters.
pub fn normalize_role_label(label: &str) -> Result<String, crate::TenancyError> {
    let label = label.trim();
    if label.is_empty() || label.chars().count() > MAX_ROLE_LABEL_LEN {
        return Err(crate::TenancyError::InvalidRoleLabel);
    }
    Ok(label.to_owned())
}

/// The canonical form of an address: trimmed, lower-cased, one `@` with
/// something on both sides, nothing that is not printable. (The identity
/// context has the same rule; the two crates do not depend on each other.)
pub fn normalize_email(email: &str) -> Result<String, crate::TenancyError> {
    let email = email.trim().to_lowercase();
    let valid = email.len() <= MAX_EMAIL_LEN
        && !email.chars().any(|c| c.is_whitespace() || c.is_control())
        && matches!(email.split_once('@'), Some((local, domain))
            if !local.is_empty() && !domain.is_empty() && !domain.contains('@'));
    if valid {
        Ok(email)
    } else {
        Err(crate::TenancyError::InvalidEmail)
    }
}

/// The stream of the invitation a link token opens.
pub fn invitation_id(token: &str) -> String {
    evento::hash_ids(vec!["invitation", token])
}

/// A fresh invitation token: 32 random bytes, URL-safe base64 without
/// padding (43 characters), from the thread generator (seeded by the OS).
pub fn generate_invitation_token() -> String {
    use base64::Engine;
    use rand::RngCore;

    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
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

    #[test]
    fn permissions_are_split_and_checked() {
        assert_eq!(
            split_permission("members:manage"),
            Some(("members", "manage"))
        );
        assert_eq!(split_permission("members:delete"), None);
        assert!(PERMISSIONS.iter().all(|p| split_permission(p).is_some()));
        let set =
            normalize_permissions(["roles:manage", " members:read ", "roles:manage"]).unwrap();
        assert_eq!(set.len(), 2);
        assert!(normalize_permissions(["nope"]).is_err());
        assert!(
            normalize_permissions(Vec::<String>::new())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn manage_implies_read_on_the_same_resource() {
        assert!(implies("members:manage", "members:read"));
        assert!(implies("members:read", "members:read"));
        assert!(!implies("members:read", "members:manage"));
        assert!(!implies("roles:manage", "members:read"));
        assert!(!implies("members:manage", "members:delete"));
    }

    #[test]
    fn roles_follow_the_slug_rules_and_owner_is_reserved() {
        assert_eq!(normalize_role(" Support ").unwrap(), "support");
        assert!(matches!(
            normalize_role("Owner"),
            Err(crate::TenancyError::ReservedRole)
        ));
        assert!(matches!(
            normalize_role("a"),
            Err(crate::TenancyError::InvalidRole)
        ));
        assert!(matches!(
            normalize_role("sup port"),
            Err(crate::TenancyError::InvalidRole)
        ));
        assert_eq!(
            normalize_role_label("  Support team ").unwrap(),
            "Support team"
        );
        assert!(normalize_role_label(" ").is_err());
        assert!(normalize_role_label(&"x".repeat(41)).is_err());
    }

    #[test]
    fn emails_are_normalized() {
        assert_eq!(
            normalize_email(" Ada@Example.COM ").unwrap(),
            "ada@example.com"
        );
        for bad in [
            "",
            "ada",
            "@example.com",
            "ada@",
            "a@b@c",
            "a d@example.com",
        ] {
            assert!(normalize_email(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn invitation_tokens_are_random_and_hashed() {
        let token = generate_invitation_token();
        assert_eq!(token.len(), 43, "{token}");
        assert!(
            token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        );
        assert_ne!(token, generate_invitation_token());
        assert_eq!(invitation_id(&token).len(), 64);
        assert_ne!(invitation_id("a"), invitation_id("b"));
    }
}
