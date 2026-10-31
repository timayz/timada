use evento::{Executor, ProjectionAggregate};
use timada_catalog::{ProductState, product_projection, stock_item_id, stock_projection};
use timada_discounts::{
    DISCOUNT_UNKNOWN, DiscountState, discount_code_id, discount_code_projection,
    discount_projection, normalize_code,
};
use ulid::Ulid;

use crate::aggregator::{
    Created, DiscountApplied, DiscountRemoved, ItemAdded, ItemRemoved, QuantitySet,
};
use crate::error::OrderError;
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
    ) -> Result<String, OrderError> {
        let cart_id = Ulid::generate().to_string();
        evento::append(&cart_id)
            .original_version(0)
            .routing_key_opt(routing_key)
            .event(&Created {
                created_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
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
    ) -> Result<CartState, OrderError> {
        let cart = self.require_open_cart(cart_id).await?;
        if !(1..=MAX_LINE_QUANTITY).contains(&item.quantity) {
            return Err(OrderError::InvalidQuantity);
        }
        let existing = cart.line(&item.product_id, item.variant_id.as_deref());
        match existing {
            Some(line) if line.quantity + item.quantity > MAX_LINE_QUANTITY => {
                return Err(OrderError::InvalidQuantity);
            }
            None if cart.lines.len() >= MAX_CART_LINES => return Err(OrderError::TooManyLines),
            _ => {}
        }
        let product = self.require_sellable(&item).await?;
        let wanted = existing.map_or(0, |line| line.quantity) + item.quantity;
        self.require_in_stock(
            cart_id,
            &item.product_id,
            item.variant_id.as_deref(),
            wanted,
        )
        .await?;
        if existing.is_none()
            && let Some(first) = cart.lines.first()
        {
            let other = self.load_product(&first.product_id).await?;
            let other_currency = other.as_ref().and_then(|p| p.currency());
            if other_currency.is_some_and(|currency| Some(currency) != product.currency()) {
                return Err(OrderError::MixedCurrencies);
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
            .tenant(&self.tenant)
            .commit(&self.executor)
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
    ) -> Result<CartState, OrderError> {
        let cart = self.require_open_cart(cart_id).await?;
        if item.quantity > MAX_LINE_QUANTITY {
            return Err(OrderError::InvalidQuantity);
        }
        let Some(line) = cart.line(&item.product_id, item.variant_id.as_deref()) else {
            return Ok(cart);
        };
        if line.quantity == item.quantity {
            return Ok(cart);
        }
        // Fewer is always fine; more needs what is left.
        if item.quantity > line.quantity {
            self.require_in_stock(
                cart_id,
                &item.product_id,
                item.variant_id.as_deref(),
                item.quantity,
            )
            .await?;
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
        write.tenant(&self.tenant).commit(&self.executor).await?;
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
    ) -> Result<CartState, OrderError> {
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

    /// Puts a discount code on the cart, replacing the one it had. A code no
    /// live discount has is refused (`discount_unknown`), so a typo shows at
    /// once; whether it takes anything off is decided when the cart is
    /// priced, since that can change before the shopper pays.
    pub async fn apply_discount(
        &self,
        actor: &str,
        cart_id: &str,
        code: &str,
        routing_key: Option<String>,
    ) -> Result<CartState, OrderError> {
        let unknown = || OrderError::DiscountRefused {
            code: DISCOUNT_UNKNOWN,
        };
        let code = normalize_code(code).map_err(|_| unknown())?;
        let cart = self.require_open_cart(cart_id).await?;
        if self.load_discount_by_code(&code).await?.is_none() {
            return Err(unknown());
        }
        if cart.discount_code.as_deref() == Some(code.as_str()) {
            return Ok(cart);
        }
        cart.write()?
            .routing_key_opt(routing_key)
            .event(&DiscountApplied {
                code,
                applied_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        self.require_cart(cart_id).await
    }

    /// Takes the discount code off the cart; none is a no-op.
    pub async fn remove_discount(
        &self,
        actor: &str,
        cart_id: &str,
        routing_key: Option<String>,
    ) -> Result<CartState, OrderError> {
        let cart = self.require_open_cart(cart_id).await?;
        if cart.discount_code.is_none() {
            return Ok(cart);
        }
        cart.write()?
            .routing_key_opt(routing_key)
            .event(&DiscountRemoved {
                removed_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        self.require_cart(cart_id).await
    }
}

impl<E: Executor> super::Command<E> {
    /// The live discount a normalized code names, read from the tenant's
    /// store like the products.
    pub(crate) async fn load_discount_by_code(
        &self,
        code: &str,
    ) -> anyhow::Result<Option<DiscountState>> {
        let Some(index) = discount_code_projection()
            .load(discount_code_id(code))
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await?
            .filter(|index| !index.released)
        else {
            return Ok(None);
        };
        Ok(discount_projection()
            .load(&index.discount_id)
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await?
            .filter(|discount| !discount.is_archived()))
    }

    pub(crate) async fn load_product(
        &self,
        product_id: &str,
    ) -> anyhow::Result<Option<ProductState>> {
        product_projection()
            .load(product_id)
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await
    }

    /// The cart, refused once it became an order.
    pub(crate) async fn require_open_cart(&self, cart_id: &str) -> Result<CartState, OrderError> {
        let cart = self.require_cart(cart_id).await?;
        if cart.is_checked_out() {
            return Err(OrderError::CheckedOut);
        }
        Ok(cart)
    }

    /// `OutOfStock` when the cart wants more of a thing than is left of it:
    /// what other checkouts hold counts, the cart's own hold does not. An
    /// untracked thing has no limit.
    pub(crate) async fn require_in_stock(
        &self,
        cart_id: &str,
        product_id: &str,
        variant_id: Option<&str>,
        quantity: u32,
    ) -> Result<(), OrderError> {
        let Some(stock) = stock_projection()
            .load(stock_item_id(product_id, variant_id))
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await?
        else {
            return Ok(());
        };
        match stock.available_for(Some(cart_id), timada_discounts::unix_now()) {
            Some(available) if available < quantity => Err(OrderError::OutOfStock {
                product_id: product_id.to_owned(),
                available,
            }),
            _ => Ok(()),
        }
    }

    /// The product of an item, when a shopper can buy it as asked: published,
    /// priced, and the variant named exactly when it sells variants.
    pub(crate) async fn require_sellable(
        &self,
        item: &CartItem,
    ) -> Result<ProductState, OrderError> {
        let unavailable = || OrderError::Unavailable {
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
            (None, false) => return Err(OrderError::VariantRequired),
            (Some(_), true) => return Err(OrderError::VariantNotFound),
            (Some(variant_id), false) => {
                product
                    .variant(variant_id)
                    .ok_or(OrderError::VariantNotFound)?;
            }
        }
        Ok(product)
    }
}
