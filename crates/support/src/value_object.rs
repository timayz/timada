use std::fmt;

use crate::error::SupportError;

/// The longest first or last name, in characters.
pub const MAX_NAME_LEN: usize = 80;
pub const MIN_MESSAGE_LEN: usize = 10;
pub const MAX_MESSAGE_LEN: usize = 4000;
pub const MAX_EMAIL_LEN: usize = 254;

/// Where a message stands.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MessageStatus {
    /// Waiting for the merchant.
    #[default]
    Open,
    Handled,
}

impl MessageStatus {
    pub const ALL: [Self; 2] = [Self::Open, Self::Handled];

    /// The value stored in the read model.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Handled => "handled",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "open" => Some(Self::Open),
            "handled" => Some(Self::Handled),
            _ => None,
        }
    }
}

impl fmt::Display for MessageStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A fresh id for a message: a ULID, unguessable and sortable by when it was
/// made.
pub fn new_message_id() -> String {
    ulid::Ulid::generate().to_string()
}

/// The canonical form of an address: trimmed, lower-cased, one `@` with
/// something on both sides, nothing that is not printable. (The orders,
/// identity and tenancy contexts have the same rule; the crates do not
/// depend on each other.)
pub fn normalize_email(email: &str) -> Result<String, SupportError> {
    let email = email.trim().to_lowercase();
    let valid = email.len() <= MAX_EMAIL_LEN
        && !email.chars().any(|c| c.is_whitespace() || c.is_control())
        && matches!(email.split_once('@'), Some((local, domain))
            if !local.is_empty() && !domain.is_empty() && !domain.contains('@'));
    if valid {
        Ok(email)
    } else {
        Err(SupportError::InvalidEmail)
    }
}

/// A name as signed: trimmed, not blank, at most [`MAX_NAME_LEN`] characters,
/// nothing that is not printable.
pub fn check_name(name: &str) -> Result<String, SupportError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME_LEN || name.chars().any(char::is_control)
    {
        return Err(SupportError::InvalidName);
    }
    Ok(name.to_owned())
}

/// The text of a message: trimmed, [`MIN_MESSAGE_LEN`] to [`MAX_MESSAGE_LEN`]
/// characters. Line breaks stay; other control characters are refused.
pub fn check_body(body: &str) -> Result<String, SupportError> {
    let body = body.trim();
    let len = body.chars().count();
    if !(MIN_MESSAGE_LEN..=MAX_MESSAGE_LEN).contains(&len)
        || body
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\r' && c != '\t')
    {
        return Err(SupportError::InvalidBody);
    }
    Ok(body.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_read_back() {
        for status in MessageStatus::ALL {
            assert_eq!(MessageStatus::parse(status.as_str()), Some(status));
        }
        assert_eq!(MessageStatus::parse("other"), None);
    }

    #[test]
    fn an_address_is_normalised_or_refused() {
        assert_eq!(
            normalize_email("  Ada@Example.com ").unwrap(),
            "ada@example.com"
        );
        for bad in [
            "",
            "ada",
            "@example.com",
            "ada@",
            "a@b@c",
            "ada @example.com",
        ] {
            assert!(normalize_email(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn names_and_texts_are_checked() {
        assert_eq!(check_name("  Ada ").unwrap(), "Ada");
        assert!(check_name("   ").is_err());
        assert!(check_name(&"x".repeat(81)).is_err());
        assert!(check_name("Ada\u{7}").is_err());
        assert_eq!(
            check_body("  Where is my order?\nThanks.  ").unwrap(),
            "Where is my order?\nThanks."
        );
        assert!(check_body("too short").is_err());
        assert!(check_body(&"x".repeat(4001)).is_err());
        assert!(check_body("Where is my order?\u{0}").is_err());
    }
}
