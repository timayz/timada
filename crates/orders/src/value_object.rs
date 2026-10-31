//! Identifiers, limits, the persisted value types and the checks on what a
//! shopper types.

use rand::Rng;

use crate::OrdersError;

/// Who a shopper's events say did them: a guest has no account to name.
pub const STOREFRONT_ACTOR: &str = "storefront";
/// The most of one item a line holds.
pub const MAX_LINE_QUANTITY: u32 = 99;
/// The most different items a cart holds.
pub const MAX_CART_LINES: usize = 50;
pub use timada_core::MAX_EMAIL_LEN;
pub const MAX_SHIP_NAME_LEN: usize = 120;
pub const MAX_ADDRESS_LINE_LEN: usize = 200;
pub const MAX_POSTAL_CODE_LEN: usize = 20;
pub const MAX_CITY_LEN: usize = 100;
pub const MAX_PHONE_LEN: usize = 30;
pub const MAX_NOTE_LEN: usize = 1000;
pub const MAX_TRACKING_LEN: usize = 200;
pub const MAX_REASON_LEN: usize = 500;
pub const MAX_SHIPPING_NAME_LEN: usize = 80;
/// The most countries one shipping method lists.
pub const MAX_SHIPPING_COUNTRIES: usize = 250;
/// The most shipping methods a shop offers.
pub const MAX_SHIPPING_METHODS: usize = 20;
/// The characters of a reference: Crockford's base 32, which leaves out the
/// letters a reader mistakes for digits (I, L, O) and U.
pub const REFERENCE_ALPHABET: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
/// How many characters a reference has, not counting its dash.
pub const REFERENCE_LEN: usize = 8;

/// Where an order goes. Frozen in `events.lock`: the fields never change.
#[derive(Debug, Clone, Default, PartialEq, Eq, bitcode::Encode, bitcode::Decode)]
pub struct ShippingAddress {
    /// The person or company the parcel is for.
    pub name: String,
    pub line1: String,
    pub line2: Option<String>,
    pub postal_code: String,
    pub city: String,
    /// ISO 3166-1 alpha-2, uppercase (`FR`).
    pub country: String,
    pub phone: Option<String>,
}

/// One line of an order, as it was sold. Frozen in `events.lock`: the fields
/// never change.
#[derive(Debug, Clone, Default, PartialEq, Eq, bitcode::Encode, bitcode::Decode)]
pub struct OrderLine {
    pub product_id: String,
    pub variant_id: Option<String>,
    /// The product's name when it was sold.
    pub name: String,
    /// The variant's values when it was sold (`Beige / Large`).
    pub variant_label: Option<String>,
    /// The merchant's code for what was sold, if it had one.
    pub sku: Option<String>,
    /// The price of one, in minor units of the order's currency.
    pub unit_amount_cents: u64,
    pub quantity: u32,
}

/// The payment that paid for an order. Frozen in `events.lock`: the fields
/// never change.
#[derive(Debug, Clone, Default, PartialEq, Eq, bitcode::Encode, bitcode::Decode)]
pub struct Payment {
    /// `stripe`.
    pub provider: String,
    /// The provider's id of the payment.
    pub payment_id: String,
    /// What was charged, in minor units of `currency`.
    pub amount_cents: u64,
    pub currency: String,
}

/// The payment provider the dashboard speaks to.
pub const PROVIDER_STRIPE: &str = "stripe";
/// The refusal of a request to pay when the merchant has no payments set up,
/// or the provider failed.
pub const PAYMENTS_UNAVAILABLE: &str = "payments_unavailable";
/// The refusal of a request to pay when the merchant has not said who it is
/// in law yet: no sale goes ahead without an invoice that can name it.
pub const INVOICING_NOT_CONFIGURED: &str = "invoicing_not_configured";

impl OrderLine {
    /// The price of the whole line.
    pub fn amount_cents(&self) -> u64 {
        self.unit_amount_cents * u64::from(self.quantity)
    }
}

/// Whether an event is one a shopper's storefront writes: filling a cart,
/// entering a discount code and asking to pay for it. Everything else is the dashboard's: answering a
/// request to pay, the orders (they exist once paid, which only the
/// provider's word to the dashboard proves) and the catalog. A store
/// connection allowed to sell but not to manage is held to these.
pub fn is_shopper_event(aggregate_type: &str, name: &str) -> bool {
    use evento::{Aggregate, AggregateEvent};

    use crate::aggregator::{
        BillingAddressGiven, BuyerIdentified, Cart, CheckoutRequested, Created, DiscountApplied,
        DiscountRemoved, ItemAdded, ItemRemoved, LanguageGiven, NewsletterOptedIn, QuantitySet,
        ShippingMethodChosen, TimeZoneGiven,
    };

    aggregate_type == Cart::aggregate_type()
        && [
            Created::event_name(),
            ItemAdded::event_name(),
            QuantitySet::event_name(),
            ItemRemoved::event_name(),
            CheckoutRequested::event_name(),
            BuyerIdentified::event_name(),
            BillingAddressGiven::event_name(),
            DiscountApplied::event_name(),
            DiscountRemoved::event_name(),
            ShippingMethodChosen::event_name(),
            LanguageGiven::event_name(),
            NewsletterOptedIn::event_name(),
            TimeZoneGiven::event_name(),
        ]
        .contains(&name)
}

