use evento::WriteError;

#[derive(Debug, thiserror::Error)]
pub enum OrderError {
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
    #[error("only {available} left of this product")]
    OutOfStock { product_id: String, available: u32 },
    #[error("pick one of the product's variants")]
    VariantRequired,
    #[error("variant not found")]
    VariantNotFound,
    #[error("everything in one order is priced in the same currency")]
    MixedCurrencies,
    #[error("the cart is empty")]
    EmptyCart,
    #[error("no payment was asked for this cart under that id")]
    PaymentNotFound,
    #[error("the amount paid is not what the cart was priced at")]
    AmountMismatch,
    #[error("this cart was already paid by another payment")]
    DuplicatePayment { order_id: String },
    /// The dashboard answered a request to pay with a refusal; `code` is the
    /// one it recorded (any code of this enum, or `payments_unavailable`).
    #[error("the payment cannot go ahead ({code})")]
    PaymentRefused { code: String },
    #[error("the total changed since it was shown")]
    PriceChanged { total_cents: u64 },
    #[error("enter a valid e-mail address")]
    InvalidEmail,
    #[error("enter an EU VAT number: its country prefix, then letters and digits")]
    InvalidVatNumber,
    #[error("this shop does not sell to businesses: leave the VAT number blank")]
    BusinessBuyersRefused,
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
    #[error("a cancelled order cannot be fulfilled")]
    OrderCancelled,
    #[error("a fulfilled or cancelled order cannot be cancelled")]
    NotCancellable,
    #[error("a refund is on its way: wait for it to be recorded")]
    RefundPending,
    #[error("the refund must be more than nothing and at most what is outstanding")]
    InvalidRefund,
    #[error("the tracking number is at most 200 characters")]
    InvalidTracking,
    #[error("say why the order is cancelled (at most 500 characters)")]
    InvalidReason,
    #[error("the storefront address starts with https:// (at most 200 characters)")]
    InvalidStorefrontUrl,
    /// The cart's discount code cannot be used; `code` says why (one of
    /// [`timada_discounts::SHOPPER_CODES`]).
    #[error("this discount code cannot be used")]
    DiscountRefused { code: &'static str },
    #[error("name the shipping method (at most 80 characters)")]
    InvalidShippingName,
    #[error("a shipping method is priced in one of the shop's currencies")]
    InvalidShippingCurrency,
    #[error("enter a shipping price, such as 4.90")]
    InvalidShippingPrice,
    #[error("list the countries as two-letter codes such as FR, BE")]
    InvalidShippingCountries,
    #[error("a shop offers at most twenty shipping methods")]
    TooManyShippingMethods,
    #[error("shipping method not found")]
    ShippingMethodNotFound,
    #[error("pick a shipping method that ships to this country")]
    ShippingUnavailable,
    #[error(transparent)]
    Write(#[from] WriteError),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Every code a refusal recorded on a cart may carry, so one read back from
/// the store maps to a `&'static str` again.
const KNOWN_CODES: &[&str] = &[
    "cart_not_found",
    "cart_checked_out",
    "product_unavailable",
    "out_of_stock",
    "variant_required",
    "variant_not_found",
    "mixed_currencies",
    "empty_cart",
    "price_changed",
    "shipping_unavailable",
    crate::PAYMENTS_UNAVAILABLE,
    crate::INVOICING_NOT_CONFIGURED,
    crate::PAYMENT_EXPIRED,
];

impl OrderError {
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
            Self::CartNotFound => "cart_not_found",
            Self::CheckedOut => "cart_checked_out",
            Self::InvalidQuantity => "invalid_quantity",
            Self::TooManyLines => "too_many_lines",
            Self::Unavailable { .. } => "product_unavailable",
            Self::OutOfStock { .. } => "out_of_stock",
            Self::VariantRequired => "variant_required",
            Self::VariantNotFound => "variant_not_found",
            Self::MixedCurrencies => "mixed_currencies",
            Self::EmptyCart => "empty_cart",
            Self::PriceChanged { .. } => "price_changed",
            Self::PaymentNotFound => "payment_not_found",
            Self::PaymentRefused { code } => {
                return Some(
                    KNOWN_CODES
                        .iter()
                        .chain(timada_discounts::SHOPPER_CODES)
                        .copied()
                        .find(|known| known == code)
                        .unwrap_or(crate::PAYMENTS_UNAVAILABLE),
                );
            }
            Self::AmountMismatch => "amount_mismatch",
            Self::DuplicatePayment { .. } => "duplicate_payment",
            Self::InvalidEmail => "invalid_email",
            Self::InvalidVatNumber => "invalid_vat_number",
            Self::BusinessBuyersRefused => "business_buyers_refused",
            Self::InvalidName => "invalid_ship_name",
            Self::InvalidAddress => "invalid_address",
            Self::InvalidCountry => "invalid_country",
            Self::InvalidPhone => "invalid_phone",
            Self::InvalidNote => "invalid_note",
            Self::OrderNotFound => "order_not_found",
            Self::OrderCancelled => "order_cancelled",
            Self::NotCancellable => "not_cancellable",
            Self::RefundPending => "refund_pending",
            Self::InvalidRefund => "invalid_refund",
            Self::InvalidTracking => "invalid_tracking",
            Self::InvalidReason => "invalid_reason",
            Self::InvalidStorefrontUrl => "invalid_storefront_url",
            Self::DiscountRefused { code } => code,
            Self::InvalidShippingName => "invalid_shipping_name",
            Self::InvalidShippingCurrency => "invalid_currency",
            Self::InvalidShippingPrice => "invalid_amount",
            Self::InvalidShippingCountries => "invalid_shipping_countries",
            Self::TooManyShippingMethods => "too_many_shipping_methods",
            Self::ShippingMethodNotFound => "shipping_method_not_found",
            Self::ShippingUnavailable => "shipping_unavailable",
            Self::Write(err) => return timada_core::write_refusal(err),
            Self::Other(_) => return None,
        })
    }
}

impl OrderError {
    /// A discount the cart cannot use, as the refusal a shopper reads.
    pub(crate) fn from_discount(err: timada_discounts::DiscountError) -> Self {
        use timada_discounts::DiscountError;
        match err {
            DiscountError::Write(err) => Self::Write(err),
            DiscountError::Other(err) => Self::Other(err),
            DiscountError::Inactive | DiscountError::Archived => Self::DiscountRefused {
                code: timada_discounts::DISCOUNT_INACTIVE,
            },
            DiscountError::Currency => Self::DiscountRefused {
                code: timada_discounts::DISCOUNT_CURRENCY,
            },
            DiscountError::Exhausted => Self::DiscountRefused {
                code: timada_discounts::DISCOUNT_EXHAUSTED,
            },
            DiscountError::AlreadyUsed => Self::DiscountRefused {
                code: timada_discounts::DISCOUNT_ALREADY_USED,
            },
            _ => Self::DiscountRefused {
                code: timada_discounts::DISCOUNT_UNKNOWN,
            },
        }
    }
}
