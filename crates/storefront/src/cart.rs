//! Selling: the shopper's cart and the order it becomes.
//!
//! Unlike the catalog, none of this is read from the local copy. A cart and
//! an order are written to the store and read back from it, so a page shows
//! a change at once; a cart's prices are worked out from the products in the
//! store too ([`timada_orders::Command::quote_cart`]), so the total a cart
//! page shows is exactly what a payment asked for now charges. The local copy only
//! adds what a page needs to link and illustrate a line (its slug and
//! picture).
//!
//! Paying goes through the dashboard, which holds the merchant's payment
//! provider account: [`start_payment`] asks to pay and waits for the
//! dashboard to open a payment (or refuse), the shopper's browser confirms
//! it with the provider's card form, and the order exists once the provider
//! tells the dashboard it went through ([`order_of_cart`]). A storefront
//! never creates an order itself.
//!
//! Writing needs a key holding `store:checkout` (or `store:write`); with a
//! read-only key every call here fails.
//!
//! Each call returns two layers: the outer `Result` is a failure of the store
//! (a topcoat error, so `?` turns it into a 500), the inner one a refusal to
//! show the shopper ([`OrdersError::code`] names it). The same calls exist
//! as [`Store`] methods for code that runs outside a request.
//!
//! A cart takes one discount code ([`apply_discount`]). The cart page shows
//! what it takes off, or why it takes nothing ([`CartView::discount_refused`]);
//! a code that stopped working by the time the shopper pays is refused with
//! its reason rather than dropped, and so is one a capped code has no use
//! left for or the shopper's e-mail already used.
//!
//! The library keeps no notion of whose cart is whose: the host remembers
//! the cart id, typically in a cookie.

use topcoat_core::context::Cx;
use topcoat_core::error::{Error, Result};

pub use timada_catalog::format_money;
use timada_invoicing::tax;
use timada_orders::{CartItem, CartLine, Command, PaymentAnswer, STOREFRONT_ACTOR};
pub use timada_orders::{
    Checkout, OrderDiscount, OrderLine, OrderState, OrderStatus, OrdersError, PAYMENTS_UNAVAILABLE,
    PaymentSetup, ShippingAddress,
};

use crate::context::store;
use crate::query::{Product, published_products_by_ids};
use crate::store::Store;

/// How long [`start_payment`] waits for the dashboard's answer.
const PAYMENT_ANSWER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
/// How often it looks again when no write wakes it.
const PAYMENT_ANSWER_POLL: std::time::Duration = std::time::Duration::from_millis(250);

/// A payment open with the merchant's provider: what the card form needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaymentSession {
    /// The request it answers; [`payment_session`] reads it back by it.
    pub request_id: String,
    /// `stripe`.
    pub provider: String,
    pub payment_id: String,
    /// What the browser confirms the payment with.
    pub client_secret: String,
    /// The merchant's public key the card form loads with.
    pub publishable_key: String,
    /// What the order will hold, priced.
    pub lines: Vec<OrderLine>,
    pub currency: String,
    /// What is charged: the lines, less the discount.
    pub total_cents: u64,
    /// The discount code the payment is priced with, if any.
    pub discount: Option<OrderDiscount>,
}

impl PaymentSession {
    /// The total as a person reads it (`24.00 EUR`).
    pub fn total(&self) -> String {
        timada_catalog::format_money(self.total_cents, &self.currency)
    }

    /// What the discount takes off, as a person reads it (`-2.50 EUR`).
    pub fn discount_amount(&self) -> Option<String> {
        let discount = self.discount.as_ref()?;
        Some(format!(
            "-{}",
            timada_catalog::format_money(discount.amount_cents, &self.currency)
        ))
    }
}

/// A refusal to show the shopper, or the outcome.
pub type Refusable<T> = std::result::Result<T, OrdersError>;

