use evento::{Executor, ProjectionAggregate, WriteError};

use crate::aggregator::{CheckedOut, Claimed, Placed};
use crate::error::OrdersError;
use crate::value_object::{
    OrderLine, ShippingAddress, new_reference, normalize_email, normalize_note, normalize_shipping,
    order_id, order_reference_id,
};

use super::{CartItem, CartLine, CartState, OrderState};

/// How many fresh references are tried before giving up: with 40 bits each,
/// a second clash in a row means something else is wrong.
const REFERENCE_ATTEMPTS: usize = 5;
/// How many times marking the cart is tried again when the shopper changed
/// it at the same moment.
const CHECK_OUT_ATTEMPTS: usize = 3;

/// What a shopper gives at checkout.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Checkout {
    pub email: String,
    pub shipping: ShippingAddress,
    pub note: Option<String>,
    /// The total the shopper was shown; when given, an order is only placed
    /// at that total, so a price that moved in between is never charged
    /// unseen.
    pub expected_total_cents: Option<u64>,
}

/// The order a checkout made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacedOrder {
    pub order_id: String,
    pub reference: String,
}

impl From<&OrderState> for PlacedOrder {
    fn from(order: &OrderState) -> Self {
        Self {
            order_id: order.id.clone(),
            reference: order.reference.clone(),
        }
    }
}

/// A cart priced from the catalog as it stands: the lines an order would
/// hold, and the ones that cannot be sold any more.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Quote {
    pub lines: Vec<OrderLine>,
    /// The lines whose product is off sale, gone, or whose variant is.
    pub unavailable: Vec<CartLine>,
    /// `None` while no line is priced.
    pub currency: Option<String>,
    pub total_cents: u64,
}

impl Quote {
    pub fn item_count(&self) -> u32 {
        self.lines.iter().map(|line| line.quantity).sum()
    }
}

impl<E: Executor> super::Command<E> {
    /// Prices a cart from the catalog as it stands now, reading every product
    /// from the store (never from a replica), so what it says is what
    /// [`place_order`](Self::place_order) charges.
    pub async fn quote_cart(&self, cart_id: &str) -> Result<Quote, OrdersError> {
        let cart = self.require_cart(cart_id).await?;
        self.quote(&cart).await
    }

    pub(crate) async fn quote(&self, cart: &CartState) -> Result<Quote, OrdersError> {
        let mut quote = Quote::default();
        for line in &cart.lines {
            let item = CartItem {
                product_id: line.product_id.clone(),
                variant_id: line.variant_id.clone(),
                quantity: line.quantity,
            };
            let product = match self.require_sellable(&item).await {
                Ok(product) => product,
                Err(
                    OrdersError::Unavailable { .. }
                    | OrdersError::VariantRequired
                    | OrdersError::VariantNotFound,
                ) => {
                    quote.unavailable.push(line.clone());
                    continue;
                }
                Err(err) => return Err(err),
            };
            let variant = line
                .variant_id
                .as_deref()
                .and_then(|id| product.variant(id));
            let price = match variant {
                Some(variant) => &variant.price,
                // `require_sellable` let a line without a variant through
                // only for a product priced on its own.
                None => product.price.as_ref().ok_or(OrdersError::Unavailable {
                    product_id: product.id.clone(),
                })?,
            };
            match &quote.currency {
                Some(currency) if *currency != price.currency => {
                    return Err(OrdersError::MixedCurrencies);
                }
                Some(_) => {}
                None => quote.currency = Some(price.currency.clone()),
            }
            let label = variant.map(|variant| {
                variant
                    .selections
                    .iter()
                    .map(|selection| selection.value.as_str())
                    .collect::<Vec<_>>()
                    .join(" / ")
            });
            let order_line = OrderLine {
                product_id: product.id.clone(),
                variant_id: line.variant_id.clone(),
                name: product.name.clone(),
                variant_label: label,
                sku: variant.and_then(|variant| variant.sku.clone()),
                unit_amount_cents: price.amount_cents,
                quantity: line.quantity,
            };
            quote.total_cents += order_line.amount_cents();
            quote.lines.push(order_line);
        }
        Ok(quote)
    }
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Turns the cart into an order awaiting payment, priced from the
    /// catalog as it stands. Placing a cart that already became an order
    /// returns that order, so a repeated submit is harmless.
    pub async fn place_order(
        &self,
        actor: &str,
        cart_id: &str,
        checkout: Checkout,
        routing_key: Option<String>,
    ) -> Result<PlacedOrder, OrdersError> {
        let email = normalize_email(&checkout.email)?;
        let shipping = normalize_shipping(checkout.shipping)?;
        let note = normalize_note(checkout.note.as_deref())?;

        let cart = self.require_cart(cart_id).await?;
        let order_id = order_id(cart_id);
        if let Some(order) = self.load_order(&order_id).await? {
            self.check_out(cart, &order, actor, routing_key).await?;
            return Ok((&order).into());
        }
        if cart.lines.is_empty() {
            return Err(OrdersError::EmptyCart);
        }
        let quote = self.quote(&cart).await?;
        if let Some(line) = quote.unavailable.first() {
            return Err(OrdersError::Unavailable {
                product_id: line.product_id.clone(),
            });
        }
        if let Some(expected) = checkout.expected_total_cents
            && expected != quote.total_cents
        {
            return Err(OrdersError::PriceChanged {
                total_cents: quote.total_cents,
            });
        }
        let currency = quote.currency.ok_or(OrdersError::EmptyCart)?;

        let reference = self.claim_reference(&order_id, routing_key.clone()).await?;
        let placed = evento::append(&order_id)
            .original_version(0)
            .routing_key_opt(routing_key.clone())
            .event(&Placed {
                reference,
                cart_id: cart_id.to_owned(),
                email,
                shipping,
                note,
                lines: quote.lines,
                currency,
                total_cents: quote.total_cents,
                placed_by: actor.to_owned(),
            })
            .commit(&self.0)
            .await;
        match placed {
            // Placed at the same moment by another submit: that one stands
            // (the reference claimed here stays unused).
            Ok(_) | Err(WriteError::InvalidOriginalVersion) => {}
            Err(err) => return Err(err.into()),
        }
        let order = self.require_order(&order_id).await?;
        self.check_out(cart, &order, actor, routing_key).await?;
        Ok((&order).into())
    }
}

