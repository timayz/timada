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
//! The library keeps no notion of whose cart is whose: the host remembers
//! the cart id, typically in a cookie.

use topcoat_core::context::Cx;
use topcoat_core::error::{Error, Result};

pub use timada_catalog::format_money;
use timada_orders::{CartItem, CartLine, Command, PaymentAnswer, STOREFRONT_ACTOR};
pub use timada_orders::{
    Checkout, OrderLine, OrderState, OrderStatus, OrdersError, PAYMENTS_UNAVAILABLE,
    ShippingAddress,
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
    pub total_cents: u64,
}

impl PaymentSession {
    /// The total as a person reads it (`24.00 EUR`).
    pub fn total(&self) -> String {
        timada_catalog::format_money(self.total_cents, &self.currency)
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

    /// Whether [`start_payment`] can go ahead: something to sell, nothing off
    /// sale left in it, not ordered already.
    pub fn can_check_out(&self) -> bool {
        !self.lines.is_empty() && self.unavailable.is_empty() && self.order_id.is_none()
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
            .map(|line| {
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
            total_cents: quote.total_cents,
            order_id: state.order_id,
        }))
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