/// One line of a cart, ready for a page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CartLineView {
    pub product_id: String,
    pub variant_id: Option<String>,
    /// The product's name; empty for a product the store no longer has.
    pub name: String,
    /// The variant's values (`Beige / Large`).
    pub variant_label: Option<String>,
    pub sku: Option<String>,
    /// The product's handle, when it is still on sale here.
    pub slug: Option<String>,
    /// The variant's picture, else the product's first one.
    pub image_url: Option<String>,
    pub quantity: u32,
    /// What kind of goods it is for VAT (`standard`, `books`, …).
    pub tax_category: String,
    /// `None` on a line that cannot be sold any more.
    pub unit_amount_cents: Option<u64>,
    pub amount_cents: Option<u64>,
}

/// A cart priced from the store as it stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CartView {
    pub id: String,
    /// The lines an order would hold.
    pub lines: Vec<CartLineView>,
    /// The lines whose product or variant is off sale: the shopper removes
    /// them before checking out.
    pub unavailable: Vec<CartLineView>,
    /// `None` while the cart holds nothing that can be sold.
    pub currency: Option<String>,
    /// What the lines add up to.
    pub subtotal_cents: u64,
    /// The discount code the shopper entered, whether or not it takes
    /// anything off.
    pub discount_code: Option<String>,
    /// What the code takes off, when it does.
    pub discount: Option<OrderDiscount>,
    /// Why the code takes nothing off (`discount_inactive`,
    /// `discount_currency`, …), to word on the page.
    pub discount_refused: Option<&'static str>,
    /// What paying now charges: the subtotal, less the discount.
    pub total_cents: u64,
    /// Items on the lines that can be sold.
    pub item_count: u32,
    /// The order it became, once checked out.
    pub order_id: Option<String>,
}

impl CartView {
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty() && self.unavailable.is_empty()
    }

    /// The total as a person reads it (`24.00 EUR`).
    pub fn total(&self) -> Option<String> {
        let currency = self.currency.as_deref()?;
        Some(timada_catalog::format_money(self.total_cents, currency))
    }

    /// The subtotal as a person reads it.
    pub fn subtotal(&self) -> Option<String> {
        let currency = self.currency.as_deref()?;
        Some(timada_catalog::format_money(self.subtotal_cents, currency))
    }

    /// What the discount takes off, as a person reads it (`-2.50 EUR`).
    pub fn discount_amount(&self) -> Option<String> {
        let currency = self.currency.as_deref()?;
        let discount = self.discount.as_ref()?;
        Some(format!(
            "-{}",
            timada_catalog::format_money(discount.amount_cents, currency)
        ))
    }

    /// The VAT the total includes when the goods go to `country` (ISO
    /// 3166-1 alpha-2), at that destination's rates for what each line is.
    /// An estimate for a page to show: the invoice decides, from the
    /// merchant's VAT standing, the year's sales abroad, the postal code and
    /// the buyer's VAT number. `None` when no VAT would apply (outside the
    /// EU) or the cart has nothing priced.
    pub fn vat_estimate(&self, country: &str) -> Option<VatEstimate> {
        let currency = self.currency.as_deref()?;
        let lines: Vec<(&str, u64)> = self
            .lines
            .iter()
            .map(|line| {
                (
                    line.tax_category.as_str(),
                    line.amount_cents.unwrap_or_default(),
                )
            })
            .collect();
        let decision = tax::decide(
            tax::TaxProfile {
                vat_registered: true,
                oss_registered: false,
            },
            &tax::Destination {
                country: country.to_ascii_uppercase(),
                ..Default::default()
            },
            &lines,
            currency,
            tax::DistanceSales::default(),
        );
        let sums: Vec<(u32, u64)> = decision
            .rates
            .iter()
            .zip(&lines)
            .filter_map(|(rate, (_, amount))| rate.map(|rate| (rate, *amount)))
            .collect();
        if sums.is_empty() {
            return None;
        }
        // The discount comes off every rate in proportion.
        let subtotals =
            timada_invoicing::prorate(&timada_invoicing::vat_breakdown(sums), self.total_cents);
        let rate_bps = match subtotals.as_slice() {
            [only] => Some(only.rate_bps),
            _ => None,
        };
        Some(VatEstimate {
            country: decision.tax_country,
            rate_bps,
            vat_cents: subtotals.iter().map(|subtotal| subtotal.vat_cents).sum(),
            currency: currency.to_owned(),
        })
    }

    /// Whether [`start_payment`] can go ahead: something to sell, nothing off
    /// sale left in it, not ordered already.
    pub fn can_check_out(&self) -> bool {
        !self.lines.is_empty() && self.unavailable.is_empty() && self.order_id.is_none()
    }
}

