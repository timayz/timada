//! Identifiers, limits, the persisted value types and the checks on what a
//! shopper types.

use rand::Rng;

use crate::OrderError;

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
/// The longest tracking link a fulfilment may give.
pub const MAX_TRACKING_URL_LEN: usize = 500;
pub const MAX_REASON_LEN: usize = 500;
pub const MAX_SHIPPING_NAME_LEN: usize = 80;
/// The longest click id, campaign parameter, referrer or landing path kept
/// with an order; a longer one is dropped.
pub const MAX_SOURCE_VALUE_LEN: usize = 500;
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
#[derive(
    Debug,
    Clone,
    Default,
    PartialEq,
    Eq,
    bitcode::Encode,
    bitcode::Decode,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct ShippingAddress {
    /// The person or company the parcel is for.
    pub name: String,
    pub line1: String,
    pub line2: Option<String>,
    pub postal_code: String,
    pub city: String,
    /// The province or region the shopper picked from the shop's list, when
    /// it shows one (a shop forwarding to AliExpress); it may read otherwise
    /// than the city's own spelling, and goes on no document.
    #[serde(default)]
    pub region: Option<String>,
    /// ISO 3166-1 alpha-2, uppercase (`FR`).
    pub country: String,
    pub phone: Option<String>,
}

/// One arrival at the shop: the click ids and campaign parameters its
/// address carried, and where it came from. Frozen in `events.lock`: the
/// fields never change.
#[derive(
    Debug,
    Clone,
    Default,
    PartialEq,
    Eq,
    bitcode::Encode,
    bitcode::Decode,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(default)]
pub struct Click {
    /// Google Ads' click id.
    pub gclid: Option<String>,
    /// Google Ads' ids for a click from an iOS app, or to one, which carry
    /// no `gclid`.
    pub gbraid: Option<String>,
    pub wbraid: Option<String>,
    /// Meta's click id.
    pub fbclid: Option<String>,
    pub utm_source: Option<String>,
    pub utm_medium: Option<String>,
    pub utm_campaign: Option<String>,
    pub utm_term: Option<String>,
    pub utm_content: Option<String>,
    /// The page the visitor came from, on another site, without its query.
    pub referrer: Option<String>,
    /// The path of the shop they landed on.
    pub landing: Option<String>,
    /// Unix seconds.
    pub at: u64,
}

impl Click {
    fn values_mut(&mut self) -> [&mut Option<String>; 11] {
        [
            &mut self.gclid,
            &mut self.gbraid,
            &mut self.wbraid,
            &mut self.fbclid,
            &mut self.utm_source,
            &mut self.utm_medium,
            &mut self.utm_campaign,
            &mut self.utm_term,
            &mut self.utm_content,
            &mut self.referrer,
            &mut self.landing,
        ]
    }

    /// Whether it says where the visitor came from: a landing path alone
    /// does not.
    pub fn tells(&self) -> bool {
        self.gclid.is_some()
            || self.gbraid.is_some()
            || self.wbraid.is_some()
            || self.fbclid.is_some()
            || self.utm_source.is_some()
            || self.utm_medium.is_some()
            || self.utm_campaign.is_some()
            || self.utm_term.is_some()
            || self.utm_content.is_some()
            || self.referrer.is_some()
    }

    /// The channel it names, as `source / medium`: what the campaign's
    /// parameters say when it has some, else `google / cpc` for a Google
    /// Ads click, `facebook / fbclid` for a click Meta tagged (an ad or a
    /// plain post: its id does not say), else the site it came from.
    pub fn channel(&self) -> String {
        if self.utm_source.is_some() || self.utm_medium.is_some() {
            return format!(
                "{} / {}",
                self.utm_source.as_deref().unwrap_or("(not set)"),
                self.utm_medium.as_deref().unwrap_or("(not set)")
            )
            .to_lowercase();
        }
        if self.gclid.is_some() || self.gbraid.is_some() || self.wbraid.is_some() {
            return "google / cpc".to_owned();
        }
        if self.fbclid.is_some() {
            return "facebook / fbclid".to_owned();
        }
        match self.referrer.as_deref().and_then(host_of) {
            Some(host) => format!("{host} / referral"),
            None => "(not set)".to_owned(),
        }
    }
}

