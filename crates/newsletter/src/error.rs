use evento::WriteError;

#[derive(Debug, thiserror::Error)]
pub enum NewsletterError {
    #[error("the address does not look like one")]
    InvalidEmail,
    #[error("unknown place to subscribe from")]
    InvalidSource,
    /// A link that was never mailed, was tampered with, or no longer names a
    /// pending request (confirmed since, asked again, or erased).
    #[error("the link is not valid")]
    InvalidLink,
    #[error("the link has expired")]
    LinkExpired,
    /// Unknown, or erased.
    #[error("subscriber not found")]
    NotFound,
    #[error(transparent)]
    Write(#[from] WriteError),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl NewsletterError {
    /// Whether the message can be shown to the person who made the request
    /// (as opposed to an infrastructure failure).
    pub fn is_user_facing(&self) -> bool {
        match self {
            // A write the store is not taking right now is told to the person.
            Self::Write(err) => timada_core::write_refusal(err).is_some(),
            Self::Other(_) => false,
            _ => true,
        }
    }

    /// A stable code for the refusal, for a page to carry in a URL and word
    /// in its own language; `None` for an infrastructure failure.
    pub fn code(&self) -> Option<&'static str> {
        Some(match self {
            Self::InvalidEmail => NEWSLETTER_INVALID_EMAIL,
            Self::InvalidSource => NEWSLETTER_INVALID_SOURCE,
            Self::InvalidLink => NEWSLETTER_INVALID_LINK,
            Self::LinkExpired => NEWSLETTER_LINK_EXPIRED,
            Self::NotFound => NEWSLETTER_NOT_FOUND,
            Self::Write(err) => return timada_core::write_refusal(err),
            Self::Other(_) => return None,
        })
    }
}

/// The codes a shopper may meet, which a refused request carries.
pub const NEWSLETTER_INVALID_EMAIL: &str = "newsletter_invalid_email";
pub const NEWSLETTER_INVALID_SOURCE: &str = "newsletter_invalid_source";
pub const NEWSLETTER_INVALID_LINK: &str = "newsletter_invalid_link";
pub const NEWSLETTER_LINK_EXPIRED: &str = "newsletter_link_expired";
pub const NEWSLETTER_NOT_FOUND: &str = "newsletter_not_found";
/// The shop has not said where its storefront lives, so no confirmation link
/// can be mailed; not a refusal of the newsletter context itself.
pub const NEWSLETTER_UNAVAILABLE: &str = "newsletter_unavailable";
/// The dashboard takes no more sign-ups from this storefront or this
/// visitor for a while; not a refusal of the newsletter context itself.
pub const NEWSLETTER_TOO_MANY_REQUESTS: &str = "too_many_requests";

/// Every code a shopper may meet, so one read back from an answer maps to a
/// `&'static str` again.
pub const SHOPPER_CODES: &[&str] = &[
    NEWSLETTER_INVALID_EMAIL,
    NEWSLETTER_INVALID_SOURCE,
    NEWSLETTER_INVALID_LINK,
    NEWSLETTER_LINK_EXPIRED,
    NEWSLETTER_NOT_FOUND,
    NEWSLETTER_UNAVAILABLE,
    NEWSLETTER_TOO_MANY_REQUESTS,
    timada_core::WRITES_PAUSED,
];
