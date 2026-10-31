//! The canonical form of an e-mail address, the same in every context, so
//! an address typed at checkout, at sign-in and on a contact form is one
//! address everywhere.

/// The longest address kept (RFC 5321's path limit), in bytes.
pub const MAX_EMAIL_LEN: usize = 254;

/// Trimmed, lower-cased, at most [`MAX_EMAIL_LEN`], one `@` with something
/// on both sides and nothing that is not printable; `None` otherwise. Each
/// context turns the `None` into its own refusal.
pub fn normalize_email(email: &str) -> Option<String> {
    let email = email.trim().to_lowercase();
    let valid = email.len() <= MAX_EMAIL_LEN
        && !email.chars().any(|c| c.is_whitespace() || c.is_control())
        && matches!(email.split_once('@'), Some((local, domain))
            if !local.is_empty() && !domain.is_empty() && !domain.contains('@'));
    valid.then_some(email)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_is_trimmed_lowercased_and_checked() {
        assert_eq!(
            normalize_email("  Ada@Example.ORG ").as_deref(),
            Some("ada@example.org")
        );
        assert_eq!(
            normalize_email("root@localhost").as_deref(),
            Some("root@localhost")
        );
        for bad in [
            "",
            "ada",
            "@example.org",
            "ada@",
            "a da@example.org",
            "ada@exa@mple.org",
            "ada\u{7}@example.org",
        ] {
            assert_eq!(normalize_email(bad), None, "{bad:?}");
        }
        let long = format!("{}@example.org", "a".repeat(MAX_EMAIL_LEN));
        assert_eq!(normalize_email(&long), None);
    }
}