/// The order a cart becomes: one per cart, so placing it twice lands on the
/// same stream.
pub fn order_id(cart_id: &str) -> String {
    evento::hash_ids(vec!["order", cart_id])
}

/// The stream of the tenant's payment method: one per tenant store.
pub fn payment_method_id() -> String {
    evento::hash_ids(vec!["payment-method"])
}

/// The stream of whether the tenant sells to businesses: one per tenant store.
pub fn business_buyers_id() -> String {
    evento::hash_ids(vec!["business-buyers"])
}

/// The stream of how the tenant ships: one per tenant store.
pub fn shipping_methods_id() -> String {
    evento::hash_ids(vec!["shipping-methods"])
}

/// The stream of where the tenant's storefront lives: one per tenant store.
pub fn storefront_id() -> String {
    evento::hash_ids(vec!["storefront"])
}

/// The stream of the confirmation mailed for an order.
pub fn order_confirmation_id(order_id: &str) -> String {
    evento::hash_ids(vec!["order-confirmation", order_id])
}

/// The address of a storefront as the merchant typed it, absolute and
/// without a trailing `/`.
pub fn normalize_storefront_url(url: &str) -> Result<String, OrdersError> {
    let url = url.trim().trim_end_matches('/');
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .ok_or(OrdersError::InvalidStorefrontUrl)?;
    if rest.is_empty() || url.len() > 200 || url.chars().any(char::is_whitespace) {
        return Err(OrdersError::InvalidStorefrontUrl);
    }
    Ok(url.to_owned())
}

/// The index stream of an order reference.
pub fn order_reference_id(reference: &str) -> String {
    evento::hash_ids(vec!["order-reference", reference])
}

/// A fresh random reference, `XXXX-XXXX`: 40 bits, short enough to read out
/// on the phone. Uniqueness is the index's job, not chance's.
pub fn new_reference() -> String {
    let mut rng = rand::rng();
    let chars: String = (0..REFERENCE_LEN)
        .map(|_| REFERENCE_ALPHABET[rng.random_range(0..REFERENCE_ALPHABET.len())] as char)
        .collect();
    format!("{}-{}", &chars[..4], &chars[4..])
}

/// The canonical form of an address: trimmed, lower-cased, one `@` with
/// something on both sides, nothing that is not printable. (The identity and
/// tenancy contexts have the same rule; the crates do not depend on each
/// other.)
pub fn normalize_email(email: &str) -> Result<String, OrdersError> {
    timada_core::normalize_email(email).ok_or(OrdersError::InvalidEmail)
}

/// Trims every field, drops empty optional ones, uppercases the country and
/// checks the lengths.
pub fn normalize_shipping(address: ShippingAddress) -> Result<ShippingAddress, OrdersError> {
    let text = |value: &str, max: usize, err: fn() -> OrdersError| {
        let value = value.trim();
        if value.is_empty() || value.chars().count() > max || value.chars().any(char::is_control) {
            Err(err())
        } else {
            Ok(value.to_owned())
        }
    };
    let optional = |value: Option<String>, max: usize, err: fn() -> OrdersError| match value
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        None => Ok(None),
        Some(value) => text(value, max, err).map(Some),
    };
    let country = address.country.trim().to_ascii_uppercase();
    if country.len() != 2 || !country.bytes().all(|b| b.is_ascii_uppercase()) {
        return Err(OrdersError::InvalidCountry);
    }
    Ok(ShippingAddress {
        name: text(&address.name, MAX_SHIP_NAME_LEN, || {
            OrdersError::InvalidName
        })?,
        line1: text(&address.line1, MAX_ADDRESS_LINE_LEN, || {
            OrdersError::InvalidAddress
        })?,
        line2: optional(address.line2, MAX_ADDRESS_LINE_LEN, || {
            OrdersError::InvalidAddress
        })?,
        postal_code: text(&address.postal_code, MAX_POSTAL_CODE_LEN, || {
            OrdersError::InvalidAddress
        })?,
        city: text(&address.city, MAX_CITY_LEN, || OrdersError::InvalidAddress)?,
        country,
        phone: optional(address.phone, MAX_PHONE_LEN, || OrdersError::InvalidPhone)?,
    })
}