/// The VAT a cart's total includes, as [`CartView::vat_estimate`] works it out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VatEstimate {
    /// The country whose VAT applies (`FR`).
    pub country: String,
    /// In basis points; `None` when the lines are taxed at several rates.
    pub rate_bps: Option<u32>,
    pub vat_cents: u64,
    pub currency: String,
}

impl VatEstimate {
    /// The VAT as a person reads it (`9.97 EUR`).
    pub fn amount(&self) -> String {
        timada_catalog::format_money(self.vat_cents, &self.currency)
    }

    /// `VAT FR 20%`, or `VAT FR` over several rates.
    pub fn label(&self) -> String {
        match self.rate_bps {
            Some(rate) if rate % 100 == 0 => format!("VAT {} {}%", self.country, rate / 100),
            Some(rate) => format!(
                "VAT {} {}%",
                self.country,
                format!("{:.2}", f64::from(rate) / 100.0).trim_end_matches('0')
            ),
            None => format!("VAT {}", self.country),
        }
    }
}

/// Splits an outcome into a store failure and a refusal.
fn refusable<T>(outcome: std::result::Result<T, OrdersError>) -> anyhow::Result<Refusable<T>> {
    match outcome {
        Err(err) if !err.is_user_facing() => Err(err.into()),
        outcome => Ok(outcome),
    }
}

impl Store {
    fn orders(&self) -> Command<evento::Evento> {
        Command(self.executor().clone())
    }

    /// See [`cart`].
    pub async fn cart(&self, cart_id: &str) -> anyhow::Result<Option<CartView>> {
        let orders = self.orders();
        let Some(state) = orders.load_cart(cart_id).await? else {
            return Ok(None);
        };
        let quote = orders
            .quote_cart(cart_id)
            .await
            .map_err(anyhow::Error::from)?;
        let ids: Vec<&str> = state.lines.iter().map(|l| l.product_id.as_str()).collect();
        let mut products = published_products_by_ids(self.pool(), &ids).await?;
        for product in &mut products {
            product.fill_urls(self.media_url());
        }
        let product = |id: &str| products.iter().find(|p| p.id == id);

        let lines = quote
            .lines
            .iter()
            .zip(&quote.tax_categories)
            .map(|(line, tax_category)| {
                let local = product(&line.product_id);
                CartLineView {
                    product_id: line.product_id.clone(),
                    variant_id: line.variant_id.clone(),
                    name: line.name.clone(),
                    variant_label: line.variant_label.clone(),
                    sku: line.sku.clone(),
                    slug: local.map(|p| p.slug.clone()),
                    image_url: local.and_then(|p| picture(p, line.variant_id.as_deref())),
                    quantity: line.quantity,
                    tax_category: tax_category.clone(),
                    unit_amount_cents: Some(line.unit_amount_cents),
                    amount_cents: Some(line.amount_cents()),
                }
            })
            .collect();
        let unavailable = quote
            .unavailable
            .iter()
            .map(|line: &CartLine| {
                let local = product(&line.product_id);
                CartLineView {
                    product_id: line.product_id.clone(),
                    variant_id: line.variant_id.clone(),
                    name: local.map(|p| p.name.clone()).unwrap_or_default(),
                    variant_label: None,
                    sku: None,
                    slug: local.map(|p| p.slug.clone()),
                    image_url: local.and_then(|p| picture(p, None)),
                    quantity: line.quantity,
                    tax_category: timada_catalog::TAX_CATEGORY_STANDARD.to_owned(),
                    unit_amount_cents: None,
                    amount_cents: None,
                }
            })
            .collect();
        Ok(Some(CartView {
            id: state.id.clone(),
            item_count: quote.item_count(),
            lines,
            unavailable,
            currency: quote.currency,
            subtotal_cents: quote.subtotal_cents,
            discount_code: state.discount_code,
            discount: quote.discount,
            discount_refused: quote.discount_refused,
            total_cents: quote.total_cents,
            order_id: state.order_id,
        }))
    }