impl<E: Executor> super::Command<E> {
    /// Claims a fresh reference for the order, trying another one on a clash.
    async fn claim_reference(
        &self,
        order_id: &str,
        routing_key: Option<String>,
    ) -> Result<String, OrdersError> {
        for _ in 0..REFERENCE_ATTEMPTS {
            let reference = new_reference();
            let claimed = evento::append(order_reference_id(&reference))
                .original_version(0)
                .routing_key_opt(routing_key.clone())
                .event(&Claimed {
                    order_id: order_id.to_owned(),
                })
                .commit(&self.0)
                .await;
            match claimed {
                Ok(_) => return Ok(reference),
                Err(WriteError::InvalidOriginalVersion) => continue,
                Err(err) => return Err(err.into()),
            }
        }
        Err(anyhow::anyhow!("no free order reference after {REFERENCE_ATTEMPTS} attempts").into())
    }

    /// Marks the cart as the order, unless it already is. A shopper changing
    /// the cart at the same moment only moves the version: the mark is tried
    /// again on the fresh cart.
    async fn check_out(
        &self,
        mut cart: CartState,
        order: &OrderState,
        actor: &str,
        routing_key: Option<String>,
    ) -> Result<(), OrdersError> {
        for attempt in 1..=CHECK_OUT_ATTEMPTS {
            if cart.is_checked_out() {
                return Ok(());
            }
            let marked = cart
                .write()?
                .routing_key_opt(routing_key.clone())
                .event(&CheckedOut {
                    order_id: order.id.clone(),
                    checked_out_by: actor.to_owned(),
                })
                .commit(&self.0)
                .await;
            match marked {
                Ok(_) => return Ok(()),
                Err(WriteError::InvalidOriginalVersion) if attempt < CHECK_OUT_ATTEMPTS => {
                    cart = self.require_cart(&cart.id).await?;
                }
                Err(err) => return Err(err.into()),
            }
        }
        Ok(())
    }

    /// The order a reference names, if any.
    pub async fn load_order_by_reference(
        &self,
        reference: &str,
    ) -> anyhow::Result<Option<OrderState>> {
        let Some(index) = super::order_reference_projection()
            .load(order_reference_id(reference))
            .execute(&self.0)
            .await?
        else {
            return Ok(None);
        };
        self.load_order(&index.order_id).await
    }
}