/// A shipping method as the merchant describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewShippingMethod {
    pub name: String,
    pub price_cents: u64,
    pub currency: String,
    /// Two-letter codes; empty means every country.
    pub countries: Vec<String>,
    pub free_above_cents: Option<u64>,
}

/// Trims the name, uppercases the currency and the countries (sorted, each
/// once) and checks the amounts.
pub fn normalize_shipping_method(
    method: NewShippingMethod,
) -> Result<NewShippingMethod, OrdersError> {
    let name = method.name.trim();
    if name.is_empty()
        || name.chars().count() > MAX_SHIPPING_NAME_LEN
        || name.chars().any(char::is_control)
    {
        return Err(OrdersError::InvalidShippingName);
    }
    let currency = timada_catalog::normalize_currency(&method.currency)
        .map_err(|_| OrdersError::InvalidShippingCurrency)?;
    let max = timada_catalog::MAX_AMOUNT_CENTS;
    if method.price_cents > max || method.free_above_cents.is_some_and(|cents| cents > max) {
        return Err(OrdersError::InvalidShippingPrice);
    }
    let mut countries = Vec::with_capacity(method.countries.len());
    for country in &method.countries {
        let country = country.trim().to_ascii_uppercase();
        if country.len() != 2 || !country.bytes().all(|b| b.is_ascii_uppercase()) {
            return Err(OrdersError::InvalidShippingCountries);
        }
        countries.push(country);
    }
    countries.sort();
    countries.dedup();
    if countries.len() > MAX_SHIPPING_COUNTRIES {
        return Err(OrdersError::InvalidShippingCountries);
    }
    Ok(NewShippingMethod {
        name: name.to_owned(),
        price_cents: method.price_cents,
        currency,
        countries,
        free_above_cents: method.free_above_cents,
    })
}

/// The countries a merchant typed, split on commas and spaces: `"fr, BE"`
/// is `["fr", "BE"]`. [`normalize_shipping_method`] checks them.
pub fn split_countries(text: &str) -> Vec<String> {
    text.split(|c: char| c == ',' || c.is_whitespace())
        .filter(|code| !code.is_empty())
        .map(str::to_owned)
        .collect()
}

/// A billing address: checked like a shipping address, without its phone.
pub fn normalize_billing(address: ShippingAddress) -> Result<ShippingAddress, OrdersError> {
    Ok(ShippingAddress {
        phone: None,
        ..normalize_shipping(address)?
    })
}

/// A trimmed optional text of at most `max` characters; blank is `None`.
fn optional_text(
    value: Option<&str>,
    max: usize,
    err: OrdersError,
) -> Result<Option<String>, OrdersError> {
    match value.map(str::trim).filter(|v| !v.is_empty()) {
        None => Ok(None),
        Some(value) if value.chars().count() <= max => Ok(Some(value.to_owned())),
        Some(_) => Err(err),
    }
}

/// The country prefixes of EU VAT numbers (Greece's is `EL`).
pub const EU_VAT_PREFIXES: &[&str] = &[
    "AT", "BE", "BG", "CY", "CZ", "DE", "DK", "EE", "EL", "ES", "FI", "FR", "HR", "HU", "IE", "IT",
    "LT", "LU", "LV", "MT", "NL", "PL", "PT", "RO", "SE", "SI", "SK",
];

/// An EU VAT number without its spaces, dots and dashes, uppercased: a
/// country prefix of [`EU_VAT_PREFIXES`] and 2 to 12 letters or digits.
/// Blank is `None`. Whether the number exists is VIES's to say.
pub fn normalize_buyer_vat_number(vat_number: Option<&str>) -> Result<Option<String>, OrdersError> {
    let Some(raw) = vat_number else {
        return Ok(None);
    };
    let number: String = raw
        .chars()
        .filter(|c| !c.is_whitespace() && !matches!(c, '.' | '-'))
        .collect::<String>()
        .to_ascii_uppercase();
    if number.is_empty() {
        return Ok(None);
    }
    let valid = number.len() >= 4
        && number.len() <= 14
        && EU_VAT_PREFIXES.contains(&&number[..2])
        && number[2..].bytes().all(|b| b.is_ascii_alphanumeric());
    if valid {
        Ok(Some(number))
    } else {
        Err(OrdersError::InvalidVatNumber)
    }
}

pub fn normalize_note(note: Option<&str>) -> Result<Option<String>, OrdersError> {
    optional_text(note, MAX_NOTE_LEN, OrdersError::InvalidNote)
}

pub fn normalize_tracking(tracking: Option<&str>) -> Result<Option<String>, OrdersError> {
    optional_text(tracking, MAX_TRACKING_LEN, OrdersError::InvalidTracking)
}

