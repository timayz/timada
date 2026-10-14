//! Selling: the shopper's cart and the order it becomes.
//!
//! Unlike the catalog, none of this is read from the local copy. A cart and
//! an order are written to the store and read back from it, so a page shows
//! a change at once; a cart's prices are worked out from the products in the
//! store too ([`timada_orders::Command::quote_cart`]), so the total a cart
//! page shows is exactly what [`place_order`] charges. The local copy only
//! adds what a page needs to link and illustrate a line (its slug and
//! picture).
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
use timada_orders::{CartItem, CartLine, Command, STOREFRONT_ACTOR};
pub use timada_orders::{
    Checkout, OrderLine, OrderState, OrderStatus, OrdersError, PlacedOrder, ShippingAddress,
};

use crate::context::store;
use crate::query::{Product, published_products_by_ids};
use crate::store::Store;

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

    /// Whether [`place_order`] can go ahead: something to sell, nothing off
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

    /// See [`place_order`].
    pub async fn place_order(
        &self,
        cart_id: &str,
        checkout: Checkout,
    ) -> anyhow::Result<Refusable<PlacedOrder>> {
        refusable(
            self.orders()
                .place_order(STOREFRONT_ACTOR, cart_id, checkout)
                .await,
        )
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

/// Places the cart as an order awaiting payment. Pass the total the shopper
/// was shown as [`Checkout::expected_total_cents`]: a price that moved since
/// is refused ([`OrdersError::PriceChanged`]) rather than charged unseen.
/// Placing a cart already ordered returns that order.
pub async fn place_order(
    cx: &Cx,
    cart_id: &str,
    checkout: Checkout,
) -> Result<Refusable<PlacedOrder>> {
    store(cx)
        .place_order(cart_id, checkout)
        .await
        .map_err(Error::from_anyhow)
}

/// An order, read from the store. Anyone holding its id can read it: the
/// host decides who may (the browser that placed it, typically).
pub async fn order(cx: &Cx, order_id: &str) -> Result<Option<OrderState>> {
    store(cx).order(order_id).await.map_err(Error::from_anyhow)
}
