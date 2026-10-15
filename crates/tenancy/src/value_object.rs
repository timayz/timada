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
/// - `api-keys:manage` — create and revoke the store API keys.
/// - `products:read` — see the products of the catalog.
/// - `products:manage` — create, edit, publish and archive products.
/// - `orders:read` — see the orders shoppers placed and the carts they left.
/// - `orders:manage` — mark orders fulfilled, and cancel (refund) them.
/// - `payments:manage` — set up how shoppers pay (the provider's keys).
/// - `discounts:read` — see the discount codes.
/// - `discounts:manage` — create, deactivate and archive discount codes.
/// - `reviews:read` — see the reviews shoppers wrote.
/// - `reviews:manage` — publish and reject reviews, take photos off them.
/// - `customers:read` — see the shop's customer accounts.
/// - `mail:manage` — set up the mail server the shop's mails go out through.
pub const PERMISSIONS: &[&str] = &[
    PERMISSION_TENANT_MANAGE,
    PERMISSION_MEMBERS_READ,
    PERMISSION_MEMBERS_MANAGE,
    PERMISSION_ROLES_MANAGE,
    PERMISSION_API_KEYS_MANAGE,
    PERMISSION_PRODUCTS_READ,
    PERMISSION_PRODUCTS_MANAGE,
    PERMISSION_ORDERS_READ,
    PERMISSION_ORDERS_MANAGE,
    PERMISSION_PAYMENTS_MANAGE,
    PERMISSION_DISCOUNTS_READ,
    PERMISSION_DISCOUNTS_MANAGE,
    PERMISSION_REVIEWS_READ,
    PERMISSION_REVIEWS_MANAGE,
    PERMISSION_CUSTOMERS_READ,
    PERMISSION_MAIL_MANAGE,
];
pub const PERMISSION_TENANT_MANAGE: &str = "tenant:manage";
pub const PERMISSION_MEMBERS_READ: &str = "members:read";
pub const PERMISSION_MEMBERS_MANAGE: &str = "members:manage";
pub const PERMISSION_ROLES_MANAGE: &str = "roles:manage";
pub const PERMISSION_API_KEYS_MANAGE: &str = "api-keys:manage";
pub const PERMISSION_PRODUCTS_READ: &str = "products:read";
pub const PERMISSION_PRODUCTS_MANAGE: &str = "products:manage";
pub const PERMISSION_ORDERS_READ: &str = "orders:read";
pub const PERMISSION_ORDERS_MANAGE: &str = "orders:manage";
pub const PERMISSION_PAYMENTS_MANAGE: &str = "payments:manage";
pub const PERMISSION_DISCOUNTS_READ: &str = "discounts:read";
pub const PERMISSION_DISCOUNTS_MANAGE: &str = "discounts:manage";
pub const PERMISSION_REVIEWS_READ: &str = "reviews:read";
pub const PERMISSION_REVIEWS_MANAGE: &str = "reviews:manage";
pub const PERMISSION_CUSTOMERS_READ: &str = "customers:read";
pub const PERMISSION_MAIL_MANAGE: &str = "mail:manage";

/// Every scope a store API key can hold, as `resource:action`. The list only
/// grows: a stored key may name any of them. `write` on a resource implies
/// `read` on it, and `store:write` implies `store:checkout` (see
/// [`api_key_scope_implies`]).
pub const API_KEY_SCOPES: &[&str] = &[SCOPE_STORE_READ, SCOPE_STORE_WRITE, SCOPE_STORE_CHECKOUT];
/// Read the tenant's events and run subscriptions over them.
pub const SCOPE_STORE_READ: &str = "store:read";
/// Append events to the tenant's store.
pub const SCOPE_STORE_WRITE: &str = "store:write";
/// Append what a shopper does and nothing else: carts and the orders placed
/// from them. What a storefront needs to sell, without the power to change
/// the catalog or an order once placed.
pub const SCOPE_STORE_CHECKOUT: &str = "store:checkout";
/// What a key created before scopes existed holds: everything there was then.
/// Frozen: a scope added later is never granted to those keys silently.
pub const LEGACY_API_KEY_SCOPES: &[&str] = &[SCOPE_STORE_READ, SCOPE_STORE_WRITE];

pub const MAX_ROLE_LABEL_LEN: usize = 40;
pub const MAX_API_KEY_LABEL_LEN: usize = 40;
/// Every store API token starts with it, so one is recognizable in a config.
pub const API_KEY_PREFIX: &str = "tk_";
/// How many characters after the prefix a key's hint keeps.
pub const API_KEY_HINT_LEN: usize = 8;
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
    random_base64url()
}

/// The stream of a store API key, from the token a storefront presents.
/// Whether holding the scope `held` grants `wanted`: the same scope,
/// `write` on the resource `wanted` only reads, or `store:write` for the
/// narrower `store:checkout`.
pub fn api_key_scope_implies(held: &str, wanted: &str) -> bool {
    if held == wanted || (held == SCOPE_STORE_WRITE && wanted == SCOPE_STORE_CHECKOUT) {
        return true;
    }
    match (held.split_once(':'), wanted.split_once(':')) {
        (Some((resource, "write")), Some((wanted_resource, "read"))) => resource == wanted_resource,
        _ => false,
    }
}

