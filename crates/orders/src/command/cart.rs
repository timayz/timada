use evento::{Executor, ProjectionAggregate};
use timada_catalog::{ProductState, Sellable, product_projection};
use timada_discounts::{
    DISCOUNT_UNKNOWN, DiscountState, discount_code_id, discount_code_projection,
    discount_projection, normalize_code,
};
use ulid::Ulid;

use crate::aggregator::{
    Created, DiscountApplied, DiscountRemoved, Expired, ItemAdded, ItemRemoved, QuantitySet,
};
use crate::error::OrderError;
use crate::value_object::{MAX_CART_LINES, MAX_LINE_QUANTITY};

use super::checkout::{QuoteSource, StoreCatalog, require_in_stock, require_sellable};
use super::{CartState, cart_projection};

/// A product, or one variant of it, and how many.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CartItem {
    pub product_id: String,
    /// Required when the product sells variants, absent otherwise.
    pub variant_id: Option<String>,
    pub quantity: u32,
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Starts a cart holding `item` and returns its id. The item is checked
    /// as [`add_to_cart`](Self::add_to_cart) checks one, before anything is
    /// written, and the cart is written whole in one go: an item refused
    /// leaves no cart behind.
    pub async fn start_cart(
        &self,
        actor: &str,
        item: CartItem,
        routing_key: Option<String>,
    ) -> Result<String, OrderError> {
        if !(1..=MAX_LINE_QUANTITY).contains(&item.quantity) {
            return Err(OrderError::InvalidQuantity);
        }
        let cart_id = Ulid::generate().to_string();
        self.require_sellable(&item).await?;
        self.require_in_stock(
            &cart_id,
            &item.product_id,
            item.variant_id.as_deref(),
            item.quantity,
        )
        .await?;
        evento::append(&cart_id)
            .original_version(0)
            .routing_key_opt(routing_key)
            .event(&Created {
                created_by: actor.to_owned(),
            })
            .event(&ItemAdded {
                product_id: item.product_id,
                variant_id: item.variant_id,
                quantity: item.quantity,
                added_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(cart_id)
    }

    /// Drops the cart if nobody touched it for `idle_secs` by `now` and
    /// nothing about it could still move money ([`CartState::is_idle`]):
    /// `Ok(true)` when it was dropped, `Ok(false)` for a cart that is not
    /// there, was dropped already or is not idle. Written at the version
    /// read, so a change landing first (the shopper back, another node
    /// dropping it) keeps it, for another look to judge.
    pub async fn expire_cart(
        &self,
        actor: &str,
        cart_id: &str,
        now: u64,
        idle_secs: u64,
        routing_key: Option<String>,
    ) -> Result<bool, OrderError> {
        let Some(cart) = cart_projection()
            .load(cart_id)
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await?
        else {
            return Ok(false);
        };
        if !cart.is_idle(now, idle_secs) {
            return Ok(false);
        }
        let written = cart
            .write()?
            .routing_key_opt(routing_key)
            .event(&Expired {
                expired_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await;
        match written {
            Ok(_) => Ok(true),
            Err(evento::WriteError::InvalidOriginalVersion) => Ok(false),
            Err(err) => Err(err.into()),
        }
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

    /// The shop's delivery time for a product that gives none.
    pub(crate) async fn load_default_delivery_days(
        &self,
    ) -> anyhow::Result<Option<timada_catalog::DeliveryDays>> {
        Ok(timada_catalog::delivery_default_projection()
            .load(timada_catalog::delivery_default_id())
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await?
            .and_then(|state| state.days))
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
    /// untracked thing has no limit. Read from the stock's own stream.
    pub(crate) async fn require_in_stock(
        &self,
        cart_id: &str,
        product_id: &str,
        variant_id: Option<&str>,
        quantity: u32,
    ) -> Result<(), OrderError> {
        let available = StoreCatalog(self)
            .available(cart_id, product_id, variant_id)
            .await?;
        require_in_stock(product_id, available, quantity)
    }

    /// The product of an item, when a shopper can buy it as asked
    /// ([`require_sellable`]), read from its own stream.
    pub(crate) async fn require_sellable(
        &self,
        item: &CartItem,
    ) -> Result<ProductState, OrderError> {
        let product = self.load_product(&item.product_id).await?;
        require_sellable(product.as_ref().map(Sellable::of).as_ref(), item)?;
        Ok(product.expect("a sellable product is some"))
    }
}
