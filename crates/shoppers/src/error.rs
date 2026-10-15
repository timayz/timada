use evento::WriteError;
use timada_orders::OrdersError;

use crate::value_object::MAX_ADDRESSES;

#[derive(Debug, thiserror::Error)]
pub enum ShopperError {
    #[error("this is not an e-mail address")]
    InvalidEmail,
    /// The address does not check out; the orders context says why.
    #[error(transparent)]
    InvalidAddress(OrdersError),
    #[error("a code was mailed less than a minute ago")]
    CodeRecentlySent,
    /// Wrong, already used, or never mailed.
    #[error("this code is not the one we mailed")]
    WrongCode,
    #[error("this code has expired")]
    CodeExpired,
    #[error("too many wrong codes: ask for a new one")]
    TooManyAttempts,
    #[error("an account keeps at most {MAX_ADDRESSES} addresses")]
    TooManyAddresses,
    #[error("shopper not found")]
    NotFound,
    #[error(transparent)]
    Write(#[from] WriteError),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl ShopperError {
    /// Whether the message can be shown to the shopper (as opposed to an
    /// infrastructure failure).
    pub fn is_user_facing(&self) -> bool {
        !matches!(self, Self::Write(_) | Self::Other(_))
    }

    /// A stable code for the refusal, for a page to word in its own language;
    /// `None` for an infrastructure failure.
    pub fn code(&self) -> Option<&'static str> {
        Some(match self {
            Self::InvalidEmail => SHOPPER_INVALID_EMAIL,
            Self::InvalidAddress(err) => return err.code(),
            Self::CodeRecentlySent => SHOPPER_CODE_RECENTLY_SENT,
            Self::WrongCode => SHOPPER_WRONG_CODE,
            Self::CodeExpired => SHOPPER_CODE_EXPIRED,
            Self::TooManyAttempts => SHOPPER_TOO_MANY_ATTEMPTS,
            Self::TooManyAddresses => SHOPPER_TOO_MANY_ADDRESSES,
            Self::NotFound => "shopper_not_found",
            Self::Write(_) | Self::Other(_) => return None,
        })
    }
}

pub const SHOPPER_INVALID_EMAIL: &str = "invalid_email";
pub const SHOPPER_CODE_RECENTLY_SENT: &str = "code_recently_sent";
pub const SHOPPER_WRONG_CODE: &str = "wrong_code";
pub const SHOPPER_CODE_EXPIRED: &str = "code_expired";
pub const SHOPPER_TOO_MANY_ATTEMPTS: &str = "too_many_attempts";
pub const SHOPPER_TOO_MANY_ADDRESSES: &str = "too_many_addresses";

/// Every code a shopper may meet, address refusals included, so one read back
/// from an answer maps to a `&'static str` again.
pub const SHOPPER_CODES: &[&str] = &[
    SHOPPER_INVALID_EMAIL,
    SHOPPER_CODE_RECENTLY_SENT,
    SHOPPER_WRONG_CODE,
    SHOPPER_CODE_EXPIRED,
    SHOPPER_TOO_MANY_ATTEMPTS,
    SHOPPER_TOO_MANY_ADDRESSES,
    "invalid_ship_name",
    "invalid_address",
    "invalid_country",
    "invalid_phone",
];
