use evento::WriteError;

#[derive(Debug, thiserror::Error)]
pub enum OrdersError {
    #[error("cart not found")]
    CartNotFound,
    #[error("this cart has already been ordered")]
    CheckedOut,
    #[error("a quantity is a whole number from 1 to 99")]
    InvalidQuantity,
    #[error("a cart holds at most fifty different items")]
    TooManyLines,
    #[error("this product is not on sale")]
    Unavailable { product_id: String },
    #[error("pick one of the product's variants")]
    VariantRequired,
    #[error("variant not found")]
    VariantNotFound,
    #[error("everything in one order is priced in the same currency")]
    MixedCurrencies,
    #[error("the cart is empty")]
    EmptyCart,
    #[error("the total changed since it was shown")]
    PriceChanged { total_cents: u64 },
    #[error("enter a valid e-mail address")]
    InvalidEmail,
    #[error("enter the name to ship to (at most 120 characters)")]
    InvalidName,
    #[error("enter the street, postal code and city of the address")]
    InvalidAddress,
    #[error("the country is a two-letter code such as FR")]
    InvalidCountry,
    #[error("the phone number is at most 30 characters")]
    InvalidPhone,
    #[error("the note is at most 1000 characters")]
    InvalidNote,
    #[error("order not found")]
    OrderNotFound,
    #[error("this order is not awaiting payment")]
    NotAwaitingPayment,
    #[error("only a paid order can be fulfilled")]
    NotPaid,
    #[error("a fulfilled or cancelled order cannot be cancelled")]
    NotCancellable,
    #[error("the tracking number is at most 200 characters")]
    InvalidTracking,
    #[error("say why the order is cancelled (at most 500 characters)")]
    InvalidReason,
    #[error(transparent)]
    Write(#[from] WriteError),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl OrdersError {
    /// Whether the message can be shown to the person who made the request
    /// (as opposed to an infrastructure failure).
    pub fn is_user_facing(&self) -> bool {
        !matches!(self, Self::Write(_) | Self::Other(_))
    }

    /// A stable code for the refusal, for a page to carry in a URL and word
    /// in its own language; `None` for an infrastructure failure.
    pub fn code(&self) -> Option<&'static str> {
        Some(match self {
            Self::CartNotFound => "cart_not_found",
            Self::CheckedOut => "cart_checked_out",
            Self::InvalidQuantity => "invalid_quantity",
            Self::TooManyLines => "too_many_lines",
            Self::Unavailable { .. } => "product_unavailable",
            Self::VariantRequired => "variant_required",
            Self::VariantNotFound => "variant_not_found",
            Self::MixedCurrencies => "mixed_currencies",
            Self::EmptyCart => "empty_cart",
            Self::PriceChanged { .. } => "price_changed",
            Self::InvalidEmail => "invalid_email",
            Self::InvalidName => "invalid_ship_name",
            Self::InvalidAddress => "invalid_address",
            Self::InvalidCountry => "invalid_country",
            Self::InvalidPhone => "invalid_phone",
            Self::InvalidNote => "invalid_note",
            Self::OrderNotFound => "order_not_found",
            Self::NotAwaitingPayment => "not_awaiting_payment",
            Self::NotPaid => "not_paid",
            Self::NotCancellable => "not_cancellable",
            Self::InvalidTracking => "invalid_tracking",
            Self::InvalidReason => "invalid_reason",
            Self::Write(_) | Self::Other(_) => return None,
        })
    }
}
