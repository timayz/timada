use evento::{Executor, ProjectionAggregate};
use timada_catalog::{ProductState, product_projection};
use ulid::Ulid;

use crate::aggregator::{Created, ItemAdded, ItemRemoved, QuantitySet};
use crate::error::OrdersError;
use crate::value_object::{MAX_CART_LINES, MAX_LINE_QUANTITY};

use super::CartState;

/// A product, or one variant of it, and how many.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CartItem {
    pub product_id: String,
    /// Required when the product sells variants, absent otherwise.
    pub variant_id: Option<String>,
    pub quantity: u32,
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Starts an empty cart and returns its id.
    pub async fn create_cart(
        &self,
        actor: &str,
        routing_key: Option<String>,
    ) -> Result<String, OrdersError> {
        let cart_id = Ulid::generate().to_string();
        evento::append(&cart_id)
            .original_version(0)
            .routing_key_opt(routing_key)
            .event(&Created {
                created_by: actor.to_owned(),
            })
            .commit(&self.0)
            .await?;
        Ok(cart_id)
    }

    /// Adds `quantity` of a product on sale to the cart, on the line of that
    /// product and variant if there is one. Everything in a cart is priced in
    /// one currency.
    pub async fn add_to_cart(
        &self,
        actor: &str,
        cart_id: &str,
        item: CartItem,
        routing_key: Option<String>,
    ) -> Result<CartState, OrdersError> {
        let cart = self.require_open_cart(cart_id).await?;
        if !(1..=MAX_LINE_QUANTITY).contains(&item.quantity) {
            return Err(OrdersError::InvalidQuantity);
        }
        let existing = cart.line(&item.product_id, item.variant_id.as_deref());
        match existing {
            Some(line) if line.quantity + item.quantity > MAX_LINE_QUANTITY => {
                return Err(OrdersError::InvalidQuantity);
            }
            None if cart.lines.len() >= MAX_CART_LINES => return Err(OrdersError::TooManyLines),
            _ => {}
        }
        let product = self.require_sellable(&item).await?;
        if existing.is_none()
            && let Some(first) = cart.lines.first()
        {
            let other = self.load_product(&first.product_id).await?;
            let other_currency = other.as_ref().and_then(|p| p.currency());
            if other_currency.is_some_and(|currency| Some(currency) != product.currency()) {
                return Err(OrdersError::MixedCurrencies);
            }
        }
        cart.write()?
            .routing_key_opt(routing_key)
            .event(&ItemAdded {
                product_id: item.product_id,
                variant_id: item.variant_id,
                quantity: item.quantity,
                added_by: actor.to_owned(),
            })
            .commit(&self.0)
            .await?;
        self.require_cart(cart_id).await
    }

    /// Sets how many of a line the cart holds; zero removes the line. A line
    /// that is not there is a no-op.
    pub async fn set_cart_quantity(
        &self,
        actor: &str,
        cart_id: &str,
        item: CartItem,
        routing_key: Option<String>,
    ) -> Result<CartState, OrdersError> {
        let cart = self.require_open_cart(cart_id).await?;
        if item.quantity > MAX_LINE_QUANTITY {
            return Err(OrdersError::InvalidQuantity);
        }
        let Some(line) = cart.line(&item.product_id, item.variant_id.as_deref()) else {
            return Ok(cart);
        };
        if line.quantity == item.quantity {
            return Ok(cart);
        }
        let mut write = cart.write()?;
        write.routing_key_opt(routing_key);
        if item.quantity == 0 {
            write.event(&ItemRemoved {
                product_id: item.product_id,
                variant_id: item.variant_id,
                removed_by: actor.to_owned(),
            });
        } else {
            write.event(&QuantitySet {
                product_id: item.product_id,
                variant_id: item.variant_id,
                quantity: item.quantity,
                set_by: actor.to_owned(),
            });
        }
        write.commit(&self.0).await?;
        self.require_cart(cart_id).await
    }

    /// Takes a line out of the cart; one that is not there is a no-op.
    pub async fn remove_from_cart(
        &self,
        actor: &str,
        cart_id: &str,
        product_id: &str,
        variant_id: Option<&str>,
        routing_key: Option<String>,
    ) -> Result<CartState, OrdersError> {
        self.set_cart_quantity_opt(
            actor,
            cart_id,
            CartItem {
                product_id: product_id.to_owned(),
                variant_id: variant_id.map(str::to_owned),
                quantity: 0,
            },
            routing_key,
        )
        .await
    }
}

impl<E: Executor> super::Command<E> {
    pub(crate) async fn load_product(
        &self,
        product_id: &str,
    ) -> anyhow::Result<Option<ProductState>> {
        product_projection().load(product_id).execute(&self.0).await
    }

    /// The cart, refused once it became an order.
    pub(crate) async fn require_open_cart(&self, cart_id: &str) -> Result<CartState, OrdersError> {
        let cart = self.require_cart(cart_id).await?;
        if cart.is_checked_out() {
            return Err(OrdersError::CheckedOut);
        }
        Ok(cart)
    }

    /// The product of an item, when a shopper can buy it as asked: published,
    /// priced, and the variant named exactly when it sells variants.
    pub(crate) async fn require_sellable(
        &self,
        item: &CartItem,
    ) -> Result<ProductState, OrdersError> {
        let unavailable = || OrdersError::Unavailable {
            product_id: item.product_id.clone(),
        };
        let product = self
            .load_product(&item.product_id)
            .await?
            .ok_or_else(unavailable)?;
        if !product.is_published() || !product.is_priced() {
            return Err(unavailable());
        }
        match (&item.variant_id, product.variants.is_empty()) {
            (None, true) => {}
            (None, false) => return Err(OrdersError::VariantRequired),
            (Some(_), true) => return Err(OrdersError::VariantNotFound),
            (Some(variant_id), false) => {
                product
                    .variant(variant_id)
                    .ok_or(OrdersError::VariantNotFound)?;
            }
        }
        Ok(product)
    }
}