/// The host of an `http(s)` address, lowercase, without `www.`.
fn host_of(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let host = rest.split(['/', '?', '#']).next()?;
    let host = host.rsplit('@').next()?.split(':').next()?.to_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    (!host.is_empty()).then(|| host.to_owned())
}

/// What brought the shopper of an order: the first arrival the storefront
/// remembers and the last before they paid (the same one when there was
/// only one). Frozen in `events.lock`: the fields never change.
#[derive(
    Debug,
    Clone,
    Default,
    PartialEq,
    Eq,
    bitcode::Encode,
    bitcode::Decode,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct OrderSource {
    pub first: Click,
    pub last: Click,
}

impl OrderSource {
    /// The channel the order is counted under: the last arrival's, as ad
    /// platforms attribute a sale.
    pub fn channel(&self) -> String {
        self.last.channel()
    }
}

/// The shopper's browser as an advertising platform matches a sale to it,
/// given by the storefront only when the shopper allowed advertising: an
/// order that has one may be told to the shop's advertising platforms from
/// the server, an order without is told to none. Frozen in `events.lock`:
/// the fields never change.
#[derive(
    Debug,
    Clone,
    Default,
    PartialEq,
    Eq,
    bitcode::Encode,
    bitcode::Decode,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(default)]
pub struct Browser {
    /// The browser's `User-Agent`.
    pub user_agent: Option<String>,
    /// Meta's id for the browser, as its pixel keeps it in `_fbp`.
    pub fbp: Option<String>,
    /// Meta's click id, as its pixel keeps it in `_fbc`.
    pub fbc: Option<String>,
    /// The address of the shop's page the checkout was on.
    pub page: Option<String>,
}

impl Browser {
    fn values_mut(&mut self) -> [&mut Option<String>; 4] {
        [
            &mut self.user_agent,
            &mut self.fbp,
            &mut self.fbc,
            &mut self.page,
        ]
    }
}

/// One step of a parcel's journey, as the carrier words it. Frozen in
/// `events.lock`: the fields never change.
#[derive(
    Debug,
    Clone,
    Default,
    PartialEq,
    Eq,
    bitcode::Encode,
    bitcode::Decode,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct Checkpoint {
    /// When, as the carrier gives it (`2026-11-09 08:00:00`): its clock and
    /// its wording, kept as they came.
    pub at: String,
    /// Where, when the carrier says (`Shenzhen`).
    pub place: String,
    /// What happened (`Departed from origin country`).
    pub description: String,
}

/// One line of an order, as it was sold. Frozen in `events.lock`: the fields
/// never change.
#[derive(
    Debug,
    Clone,
    Default,
    PartialEq,
    Eq,
    bitcode::Encode,
    bitcode::Decode,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct OrderLine {
    pub product_id: String,
    pub variant_id: Option<String>,
    /// The product's name when it was sold.
    pub name: String,
    /// The variant's values when it was sold, as the shop showed them
    /// (`Sable / Large`).
    pub variant_label: Option<String>,
    /// The merchant's code for what was sold, if it had one.
    pub sku: Option<String>,
    /// The price of one, in minor units of the order's currency.
    pub unit_amount_cents: u64,
    pub quantity: u32,
    /// What kind of goods it is for VAT (`standard`, `books`, ...), as the
    /// product said when it was sold: the invoice reads it here, not from
    /// the product as it is now. Empty reads as `standard`
    /// ([`tax_category`](Self::tax_category)).
    pub tax_category: String,
    /// How long delivery takes, as the shop said when it was sold: the
    /// product's own time, else the shop's default; `None` when neither.
    #[serde(default)]
    pub delivery_days: Option<timada_catalog::DeliveryDays>,
}