    /// See [`apply_discount`].
    pub async fn apply_discount(&self, cart_id: &str, code: &str) -> anyhow::Result<Refusable<()>> {
        refusable(
            self.orders()
                .apply_discount(STOREFRONT_ACTOR, cart_id, code)
                .await
                .map(|_| ()),
        )
    }

    /// See [`remove_discount`].
    pub async fn remove_discount(&self, cart_id: &str) -> anyhow::Result<Refusable<()>> {
        refusable(
            self.orders()
                .remove_discount(STOREFRONT_ACTOR, cart_id)
                .await
                .map(|_| ()),
        )
    }

    /// See [`add_to_cart`].
    pub async fn add_to_cart(
        &self,
        cart_id: Option<&str>,
        product_id: &str,
        variant_id: Option<&str>,
        quantity: u32,
    ) -> anyhow::Result<Refusable<String>> {
        let orders = self.orders();
        let item = CartItem {
            product_id: product_id.to_owned(),
            variant_id: variant_id.map(str::to_owned),
            quantity,
        };
        if let Some(cart_id) = cart_id {
            match orders
                .add_to_cart(STOREFRONT_ACTOR, cart_id, item.clone())
                .await
            {
                Ok(_) => return Ok(Ok(cart_id.to_owned())),
                Err(OrdersError::CartNotFound | OrdersError::CheckedOut) => {}
                Err(err) => return refusable(Err(err)),
            }
        }
        let cart_id = match orders.create_cart(STOREFRONT_ACTOR).await {
            Ok(id) => id,
            Err(err) => return refusable(Err(err)),
        };
        let added = orders.add_to_cart(STOREFRONT_ACTOR, &cart_id, item).await;
        refusable(added.map(|_| cart_id))
    }

    /// See [`set_cart_quantity`].
    pub async fn set_cart_quantity(
        &self,
        cart_id: &str,
        product_id: &str,
        variant_id: Option<&str>,
        quantity: u32,
    ) -> anyhow::Result<Refusable<()>> {
        let item = CartItem {
            product_id: product_id.to_owned(),
            variant_id: variant_id.map(str::to_owned),
            quantity,
        };
        refusable(
            self.orders()
                .set_cart_quantity(STOREFRONT_ACTOR, cart_id, item)
                .await
                .map(|_| ()),
        )
    }

    /// See [`start_payment`].
    pub async fn start_payment(
        &self,
        cart_id: &str,
        checkout: Checkout,
    ) -> anyhow::Result<Refusable<PaymentSession>> {
        let orders = self.orders();
        let request_id = match orders
            .request_checkout(STOREFRONT_ACTOR, cart_id, checkout)
            .await
        {
            Ok(id) => id,
            Err(err) => return refusable(Err(err)),
        };
        let mut watch = evento::Executor::write_watch(self.executor());
        let deadline = tokio::time::Instant::now() + PAYMENT_ANSWER_TIMEOUT;
        loop {
            match self.payment_answer(cart_id, &request_id).await? {
                Some(answer) => return Ok(answer),
                None if tokio::time::Instant::now() >= deadline => {
                    tracing::warn!(cart_id, request_id, "no answer to a request to pay");
                    return Ok(Err(OrdersError::PaymentRefused {
                        code: PAYMENTS_UNAVAILABLE.to_owned(),
                    }));
                }
                None => {}
            }
            // A write to the store wakes us; the poll covers a missed one.
            let wait = PAYMENT_ANSWER_POLL;
            match watch.as_mut() {
                Some(watch) => {
                    let _ = tokio::time::timeout(wait, watch.changed()).await;
                }
                None => tokio::time::sleep(wait).await,
            }
        }
    }

