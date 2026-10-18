use evento::WriteError;

use crate::value_object::{MAX_MESSAGE_LEN, MAX_NAME_LEN, MIN_MESSAGE_LEN};

#[derive(Debug, thiserror::Error)]
pub enum SupportError {
    #[error("a first and a last name are needed, each at most {MAX_NAME_LEN} characters")]
    InvalidName,
    #[error("the address does not look like one")]
    InvalidEmail,
    #[error("a message says something in {MIN_MESSAGE_LEN} to {MAX_MESSAGE_LEN} characters")]
    InvalidBody,
    /// Unknown, or erased.
    #[error("message not found")]
    NotFound,
    #[error(transparent)]
    Write(#[from] WriteError),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl SupportError {
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
            Self::InvalidName => MESSAGE_INVALID_NAME,
            Self::InvalidEmail => MESSAGE_INVALID_EMAIL,
            Self::InvalidBody => MESSAGE_INVALID_BODY,
            Self::NotFound => MESSAGE_NOT_FOUND,
            Self::Write(err) => return timada_core::write_refusal(err),
            Self::Other(_) => return None,
        })
    }
}

/// The codes a visitor may meet, which a refused message carries.
pub const MESSAGE_INVALID_NAME: &str = "message_invalid_name";
pub const MESSAGE_INVALID_EMAIL: &str = "message_invalid_email";
pub const MESSAGE_INVALID_BODY: &str = "message_invalid_body";
pub const MESSAGE_NOT_FOUND: &str = "message_not_found";
/// The dashboard takes no more messages from this storefront or this
/// visitor for a while; not a refusal of the support context itself.
pub const SUPPORT_TOO_MANY_REQUESTS: &str = "too_many_requests";

/// Every code a visitor may meet, so one read back from an answer maps to a
/// `&'static str` again.
pub const SHOPPER_CODES: &[&str] = &[
    MESSAGE_INVALID_NAME,
    MESSAGE_INVALID_EMAIL,
    MESSAGE_INVALID_BODY,
    SUPPORT_TOO_MANY_REQUESTS,
    timada_core::WRITES_PAUSED,
];