/// What a sale cost the shop, as it stood when the payment was asked for:
/// what the order keeps, so that what it left can be told later whatever
/// the products cost by then. Nothing a shopper or a storefront is ever
/// told. Frozen in `events.lock`: the fields never change.
#[derive(
    Debug,
    Clone,
    Default,
    PartialEq,
    Eq,
    bitcode::Encode,
    bitcode::Decode,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct OrderCosts {
    /// What the shop paid for one of each line, in the lines' order; `None`
    /// for a line whose cost it had not said.
    pub unit_costs: Vec<Option<u64>>,
    /// What sending the goods cost: once for all those sent the shop's
    /// usual way, and once more for each product sent its own.
    pub shipping_cents: u64,
    /// What the border charged, once a product.
    pub import_cents: u64,
    /// The share of the shop's fixed costs the sale carries.
    pub fixed_cents: u64,
    /// What the payment provider keeps of the total.
    pub payment_fee_cents: u64,
    /// The VAT the sale was expected to carry when it was made, by the
    /// rate of each line where it goes. The invoice says what it carried:
    /// this one is there before any invoice is.
    pub vat_cents: u64,
}

impl OrderCosts {
    /// What a cart priced as `lines` for `total_cents` in `currency` costs
    /// the shop: `unit_costs` says what it pays for a line's product
    /// ([`UnitCosts`](timada_catalog::UnitCosts)), `shop` what a sale costs
    /// it beyond the goods. `None` when it said the cost of none of the
    /// lines (nothing worth keeping), or when its sale costs are in another
    /// currency (nothing true can be said). The VAT is left at nothing:
    /// whoever knows the tax fills it in.
    pub fn reckon(
        lines: &[OrderLine],
        unit_costs: impl Fn(&OrderLine) -> timada_catalog::UnitCosts,
        shop: Option<&timada_catalog::ShopCosts>,
        currency: &str,
        total_cents: u64,
    ) -> Option<Self> {
        let units: Vec<timada_catalog::UnitCosts> = lines.iter().map(unit_costs).collect();
        if units.iter().all(|unit| unit.cost_cents.is_none())
            || shop.is_some_and(|shop| shop.currency != currency)
        {
            return None;
        }
        // The border and the carrier charge by product, however many of it
        // and however many lines an offer split it into.
        let mut products: Vec<(&str, timada_catalog::UnitCosts)> = Vec::new();
        for (line, unit) in lines.iter().zip(&units) {
            if !products.iter().any(|(id, _)| *id == line.product_id) {
                products.push((&line.product_id, *unit));
            }
        }
        let usual_shipping = shop.map_or(0, |shop| shop.shipping_cents);
        let usual_import = shop.map_or(0, |shop| shop.import_cents);
        let sent_apart: u64 = products
            .iter()
            .filter_map(|(_, unit)| unit.shipping_cents)
            .sum();
        let sent_together = products
            .iter()
            .any(|(_, unit)| unit.shipping_cents.is_none());
        Some(Self {
            unit_costs: units.iter().map(|unit| unit.cost_cents).collect(),
            shipping_cents: sent_apart + if sent_together { usual_shipping } else { 0 },
            import_cents: products
                .iter()
                .map(|(_, unit)| unit.import_cents.unwrap_or(usual_import))
                .sum(),
            fixed_cents: shop.map_or(0, |shop| shop.fixed_cents),
            payment_fee_cents: shop.map_or(0, |shop| shop.payment_fee(total_cents)),
            vat_cents: 0,
        })
    }

    /// What the shop paid for the goods of `lines`; `None` when it had not
    /// said the cost of one of them.
    pub fn goods_cents(&self, lines: &[OrderLine]) -> Option<u64> {
        if self.unit_costs.len() != lines.len() {
            return None;
        }
        lines
            .iter()
            .zip(&self.unit_costs)
            .map(|(line, cost)| Some((*cost)? * u64::from(line.quantity)))
            .sum()
    }