    /// See [`payment_setup`].
    pub async fn payment_setup(&self) -> anyhow::Result<Option<PaymentSetup>> {
        self.orders().payment_setup().await
    }

    /// See [`accepts_business_buyers`].
    pub async fn accepts_business_buyers(&self) -> anyhow::Result<bool> {
        self.orders().accepts_business_buyers().await
    }

    /// See [`payment_session`].
    pub async fn payment_session(
        &self,
        cart_id: &str,
        request_id: &str,
    ) -> anyhow::Result<Option<PaymentSession>> {
        Ok(self
            .payment_answer(cart_id, request_id)
            .await?
            .and_then(Result::ok))
    }

    /// The answer to a request to pay, once there is one.
    async fn payment_answer(
        &self,
        cart_id: &str,
        request_id: &str,
    ) -> anyhow::Result<Option<Refusable<PaymentSession>>> {
        let Some(cart) = self.orders().load_cart(cart_id).await? else {
            return Ok(None);
        };
        let Some(request) = cart.request(request_id) else {
            return Ok(None);
        };
        Ok(match &request.answer {
            None => None,
            Some(PaymentAnswer::Refused(code)) => {
                Some(Err(OrdersError::PaymentRefused { code: code.clone() }))
            }
            Some(PaymentAnswer::Requested(payment)) => Some(Ok(PaymentSession {
                request_id: request.request_id.clone(),
                provider: payment.provider.clone(),
                payment_id: payment.payment_id.clone(),
                client_secret: payment.client_secret.clone(),
                publishable_key: payment.publishable_key.clone(),
                lines: payment.lines.clone(),
                currency: payment.currency.clone(),
                total_cents: payment.total_cents,
                discount: payment.discount.clone(),
            })),
        })
    }

    /// See [`order_of_cart`].
    pub async fn order_of_cart(&self, cart_id: &str) -> anyhow::Result<Option<OrderState>> {
        self.orders().load_order_of_cart(cart_id).await
    }

    /// See [`order`].
    pub async fn order(&self, order_id: &str) -> anyhow::Result<Option<OrderState>> {
        self.orders().load_order(order_id).await
    }
}

/// The picture of a line: its variant's, else the product's first.
fn picture(product: &Product, variant_id: Option<&str>) -> Option<String> {
    let variant_image = variant_id
        .and_then(|id| product.variants.iter().find(|v| v.id == id))
        .and_then(|v| v.image_id.as_deref())
        .and_then(|id| product.images.iter().find(|image| image.id == id));
    variant_image
        .or_else(|| product.image())
        .map(|image| image.url.clone())
}

/// The cart, priced, or `None` for an id the store does not know.
pub async fn cart(cx: &Cx, cart_id: &str) -> Result<Option<CartView>> {
    store(cx).cart(cart_id).await.map_err(Error::from_anyhow)
}

/// Adds `quantity` of a product (one variant of it, when it sells variants)
/// to the cart and returns the cart's id: `cart_id` itself, or a new cart
/// when there is none yet or it was already ordered or is unknown.
pub async fn add_to_cart(
    cx: &Cx,
    cart_id: Option<&str>,
    product_id: &str,
    variant_id: Option<&str>,
    quantity: u32,
) -> Result<Refusable<String>> {
    store(cx)
        .add_to_cart(cart_id, product_id, variant_id, quantity)
        .await
        .map_err(Error::from_anyhow)
}