/// A cancellation needs a reason.
pub fn normalize_reason(reason: &str) -> Result<String, OrdersError> {
    optional_text(Some(reason), MAX_REASON_LEN, OrdersError::InvalidReason)?
        .ok_or(OrdersError::InvalidReason)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn references_read_well() {
        for _ in 0..100 {
            let reference = new_reference();
            assert_eq!(reference.len(), REFERENCE_LEN + 1);
            assert_eq!(reference.as_bytes()[4], b'-');
            assert!(
                reference
                    .bytes()
                    .filter(|b| *b != b'-')
                    .all(|b| REFERENCE_ALPHABET.contains(&b))
            );
        }
    }

    #[test]
    fn addresses_are_trimmed_and_checked() {
        let address = ShippingAddress {
            name: " Ada Lovelace ".into(),
            line1: "1 rue de la Paix".into(),
            line2: Some("  ".into()),
            postal_code: "75002".into(),
            city: "Paris".into(),
            country: "fr".into(),
            phone: None,
        };
        let address = normalize_shipping(address).unwrap();
        assert_eq!(address.name, "Ada Lovelace");
        assert_eq!(address.line2, None);
        assert_eq!(address.country, "FR");

        let no_city = ShippingAddress {
            city: " ".into(),
            ..address.clone()
        };
        assert!(matches!(
            normalize_shipping(no_city),
            Err(OrdersError::InvalidAddress)
        ));
        let bad_country = ShippingAddress {
            country: "FRA".into(),
            ..address
        };
        assert!(matches!(
            normalize_shipping(bad_country),
            Err(OrdersError::InvalidCountry)
        ));
    }

    #[test]
    fn shoppers_write_carts_and_place_orders_only() {
        assert!(is_shopper_event("timada-orders/Cart", "ItemAdded"));
        assert!(is_shopper_event("timada-orders/Cart", "CheckoutRequested"));
        assert!(is_shopper_event("timada-orders/Cart", "DiscountApplied"));
        assert!(is_shopper_event("timada-orders/Cart", "DiscountRemoved"));
        assert!(is_shopper_event(
            "timada-orders/Cart",
            "BillingAddressGiven"
        ));
        assert!(!is_shopper_event(
            "timada-orders/Order",
            "BillingAddressSet"
        ));
        assert!(!is_shopper_event("timada-orders/Cart", "PaymentRequested"));
        // Only the dashboard says what a payment is discounted by.
        assert!(!is_shopper_event("timada-orders/Cart", "DiscountGranted"));
        assert!(!is_shopper_event("timada-orders/Order", "Discounted"));
        // The shopper picks a shipping method; only the dashboard prices it.
        assert!(is_shopper_event(
            "timada-orders/Cart",
            "ShippingMethodChosen"
        ));
        assert!(!is_shopper_event("timada-orders/Cart", "ShippingGranted"));
        assert!(!is_shopper_event("timada-orders/Order", "ShippingCharged"));
        assert!(!is_shopper_event(
            "timada-orders/ShippingMethods",
            "MethodAdded"
        ));
        assert!(!is_shopper_event("timada-orders/Cart", "CheckedOut"));
        assert!(!is_shopper_event("timada-orders/OrderReference", "Claimed"));
        assert!(!is_shopper_event("timada-orders/Order", "Placed"));
        assert!(!is_shopper_event("timada-orders/Order", "Cancelled"));
        assert!(!is_shopper_event("timada-catalog/Product", "Priced"));
        assert!(!is_shopper_event(
            "timada-orders/Storefront",
            "StorefrontAddressSet"
        ));
        assert!(!is_shopper_event(
            "timada-orders/OrderConfirmation",
            "ConfirmationMailed"
        ));
    }

    #[test]
    fn storefront_addresses_are_absolute() {
        assert_eq!(
            normalize_storefront_url(" https://shop.example/ ").unwrap(),
            "https://shop.example"
        );
        assert_eq!(
            normalize_storefront_url("http://localhost:4000").unwrap(),
            "http://localhost:4000"
        );
        for bad in [
            "shop.example",
            "ftp://shop.example",
            "https://",
            "https://a b",
        ] {
            assert!(
                matches!(
                    normalize_storefront_url(bad),
                    Err(OrdersError::InvalidStorefrontUrl)
                ),
                "{bad}"
            );
        }
    }

    #[test]
    fn emails_and_reasons() {
        assert_eq!(
            normalize_email(" Ada@Example.org ").unwrap(),
            "ada@example.org"
        );
        assert!(normalize_email("ada").is_err());
        assert!(normalize_reason("  ").is_err());
        assert_eq!(normalize_tracking(Some(" ")).unwrap(), None);
    }
}
