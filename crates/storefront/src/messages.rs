//! What the shop says to its shoppers in words the library chooses: how a
//! refusal reads, an order's status, a country's name, what the scripts say.
//! English out of the box; an app hands its own table to
//! [`Storefront::messages`](crate::Storefront::messages) once, and every
//! page and script reads from it.

use std::borrow::Cow;
use std::collections::HashMap;

use topcoat_core::context::Cx;

use crate::cart::OrderStatus;
use crate::reviews::ReviewStatus;

type Text = Cow<'static, str>;

/// The shop's wording.
#[derive(Clone, Debug)]
pub struct Messages {
    refusals: HashMap<&'static str, Text>,
    /// What a refusal the table does not know reads as.
    pub fallback: Text,
    /// What the account page says once something is done, by code
    /// (`signed_out_elsewhere`).
    done: HashMap<&'static str, Text>,
    pub order_paid: Text,
    pub order_fulfilled: Text,
    pub order_cancelled: Text,
    pub review_pending: Text,
    pub review_published: Text,
    pub review_rejected: Text,
    /// The countries the shop ships to, as the checkout lists them: ISO
    /// 3166-1 alpha-2 and the name a shopper reads, in that order.
    pub countries: Vec<(&'static str, Text)>,
    /// Where a checkout starts when the shopper has not said.
    pub default_country: Text,
    /// A shipping method that costs nothing for this cart.
    pub shipping_free: Text,
    /// No method ships to the country.
    pub shipping_unavailable: Text,
    /// The shop ships for free, with no method to pick.
    pub shipping_included: Text,
    /// Before the VAT a total includes: `incl.` in `(incl. 4.17 EUR VAT FR 20%)`.
    pub vat_prefix: Text,
    /// A cart line whose product the store no longer has.
    pub off_sale_product: Text,
    /// What a wallet calls the shipping of a shop that ships for free.
    pub standard_shipping: Text,
    /// What a script says when a request fails.
    pub something_went_wrong: Text,
}

impl Default for Messages {
    fn default() -> Self {
        Self::english()
    }
}

/// Every refusal code and its English wording.
const ENGLISH: &[(&str, &str)] = &[
    ("invalid_quantity", "Choose a quantity from 1 to 99."),
    (
        "too_many_lines",
        "Your cart is full: order what is in it first.",
    ),
    (
        "product_unavailable",
        "Sorry, this product is no longer on sale.",
    ),
    ("variant_required", "Choose an option first."),
    ("variant_not_found", "This combination is not sold."),
    (
        "mixed_currencies",
        "This product is sold in another currency than your cart.",
    ),
    ("empty_cart", "Your cart is empty."),
    (
        "price_changed",
        "A price changed since you saw it: check the new total and confirm again.",
    ),
    (
        "payments_unavailable",
        "Payments are not available right now. Please try again later.",
    ),
    (
        "invoicing_not_configured",
        "This shop cannot take orders yet: it has not finished setting up. Please try again later.",
    ),
    (
        "payment_failed",
        "The payment did not go through. Try again or use another card.",
    ),
    (
        "payment_expired",
        "This payment is no longer open. Please check out again.",
    ),
    (
        "payment_not_found",
        "This payment is no longer open. Please check out again.",
    ),
    ("invalid_email", "Enter a valid e-mail address."),
    (
        "invalid_vat_number",
        "Enter an EU VAT number: its country prefix, then letters and digits.",
    ),
    (
        "shipping_unavailable",
        "Choose a delivery method that ships to your country.",
    ),
    (
        "business_buyers_refused",
        "This shop does not sell to businesses: leave the VAT number blank.",
    ),
    ("invalid_ship_name", "Enter the name to ship to."),
    ("invalid_address", "Enter the street, postal code and city."),
    (
        "invalid_country",
        "Enter the country as a two-letter code, such as FR.",
    ),
    ("invalid_phone", "The phone number is too long."),
    ("invalid_note", "The note is too long."),
    ("cart_not_found", "This cart was already ordered."),
    ("cart_checked_out", "This cart was already ordered."),
    ("discount_unknown", "This discount code is not valid."),
    (
        "discount_inactive",
        "This discount code cannot be used right now.",
    ),
    (
        "discount_currency",
        "This discount code is for another currency.",
    ),
    (
        "discount_exhausted",
        "This discount code has been used as many times as it can be.",
    ),
    (
        "discount_already_used",
        "You have already used this discount code.",
    ),
    (
        "offer_ended",
        "This offer is no longer available: the item was added at its full price.",
    ),
    ("review_invalid_rating", "Choose from one to five stars."),
    (
        "review_invalid_title",
        "The title is at most 100 characters.",
    ),
    (
        "review_invalid_body",
        "Say a little more: 10 to 2000 characters.",
    ),
    ("review_too_many_photos", "Attach at most three photos."),
    (
        "review_invalid_photo",
        "Attach JPEG, PNG, WebP or GIF photos of at most 8 MiB.",
    ),
    ("review_order_not_found", "This order cannot be reviewed."),
    (
        "review_not_purchased",
        "This product is not part of this order.",
    ),
    (
        "review_order_cancelled",
        "A cancelled order cannot be reviewed.",
    ),
    (
        "review_already_reviewed",
        "You have already reviewed this product.",
    ),
    (
        "review_not_pending",
        "The shop has already looked at your review: it can no longer be changed.",
    ),
    ("review_not_found", "This review cannot be found."),
    (
        "review_photo_not_found",
        "One of your photos was taken off meanwhile: try again.",
    ),
    (
        "wrong_code",
        "This is not the code we sent. Check the e-mail and try again.",
    ),
    ("code_expired", "This code has expired: ask for a new one."),
    (
        "too_many_attempts",
        "Too many wrong codes: ask for a new one.",
    ),
    (
        "code_recently_sent",
        "A code was sent less than a minute ago: check your inbox.",
    ),
    (
        "too_many_requests",
        "Too many codes were asked for: try again in a few minutes.",
    ),
    (
        "too_many_addresses",
        "Your account keeps ten addresses: remove one first.",
    ),
    ("signed_out", "You were signed out. Please sign in again."),
    (
        "writes_paused",
        "This shop is temporarily not accepting changes. Please try again in a few minutes.",
    ),
];

/// The countries the checkout lists by default: the EU, then its
/// neighbours and a few more.
const COUNTRIES: &[(&str, &str)] = &[
    ("AT", "Austria"),
    ("BE", "Belgium"),
    ("BG", "Bulgaria"),
    ("HR", "Croatia"),
    ("CY", "Cyprus"),
    ("CZ", "Czechia"),
    ("DK", "Denmark"),
    ("EE", "Estonia"),
    ("FI", "Finland"),
    ("FR", "France"),
    ("DE", "Germany"),
    ("GR", "Greece"),
    ("HU", "Hungary"),
    ("IE", "Ireland"),
    ("IT", "Italy"),
    ("LV", "Latvia"),
    ("LT", "Lithuania"),
    ("LU", "Luxembourg"),
    ("MT", "Malta"),
    ("MC", "Monaco"),
    ("NL", "Netherlands"),
    ("PL", "Poland"),
    ("PT", "Portugal"),
    ("RO", "Romania"),
    ("SK", "Slovakia"),
    ("SI", "Slovenia"),
    ("ES", "Spain"),
    ("SE", "Sweden"),
    ("CA", "Canada"),
    ("NO", "Norway"),
    ("CH", "Switzerland"),
    ("GB", "United Kingdom"),
    ("US", "United States"),
];

/// What the account page says once the shopper signed out everywhere else.
pub const SIGNED_OUT_ELSEWHERE: &str = "signed_out_elsewhere";

macro_rules! text_setter {
    ($($name:ident),* $(,)?) => {
        $(
            pub fn $name(mut self, text: impl Into<Text>) -> Self {
                self.$name = text.into();
                self
            }
        )*
    };
}

impl Messages {
    pub fn english() -> Self {
        Self {
            refusals: ENGLISH
                .iter()
                .map(|(code, text)| (*code, Cow::Borrowed(*text)))
                .collect(),
            fallback: "Something went wrong. Please try again.".into(),
            done: [(
                SIGNED_OUT_ELSEWHERE,
                Cow::Borrowed("You are signed out of every other browser."),
            )]
            .into_iter()
            .collect(),
            order_paid: "Paid".into(),
            order_fulfilled: "Fulfilled".into(),
            order_cancelled: "Cancelled".into(),
            review_pending: "Your review is waiting for the shop to publish it.".into(),
            review_published: "Your review is published. Thank you!".into(),
            review_rejected: "The shop did not publish your review.".into(),
            countries: COUNTRIES
                .iter()
                .map(|(code, name)| (*code, Cow::Borrowed(*name)))
                .collect(),
            default_country: "FR".into(),
            shipping_free: "Free".into(),
            shipping_unavailable: "—".into(),
            shipping_included: "Included".into(),
            vat_prefix: "incl.".into(),
            off_sale_product: "A product no longer sold".into(),
            standard_shipping: "Standard shipping".into(),
            something_went_wrong: "Something went wrong. Please try again.".into(),
        }
    }