/// Sets how many of a line the cart holds; zero removes it.
pub async fn set_cart_quantity(
    cx: &Cx,
    cart_id: &str,
    product_id: &str,
    variant_id: Option<&str>,
    quantity: u32,
) -> Result<Refusable<()>> {
    store(cx)
        .set_cart_quantity(cart_id, product_id, variant_id, quantity)
        .await
        .map_err(Error::from_anyhow)
}

/// Takes a line out of the cart.
pub async fn remove_from_cart(
    cx: &Cx,
    cart_id: &str,
    product_id: &str,
    variant_id: Option<&str>,
) -> Result<Refusable<()>> {
    set_cart_quantity(cx, cart_id, product_id, variant_id, 0).await
}

/// Puts a discount code on the cart, replacing the one it had. A code the
/// shop has not is refused (`discount_unknown`); one that takes nothing off
/// right now stays on, and [`CartView::discount_refused`] says why.
pub async fn apply_discount(cx: &Cx, cart_id: &str, code: &str) -> Result<Refusable<()>> {
    store(cx)
        .apply_discount(cart_id, code)
        .await
        .map_err(Error::from_anyhow)
}

/// Takes the discount code off the cart.
pub async fn remove_discount(cx: &Cx, cart_id: &str) -> Result<Refusable<()>> {
    store(cx)
        .remove_discount(cart_id)
        .await
        .map_err(Error::from_anyhow)
}

/// Asks to pay for the cart, shipping where `checkout` says, and waits (up
/// to fifteen seconds) for the dashboard to open a payment with the
/// merchant's provider: the session is what the page loads the card form
/// with. Pass the total the shopper was shown as
/// [`Checkout::expected_total_cents`]: a price that moved since is refused
/// (`price_changed`) rather than charged unseen. A refusal from the
/// dashboard is [`OrdersError::PaymentRefused`], its code what to word
/// (`payments_unavailable` when the merchant takes no payments, the provider
/// failed or nobody answered).
pub async fn start_payment(
    cx: &Cx,
    cart_id: &str,
    checkout: Checkout,
) -> Result<Refusable<PaymentSession>> {
    store(cx)
        .start_payment(cart_id, checkout)
        .await
        .map_err(Error::from_anyhow)
}

/// The open payment of a request [`start_payment`] made, for a page shown
/// after it (the card form's page, typically); `None` once the request is
/// unknown or was refused.
pub async fn payment_session(
    cx: &Cx,
    cart_id: &str,
    request_id: &str,
) -> Result<Option<PaymentSession>> {
    store(cx)
        .payment_session(cart_id, request_id)
        .await
        .map_err(Error::from_anyhow)
}

/// The order the cart became, once its payment went through. The provider
/// tells the dashboard, not the shopper's browser, so a page coming back
/// from the card form may have to wait a moment for it.
pub async fn order_of_cart(cx: &Cx, cart_id: &str) -> Result<Option<OrderState>> {
    store(cx)
        .order_of_cart(cart_id)
        .await
        .map_err(Error::from_anyhow)
}

/// An order, read from the store. Anyone holding its id can read it: the
/// host decides who may (the browser that placed it, typically).
pub async fn order(cx: &Cx, order_id: &str) -> Result<Option<OrderState>> {
    store(cx).order(order_id).await.map_err(Error::from_anyhow)
}

/// How the shop's shoppers pay (the provider and the merchant's public key),
/// so a page can show the payment form before asking to pay; `None` while
/// the shop takes no payments.
pub async fn payment_setup(cx: &Cx) -> Result<Option<PaymentSetup>> {
    store(cx).payment_setup().await.map_err(Error::from_anyhow)
}

/// Whether the shop sells to businesses: a shopper may then give an EU VAT
/// number, for an invoice without VAT. A checkout form asks for one only
/// then; the store refuses one otherwise (`business_buyers_refused`).
pub async fn accepts_business_buyers(cx: &Cx) -> Result<bool> {
    store(cx)
        .accepts_business_buyers()
        .await
        .map_err(Error::from_anyhow)
}