/// Deduplicates and checks a scope list against [`API_KEY_SCOPES`]; a key
/// holds at least one.
pub fn normalize_api_key_scopes<I>(scopes: I) -> Result<BTreeSet<String>, crate::TenancyError>
where
    I: IntoIterator,
    I::Item: AsRef<str>,
{
    let scopes = scopes
        .into_iter()
        .map(|s| {
            let s = s.as_ref().trim();
            if API_KEY_SCOPES.contains(&s) {
                Ok(s.to_owned())
            } else {
                Err(crate::TenancyError::InvalidApiKeyScopes)
            }
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    if scopes.is_empty() {
        return Err(crate::TenancyError::InvalidApiKeyScopes);
    }
    Ok(scopes)
}

pub fn api_key_id(token: &str) -> String {
    evento::hash_ids(vec!["api-key", token])
}

/// A fresh store API token: the prefix and 32 random bytes as URL-safe
/// base64 without padding (46 characters in all).
pub fn generate_api_key_token() -> String {
    format!("{API_KEY_PREFIX}{}", random_base64url())
}

/// What the pages show of a token: the prefix and the next
/// [`API_KEY_HINT_LEN`] characters.
pub fn api_key_hint(token: &str) -> String {
    token
        .chars()
        .take(API_KEY_PREFIX.len() + API_KEY_HINT_LEN)
        .collect()
}

/// Trims a key label and checks it is 1 to 40 characters.
pub fn normalize_api_key_label(label: &str) -> Result<String, crate::TenancyError> {
    let label = label.trim();
    if label.is_empty() || label.chars().count() > MAX_API_KEY_LABEL_LEN {
        return Err(crate::TenancyError::InvalidApiKeyLabel);
    }
    Ok(label.to_owned())
}

/// 32 random bytes as URL-safe base64 without padding (43 characters), from
/// the thread generator (seeded by the OS).
fn random_base64url() -> String {
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
    fn api_key_scopes_are_checked_and_write_implies_read() {
        let set = normalize_api_key_scopes([" store:write ", "store:read", "store:write"]).unwrap();
        assert_eq!(set.len(), 2);
        assert!(normalize_api_key_scopes(Vec::<String>::new()).is_err());
        assert!(normalize_api_key_scopes(["store:delete"]).is_err());
        assert!(api_key_scope_implies(SCOPE_STORE_WRITE, SCOPE_STORE_READ));
        assert!(!api_key_scope_implies(SCOPE_STORE_READ, SCOPE_STORE_WRITE));
        assert!(!api_key_scope_implies("catalog:write", SCOPE_STORE_READ));
        assert!(api_key_scope_implies(
            SCOPE_STORE_WRITE,
            SCOPE_STORE_CHECKOUT
        ));
        assert!(!api_key_scope_implies(
            SCOPE_STORE_CHECKOUT,
            SCOPE_STORE_WRITE
        ));
        assert!(!api_key_scope_implies(
            SCOPE_STORE_CHECKOUT,
            SCOPE_STORE_READ
        ));
        assert!(
            LEGACY_API_KEY_SCOPES
                .iter()
                .all(|s| API_KEY_SCOPES.contains(s))
        );
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
        assert!(implies("products:manage", "products:read"));
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

    #[test]
    fn api_key_tokens_are_prefixed_hinted_and_hashed() {
        let token = generate_api_key_token();
        assert_eq!(token.len(), API_KEY_PREFIX.len() + 43, "{token}");
        assert!(token.starts_with(API_KEY_PREFIX));
        assert_ne!(token, generate_api_key_token());
        let hint = api_key_hint(&token);
        assert_eq!(hint.len(), API_KEY_PREFIX.len() + API_KEY_HINT_LEN);
        assert!(token.starts_with(&hint));
        assert_eq!(api_key_id(&token).len(), 64);
        assert_ne!(api_key_id(&token), invitation_id(&token));
        assert_eq!(normalize_api_key_label("  Shop ").unwrap(), "Shop");
        assert!(normalize_api_key_label(" ").is_err());
        assert!(normalize_api_key_label(&"x".repeat(41)).is_err());
    }
}

pub const MAX_LEGAL_NAME_LEN: usize = 120;
pub const MAX_LEGAL_FORM_LEN: usize = 40;
pub const MAX_LEGAL_LINE_LEN: usize = 200;
pub const MAX_LEGAL_POSTAL_CODE_LEN: usize = 20;
pub const MAX_LEGAL_CITY_LEN: usize = 100;

/// Strips spaces and checks a SIRET: 14 digits passing the Luhn check. La
/// Poste's establishments (SIREN 356 000 000) follow their own rule instead:
/// the sum of the digits is a multiple of five.
pub fn normalize_siret(siret: &str) -> Result<String, crate::TenancyError> {
    let siret: String = siret.chars().filter(|c| !c.is_whitespace()).collect();
    if siret.len() != 14 || !siret.bytes().all(|b| b.is_ascii_digit()) {
        return Err(crate::TenancyError::InvalidSiret);
    }
    let digits = siret.bytes().map(|b| u32::from(b - b'0'));
    let valid = if siret.starts_with("356000000") {
        digits.sum::<u32>().is_multiple_of(5)
    } else {
        luhn(digits)
    };
    if valid {
        Ok(siret)
    } else {
        Err(crate::TenancyError::InvalidSiret)
    }
}

fn luhn(digits: impl DoubleEndedIterator<Item = u32>) -> bool {
    let sum: u32 = digits
        .rev()
        .enumerate()
        .map(|(i, d)| {
            if i % 2 == 1 {
                let d = d * 2;
                if d > 9 { d - 9 } else { d }
            } else {
                d
            }
        })
        .sum();
    sum.is_multiple_of(10)
}

/// Strips spaces, uppercases and checks a French VAT number against the
/// SIREN it must embed: `FR`, a two-digit key, the nine digits of the SIREN,
/// the key being `(12 + 3 × (SIREN mod 97)) mod 97`.
pub fn normalize_vat_number(vat_number: &str, siret: &str) -> Result<String, crate::TenancyError> {
    let vat: String = vat_number
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_ascii_uppercase();
    let siren = &siret[..9.min(siret.len())];
    let expected = siren
        .parse::<u64>()
        .map(|siren| format!("FR{:02}{siren:09}", (12 + 3 * (siren % 97)) % 97));
    match expected {
        Ok(expected) if expected == vat => Ok(vat),
        _ => Err(crate::TenancyError::InvalidVatNumber),
    }
}

/// Trims a required piece of the legal identity and checks its length.
pub(crate) fn normalize_legal_text(text: &str, max: usize) -> Result<String, crate::TenancyError> {
    let text = text.trim();
    if text.is_empty() || text.chars().count() > max {
        Err(crate::TenancyError::InvalidLegalIdentity)
    } else {
        Ok(text.to_owned())
    }
}

/// Like [`normalize_legal_text`], blank being `None`.
pub(crate) fn normalize_legal_optional(
    text: Option<&str>,
    max: usize,
) -> Result<Option<String>, crate::TenancyError> {
    match text.map(str::trim).filter(|t| !t.is_empty()) {
        None => Ok(None),
        Some(text) => normalize_legal_text(text, max).map(Some),
    }
}

#[cfg(test)]
mod legal_tests {
    use super::*;

    #[test]
    fn siret_is_luhn_checked() {
        // INSEE's head office.
        assert_eq!(
            normalize_siret("120 027 016 00092").unwrap(),
            "12002701600092"
        );
        assert!(normalize_siret("12002701600093").is_err());
        assert!(normalize_siret("1200270160009").is_err());
        assert!(normalize_siret("1200270160009a").is_err());
        // La Poste's own rule.
        assert!(normalize_siret("35600000000047").is_ok());
    }

    #[test]
    fn vat_number_embeds_the_siren_and_its_key() {
        assert_eq!(
            normalize_vat_number("fr 85 120027016", "12002701600092").unwrap(),
            "FR85120027016"
        );
        assert!(normalize_vat_number("FR86120027016", "12002701600092").is_err());
        assert!(normalize_vat_number("FR85120027017", "12002701600092").is_err());
    }
}

/// The longest series code a tenant numbers its invoices under.
pub const MAX_SERIES_CODE_LEN: usize = 10;

/// The index stream of a SIREN.
pub fn company_siren_id(siren: &str) -> String {
    evento::hash_ids(vec!["company-siren", siren])
}

/// The index stream of a company's series code (`""` is the plain series).
pub fn company_series_id(company_id: &str, code: &str) -> String {
    evento::hash_ids(vec!["company-series", company_id, code])
}

/// Uppercases a series code: blank is the plain series (`""`), otherwise 1
/// to [`MAX_SERIES_CODE_LEN`] letters and digits.
pub fn normalize_series_code(code: Option<&str>) -> Result<String, crate::TenancyError> {
    let code = code.unwrap_or_default().trim().to_ascii_uppercase();
    if code.len() <= MAX_SERIES_CODE_LEN && code.bytes().all(|b| b.is_ascii_alphanumeric()) {
        Ok(code)
    } else {
        Err(crate::TenancyError::InvalidSeriesCode)
    }
}

/// How a mail server connection is secured, as [`MailServerConfigured`]
/// records it: implicit TLS, STARTTLS, or none.
///
/// [`MailServerConfigured`]: crate::aggregator::MailServerConfigured
pub const MAIL_SECURITIES: &[&str] = &[
    MAIL_SECURITY_TLS,
    MAIL_SECURITY_STARTTLS,
    MAIL_SECURITY_NONE,
];
pub const MAIL_SECURITY_TLS: &str = "tls";
pub const MAIL_SECURITY_STARTTLS: &str = "starttls";
pub const MAIL_SECURITY_NONE: &str = "none";
