use evento::WriteError;

#[derive(Debug, thiserror::Error)]
pub enum DiscountError {
    #[error("a code is 3 to 32 letters, digits, dashes and underscores")]
    InvalidCode,
    #[error("this code is already taken")]
    CodeTaken,
    #[error("a percentage is more than 0 and at most 100, with at most two decimals")]
    InvalidPercent,
    #[error("the amount must be a positive number with at most two decimals")]
    InvalidAmount,
    #[error("unknown currency")]
    InvalidCurrency,
    #[error("give either a percentage or an amount off")]
    InvalidValue,
    #[error("the end must come after the start")]
    InvalidWindow,
    #[error("a code serves 1 to 10000 orders")]
    InvalidMaxUses,
    #[error("discount not found")]
    NotFound,
    #[error("this discount is archived")]
    Archived,
    /// No live discount has this code.
    #[error("this code is not valid")]
    Unknown,
    /// Deactivated, not started yet, or over.
    #[error("this code cannot be used now")]
    Inactive,
    /// A fixed amount in another currency than the cart's.
    #[error("this code is for another currency")]
    Currency,
    #[error("this code has been used as many times as it can be")]
    Exhausted,
    #[error("this code has already been used with this e-mail")]
    AlreadyUsed,
    #[error(transparent)]
    Write(#[from] WriteError),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl DiscountError {
    /// Whether the message can be shown to the person who made the request
    /// (as opposed to an infrastructure failure).
    pub fn is_user_facing(&self) -> bool {
        !matches!(self, Self::Write(_) | Self::Other(_))
    }

    /// A stable code for the refusal, for a page to carry in a URL and word
    /// in its own language; `None` for an infrastructure failure.
    pub fn code(&self) -> Option<&'static str> {
        Some(match self {
            Self::InvalidCode => "invalid_code",
            Self::CodeTaken => "code_taken",
            Self::InvalidPercent => "invalid_percent",
            Self::InvalidAmount => "invalid_amount",
            Self::InvalidCurrency => "invalid_currency",
            Self::InvalidValue => "invalid_value",
            Self::InvalidWindow => "invalid_window",
            Self::InvalidMaxUses => "invalid_max_uses",
            Self::NotFound => "discount_not_found",
            Self::Archived => "discount_archived",
            Self::Unknown => DISCOUNT_UNKNOWN,
            Self::Inactive => DISCOUNT_INACTIVE,
            Self::Currency => DISCOUNT_CURRENCY,
            Self::Exhausted => DISCOUNT_EXHAUSTED,
            Self::AlreadyUsed => DISCOUNT_ALREADY_USED,
            Self::Write(_) | Self::Other(_) => return None,
        })
    }
}

/// The codes a shopper may meet, which a checkout refusal carries.
pub const DISCOUNT_UNKNOWN: &str = "discount_unknown";
pub const DISCOUNT_INACTIVE: &str = "discount_inactive";
pub const DISCOUNT_CURRENCY: &str = "discount_currency";
pub const DISCOUNT_EXHAUSTED: &str = "discount_exhausted";
pub const DISCOUNT_ALREADY_USED: &str = "discount_already_used";

/// Every code above, so one read back from the store maps to a
/// `&'static str` again.
pub const SHOPPER_CODES: &[&str] = &[
    DISCOUNT_UNKNOWN,
    DISCOUNT_INACTIVE,
    DISCOUNT_CURRENCY,
    DISCOUNT_EXHAUSTED,
    DISCOUNT_ALREADY_USED,
];