    /// Words one refusal code.
    pub fn code(mut self, code: &'static str, text: impl Into<Text>) -> Self {
        self.refusals.insert(code, text.into());
        self
    }

    /// Words several refusal codes at once.
    pub fn codes<T: Into<Text>>(
        mut self,
        table: impl IntoIterator<Item = (&'static str, T)>,
    ) -> Self {
        for (code, text) in table {
            self.refusals.insert(code, text.into());
        }
        self
    }

    /// Words what the account page says once something is done.
    pub fn done(mut self, code: &'static str, text: impl Into<Text>) -> Self {
        self.done.insert(code, text.into());
        self
    }

    /// The countries the checkout lists, in this order.
    pub fn countries<T: Into<Text>>(
        mut self,
        list: impl IntoIterator<Item = (&'static str, T)>,
    ) -> Self {
        self.countries = list
            .into_iter()
            .map(|(code, name)| (code, name.into()))
            .collect();
        self
    }

    pub fn order_statuses(
        mut self,
        paid: impl Into<Text>,
        fulfilled: impl Into<Text>,
        cancelled: impl Into<Text>,
    ) -> Self {
        self.order_paid = paid.into();
        self.order_fulfilled = fulfilled.into();
        self.order_cancelled = cancelled.into();
        self
    }

    pub fn review_statuses(
        mut self,
        pending: impl Into<Text>,
        published: impl Into<Text>,
        rejected: impl Into<Text>,
    ) -> Self {
        self.review_pending = pending.into();
        self.review_published = published.into();
        self.review_rejected = rejected.into();
        self
    }

    text_setter!(
        fallback,
        default_country,
        shipping_free,
        shipping_unavailable,
        shipping_included,
        vat_prefix,
        off_sale_product,
        standard_shipping,
        something_went_wrong,
    );

    /// How a refusal code reads.
    pub fn say(&self, code: &str) -> &str {
        self.refusals
            .get(code)
            .map(Cow::as_ref)
            .unwrap_or(self.fallback.as_ref())
    }

    /// Whether the table words this code (the fallback is not "knowing").
    pub fn knows(&self, code: &str) -> bool {
        self.refusals.contains_key(code)
    }

    /// What the account page says once something is done, if it is a thing
    /// the library knows.
    pub fn say_done(&self, code: &str) -> Option<&str> {
        self.done.get(code).map(Cow::as_ref)
    }

    pub fn order_status(&self, status: OrderStatus) -> &str {
        match status {
            OrderStatus::Paid => &self.order_paid,
            OrderStatus::Fulfilled => &self.order_fulfilled,
            OrderStatus::Cancelled => &self.order_cancelled,
        }
    }

    pub fn review_status(&self, status: ReviewStatus) -> &str {
        match status {
            ReviewStatus::Pending => &self.review_pending,
            ReviewStatus::Published => &self.review_published,
            ReviewStatus::Rejected => &self.review_rejected,
        }
    }

    /// The name of a country the checkout lists.
    pub fn country_name(&self, code: &str) -> Option<&str> {
        self.countries
            .iter()
            .find(|(known, _)| known.eq_ignore_ascii_case(code))
            .map(|(_, name)| name.as_ref())
    }

    /// The codes of the countries the checkout lists, comma-separated: what
    /// a wallet is told it may ship to.
    pub fn country_codes(&self) -> String {
        self.countries
            .iter()
            .map(|(code, _)| *code)
            .collect::<Vec<_>>()
            .join(",")
    }
}

/// How a refusal code reads to this shop's shoppers.
pub fn say<'cx>(cx: &'cx Cx, code: &str) -> &'cx str {
    crate::messages(cx).say(code)
}

/// What the account page says once something is done, if the library
/// knows the code.
pub fn say_done<'cx>(cx: &'cx Cx, code: &str) -> Option<&'cx str> {
    crate::messages(cx).say_done(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_words_every_code_a_shopper_may_meet() {
        let messages = Messages::english();
        let orders = [
            "cart_not_found",
            "cart_checked_out",
            "invalid_quantity",
            "too_many_lines",
            "product_unavailable",
            "variant_required",
            "variant_not_found",
            "mixed_currencies",
            "empty_cart",
            "price_changed",
            "payment_not_found",
            "invalid_email",
            "invalid_vat_number",
            "business_buyers_refused",
            "invalid_ship_name",
            "invalid_address",
            "invalid_country",
            "invalid_phone",
            "invalid_note",
            "shipping_unavailable",
            crate::PAYMENTS_UNAVAILABLE,
            crate::SHOPPER_SIGNED_OUT,
            crate::OFFER_ENDED,
        ];
        for code in orders
            .iter()
            .chain(timada_shoppers::SHOPPER_CODES)
            .chain(timada_reviews::SHOPPER_CODES)
            .chain(timada_discounts::SHOPPER_CODES)
        {
            assert!(messages.knows(code), "no wording for {code}");
        }
        assert_eq!(messages.say("nope"), messages.fallback);
    }

    #[test]
    fn an_app_overrides_what_it_wants() {
        let messages = Messages::english()
            .code("empty_cart", "Votre panier est vide.")
            .shipping_free("Offerte")
            .countries([("FR", "France"), ("BE", "Belgique")]);
        assert_eq!(messages.say("empty_cart"), "Votre panier est vide.");
        assert_eq!(
            messages.say("invalid_email"),
            "Enter a valid e-mail address."
        );
        assert_eq!(messages.shipping_free, "Offerte");
        assert_eq!(messages.country_codes(), "FR,BE");
        assert_eq!(messages.country_name("be"), Some("Belgique"));
    }
}