    /// Everything the sale cost beyond the goods, VAT included.
    pub fn other_cents(&self) -> u64 {
        self.shipping_cents
            + self.import_cents
            + self.fixed_cents
            + self.payment_fee_cents
            + self.vat_cents
    }

    /// What each of `lines` left of an order paid `total_cents`, in their
    /// order: its share of the total once everything the sale cost beyond
    /// the goods came off (by what the line was sold for, the last line
    /// taking what rounding leaves), less what its own goods cost. They add
    /// up to [`profit_cents`](Self::profit_cents). `None` when the cost of
    /// one of the lines is not known.
    pub fn line_profits(&self, lines: &[OrderLine], total_cents: u64) -> Option<Vec<i64>> {
        self.goods_cents(lines)?;
        let net = total_cents as i128 - self.other_cents() as i128;
        let sold: i128 = lines.iter().map(|line| line.amount_cents() as i128).sum();
        let mut left = net;
        let mut profits = Vec::with_capacity(lines.len());
        for (n, (line, cost)) in lines.iter().zip(&self.unit_costs).enumerate() {
            let share = if n + 1 == lines.len() {
                left
            } else if sold == 0 {
                0
            } else {
                (net * line.amount_cents() as i128).div_euclid(sold)
            };
            left -= share;
            let goods = (*cost)? as i128 * i128::from(line.quantity);
            profits.push((share - goods) as i64);
        }
        Some(profits)
    }

    /// What an order of `lines` paid `total_cents` left; negative when it
    /// lost money, `None` when the cost of one of its lines is not known.
    pub fn profit_cents(&self, lines: &[OrderLine], total_cents: u64) -> Option<i64> {
        let goods = self.goods_cents(lines)?;
        Some(total_cents as i64 - goods as i64 - self.other_cents() as i64)
    }
}

/// The payment of an order: held on the shopper's card, then taken. Frozen
/// in `events.lock`: the fields never change.
#[derive(
    Debug,
    Clone,
    Default,
    PartialEq,
    Eq,
    bitcode::Encode,
    bitcode::Decode,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct Payment {
    /// `stripe`.
    pub provider: String,
    /// The provider's id of the payment.
    pub payment_id: String,
    /// What the shopper's card holds for the order, in minor units of
    /// `currency`: what is taken when it is.
    pub amount_cents: u64,
    pub currency: String,
    /// Unix seconds the hold lapses at, when the provider says (a card's
    /// lasts a week): the money must be taken before.
    pub capture_before: Option<u64>,
}

/// The payment provider the dashboard speaks to.
pub const PROVIDER_STRIPE: &str = "stripe";
/// The refusal of a new cart when the storefront started as many as the
/// dashboard lets it within its window ([`OrderError::TooManyCarts`](crate::OrderError::TooManyCarts)).
pub const TOO_MANY_CARTS: &str = "too_many_carts";
/// The refusal of a request to pay when the merchant has no payments set up,
/// or the provider failed.
pub const PAYMENTS_UNAVAILABLE: &str = "payments_unavailable";
/// The refusal of a request to pay when the merchant has not said who it is
/// in law yet: no sale goes ahead without an invoice that can name it.
pub const INVOICING_NOT_CONFIGURED: &str = "invoicing_not_configured";
/// The refusal a storefront reads for a payment the dashboard cancelled
/// once what its checkout held had lapsed: the shopper asks to pay again.
pub const PAYMENT_EXPIRED: &str = "payment_expired";
/// How long a node that took a request to pay on is the only one to answer
/// it, in seconds (`Cart::CheckoutClaimed`): its own retries, and a restart,
/// go on under the claim meanwhile. Past it, another node may take the
/// request on; by then nobody waits for the answer any more.
pub const CHECKOUT_CLAIM_SECS: u64 = 15 * 60;
/// How long before the hold on the shopper's card lapses its goods are no
/// longer ordered from a supplier: buying them and taking the money must both
/// happen before, even with the provider slow to answer.
pub const HOLD_MARGIN_SECS: u64 = 12 * 60 * 60;
/// How long a hold on a card usually lasts, counted from the order: what an
/// order is held to when the provider did not say.
pub const HOLD_USUAL_SECS: u64 = 7 * 24 * 60 * 60;
/// How long before the hold lapses the merchant is reminded of an order the
/// supplier may have and whose money is still only held: time enough to
/// look at the supplier and take the payment.
pub const HOLD_REMINDER_BEFORE_SECS: u64 = 2 * 24 * 60 * 60;
/// Why an order is cancelled when the hold on the shopper's card lapsed
/// before the money was taken.
pub const HOLD_EXPIRED_REASON: &str =
    "The hold on the shopper's card expired before the payment was taken.";
/// Why an order is cancelled when the shopper cancelled it.
pub const SHOPPER_CANCEL_REASON: &str = "Cancelled by the shopper.";
/// Why an order is cancelled when its hold was cancelled in the provider's
/// own dashboard.
pub const HOLD_CANCELLED_REASON: &str = "The payment was cancelled with the provider.";
/// The refusal of a request to pay for an EU distance sale, from a merchant
/// not registered for the One-Stop Shop, in a currency the European Central
/// Bank publishes no euro rate for: the sale cannot be counted against the
/// yearly threshold, so it would owe the destination's VAT, which only the
/// One-Stop Shop lets the merchant remit ([`timada_invoicing::tax::accepts`]).
pub const OSS_REQUIRED_FOR_CURRENCY: &str = "oss_required_for_currency";

impl OrderLine {
    /// The price of the whole line.
    pub fn amount_cents(&self) -> u64 {
        self.unit_amount_cents * u64::from(self.quantity)
    }

    /// The kind of goods for VAT; a line that says nothing is standard goods.
    pub fn tax_category(&self) -> &str {
        if self.tax_category.is_empty() {
            timada_catalog::TAX_CATEGORY_STANDARD
        } else {
            &self.tax_category
        }
    }
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

/// The stream of where an order's parcels are: one per order, from its id.
pub fn shipment_id(order_id: &str) -> String {
    evento::hash_ids(vec!["shipment", order_id])
}

/// The stream of how the tenant ships: one per tenant store.
pub fn shipping_methods_id() -> String {
    evento::hash_ids(vec!["shipping-methods"])
}

/// The longest return window a shop states, in days.
pub const MAX_RETURN_DAYS: u32 = 365;

/// The stream of the tenant's return policy: one per tenant store.
pub fn return_policy_id() -> String {
    evento::hash_ids(vec!["return-policy"])
}

/// The stream of the tenant's scheduled export of one kind: one per kind in
/// a tenant store.
pub fn export_schedule_id(kind: crate::ExportKind) -> String {
    evento::hash_ids(vec!["export-schedule", kind.key()])
}

/// The longest reason kept of an export that was not taken.
pub const MAX_EXPORT_REASON_LEN: usize = 300;

/// The stream of where the tenant's storefront lives: one per tenant store.
pub fn storefront_id() -> String {
    evento::hash_ids(vec!["storefront"])
}

/// The stream of one mail about an order ([`OrderMail`](crate::aggregator::OrderMail)).
pub fn order_mail_id(order_id: &str, mail: &str) -> String {
    evento::hash_ids(vec!["order-mail", order_id, mail])
}

/// How long after the event that owes it a mail about an order is still
/// worth sending: a day. Longer than the retry schedule of a send
/// ([`timada_core::RETRY_DELAYS_SECS`], about eighteen hours in all), so a
/// shop whose server is down for a while gets every retry; past it the mail
/// is history (a node replaying its events), and the shopper has the
/// order's page.
pub const ORDER_MAIL_WINDOW_SECS: u64 = 24 * 60 * 60;

/// The address of a storefront as the merchant typed it, absolute and
/// without a trailing `/`.
pub fn normalize_storefront_url(url: &str) -> Result<String, OrderError> {
    let url = url.trim().trim_end_matches('/');
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .ok_or(OrderError::InvalidStorefrontUrl)?;
    if rest.is_empty() || url.len() > 200 || url.chars().any(char::is_whitespace) {
        return Err(OrderError::InvalidStorefrontUrl);
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
pub fn normalize_email(email: &str) -> Result<String, OrderError> {
    timada_core::normalize_email(email).ok_or(OrderError::InvalidEmail)
}

/// Trims every field, drops empty optional ones, uppercases the country and
/// checks the lengths.
pub fn normalize_shipping(address: ShippingAddress) -> Result<ShippingAddress, OrderError> {
    let text = |value: &str, max: usize, err: fn() -> OrderError| {
        let value = value.trim();
        if value.is_empty() || value.chars().count() > max || value.chars().any(char::is_control) {
            Err(err())
        } else {
            Ok(value.to_owned())
        }
    };
    let optional = |value: Option<String>, max: usize, err: fn() -> OrderError| match value
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        None => Ok(None),
        Some(value) => text(value, max, err).map(Some),
    };
    let country = address.country.trim().to_ascii_uppercase();
    if country.len() != 2 || !country.bytes().all(|b| b.is_ascii_uppercase()) {
        return Err(OrderError::InvalidCountry);
    }
    Ok(ShippingAddress {
        name: text(&address.name, MAX_SHIP_NAME_LEN, || OrderError::InvalidName)?,
        line1: text(&address.line1, MAX_ADDRESS_LINE_LEN, || {
            OrderError::InvalidAddress
        })?,
        line2: optional(address.line2, MAX_ADDRESS_LINE_LEN, || {
            OrderError::InvalidAddress
        })?,
        postal_code: text(&address.postal_code, MAX_POSTAL_CODE_LEN, || {
            OrderError::InvalidAddress
        })?,
        city: text(&address.city, MAX_CITY_LEN, || OrderError::InvalidAddress)?,
        region: optional(address.region, MAX_CITY_LEN, || OrderError::InvalidAddress)?,
        country,
        phone: optional(address.phone, MAX_PHONE_LEN, || OrderError::InvalidPhone)?,
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
) -> Result<NewShippingMethod, OrderError> {
    let name = method.name.trim();
    if name.is_empty()
        || name.chars().count() > MAX_SHIPPING_NAME_LEN
        || name.chars().any(char::is_control)
    {
        return Err(OrderError::InvalidShippingName);
    }
    let currency = timada_catalog::normalize_currency(&method.currency)
        .map_err(|_| OrderError::InvalidShippingCurrency)?;
    let max = timada_catalog::MAX_AMOUNT_CENTS;
    if method.price_cents > max || method.free_above_cents.is_some_and(|cents| cents > max) {
        return Err(OrderError::InvalidShippingPrice);
    }
    let mut countries = Vec::with_capacity(method.countries.len());
    for country in &method.countries {
        let country = country.trim().to_ascii_uppercase();
        if country.len() != 2 || !country.bytes().all(|b| b.is_ascii_uppercase()) {
            return Err(OrderError::InvalidShippingCountries);
        }
        countries.push(country);
    }
    countries.sort();
    countries.dedup();
    if countries.len() > MAX_SHIPPING_COUNTRIES {
        return Err(OrderError::InvalidShippingCountries);
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
pub fn normalize_billing(address: ShippingAddress) -> Result<ShippingAddress, OrderError> {
    Ok(ShippingAddress {
        phone: None,
        ..normalize_shipping(address)?
    })
}

/// A trimmed optional text of at most `max` characters; blank is `None`.
fn optional_text(
    value: Option<&str>,
    max: usize,
    err: OrderError,
) -> Result<Option<String>, OrderError> {
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
pub fn normalize_buyer_vat_number(vat_number: Option<&str>) -> Result<Option<String>, OrderError> {
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
        Err(OrderError::InvalidVatNumber)
    }
}

pub fn normalize_note(note: Option<&str>) -> Result<Option<String>, OrderError> {
    optional_text(note, MAX_NOTE_LEN, OrderError::InvalidNote)
}

/// A value a storefront passes on from a browser, trimmed; dropped when
/// empty, too long or holding a control character.
fn kept_value(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|text| {
            !text.is_empty()
                && text.chars().count() <= MAX_SOURCE_VALUE_LEN
                && !text.chars().any(char::is_control)
        })
        .map(str::to_owned)
}

/// What a storefront says brought the shopper, as an order keeps it: each
/// value trimmed, and dropped when empty, too long or holding a control
/// character. Nothing is refused: what measures a sale never stops one.
/// `None` when neither arrival says anything.
pub fn normalize_source(source: Option<OrderSource>) -> Option<OrderSource> {
    fn clean(mut click: Click) -> Option<Click> {
        for value in click.values_mut() {
            *value = kept_value(value.as_deref());
        }
        click.tells().then_some(click)
    }
    let source = source?;
    match (clean(source.first), clean(source.last)) {
        (Some(first), Some(last)) => Some(OrderSource { first, last }),
        (Some(only), None) | (None, Some(only)) => Some(OrderSource {
            first: only.clone(),
            last: only,
        }),
        (None, None) => None,
    }
}

/// What a storefront says of the shopper's browser, as an order keeps it:
/// tidied as [`normalize_source`] does, never refused. Kept even when every
/// value was dropped: that the storefront gave one says the shopper allowed
/// advertising.
pub fn normalize_browser(browser: Option<Browser>) -> Option<Browser> {
    let mut browser = browser?;
    for value in browser.values_mut() {
        *value = kept_value(value.as_deref());
    }
    Some(browser)
}

pub fn normalize_tracking(tracking: Option<&str>) -> Result<Option<String>, OrderError> {
    optional_text(tracking, MAX_TRACKING_LEN, OrderError::InvalidTracking)
}

/// The carrier's page for a parcel: an `http(s)` address, or nothing.
pub fn normalize_tracking_url(url: Option<&str>) -> Result<Option<String>, OrderError> {
    let url = optional_text(url, MAX_TRACKING_URL_LEN, OrderError::InvalidTrackingUrl)?;
    match url {
        Some(url) if url.starts_with("https://") || url.starts_with("http://") => Ok(Some(url)),
        Some(_) => Err(OrderError::InvalidTrackingUrl),
        None => Ok(None),
    }
}

/// A cancellation needs a reason.
pub fn normalize_reason(reason: &str) -> Result<String, OrderError> {
    optional_text(Some(reason), MAX_REASON_LEN, OrderError::InvalidReason)?
        .ok_or(OrderError::InvalidReason)
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
            region: None,
            country: "fr".into(),
            phone: None,
        };
        let address = normalize_shipping(address).unwrap();
        assert_eq!(address.name, "Ada Lovelace");
        assert_eq!(address.line2, None);
        assert_eq!(address.country, "FR");

        let no_city = ShippingAddress {
            city: " ".into(),

            region: None,
            ..address.clone()
        };
        assert!(matches!(
            normalize_shipping(no_city),
            Err(OrderError::InvalidAddress)
        ));
        let bad_country = ShippingAddress {
            country: "FRA".into(),
            ..address
        };
        assert!(matches!(
            normalize_shipping(bad_country),
            Err(OrderError::InvalidCountry)
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
                    Err(OrderError::InvalidStorefrontUrl)
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

    #[test]
    fn a_tracking_link_is_an_http_address() {
        assert_eq!(
            normalize_tracking_url(Some(
                " https://global.cainiao.com/newDetail.htm?mailNoList=LP1 "
            ))
            .unwrap()
            .as_deref(),
            Some("https://global.cainiao.com/newDetail.htm?mailNoList=LP1")
        );
        assert_eq!(normalize_tracking_url(Some("")).unwrap(), None);
        assert!(normalize_tracking_url(Some("global.cainiao.com")).is_err());
        assert!(normalize_tracking_url(Some(&format!("https://{}", "a".repeat(500)))).is_err());
    }
}
