//! SQL list read model behind "mes paniers sauvegardés": one row per cart
//! that is saved *and* has an owner. Fed by the `cart-saved-list`
//! subscription; the cart itself is served by [`crate::CartDetailsView`].

use evento::{
    Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::SqlitePool;

use crate::{
    aggregator::{
        CartAssignedToCustomer, CartCheckedOut, CartDiscarded, CartLineAdded,
        CartLineQuantityChanged, CartLineRemoved, CartLineRepriced, CartOpened, CartReopened,
        CartSaved, PromoCodeApplied, PromoCodeRemoved,
    },
    query::load_cart_details,
    value_object::CartStatus,
};

/// Subscription key; the caller attaches the pool with `.data(pool)`.
pub const SAVED_CART_LIST_SUBSCRIPTION: &str = "cart-saved-list";

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct SavedCartRow {
    pub cart_id: String,
    pub customer_id: String,
    pub name: String,
    /// Units in the cart, all lines together.
    pub units: i64,
    pub subtotal_minor: i64,
    pub currency: String,
    pub saved_at: i64,
}

pub fn saved_cart_list_subscription<E: Executor>() -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(SAVED_CART_LIST_SUBSCRIPTION)
        .handler(refresh_on_cart_saved())
        .handler(refresh_on_cart_assigned_to_customer())
        .handler(refresh_on_cart_reopened())
        .handler(refresh_on_cart_discarded())
        .handler(refresh_on_cart_checked_out())
        .handler(refresh_on_cart_line_added())
        .handler(refresh_on_cart_line_quantity_changed())
        .handler(refresh_on_cart_line_removed())
        .handler(refresh_on_cart_line_repriced())
        .skip::<CartOpened>()
        .skip::<PromoCodeApplied>()
        .skip::<PromoCodeRemoved>()
        .strict()
}

/// A customer's saved carts, the most recently saved first.
pub async fn saved_carts_of_customer(
    db: &SqlitePool,
    customer_id: &str,
) -> sqlx::Result<Vec<SavedCartRow>> {
    sqlx::query_as(
        "SELECT cart_id, customer_id, name, units, subtotal_minor, currency, saved_at
         FROM cart_saved_list
         WHERE customer_id = ?
         ORDER BY saved_at DESC, cart_id DESC",
    )
    .bind(customer_id)
    .fetch_all(db)
    .await
}

/// Writes the cart as it stands now: listed while it is saved and owned,
/// gone otherwise. Absolute, so a redelivery changes nothing.
async fn refresh<E: Executor>(ctx: &Context<'_, E>, cart_id: &str) -> anyhow::Result<()> {
    let db = ctx
        .get::<SqlitePool>()
        .ok_or_else(|| anyhow::anyhow!("SqlitePool missing from subscription context"))?;
    let Some(cart) = load_cart_details(ctx.executor, cart_id).await? else {
        anyhow::bail!("cart {cart_id} cannot be loaded");
    };
    let listed = match (&cart.status, &cart.customer_id, &cart.saved_name) {
        (CartStatus::Saved, Some(customer_id), Some(name)) => Some((customer_id, name)),
        _ => None,
    };
    let Some((customer_id, name)) = listed else {
        sqlx::query("DELETE FROM cart_saved_list WHERE cart_id = ?")
            .bind(cart_id)
            .execute(&db)
            .await?;
        return Ok(());
    };
    let units: u32 = cart.lines.iter().map(|l| l.quantity).sum();
    sqlx::query(
        "INSERT INTO cart_saved_list
            (cart_id, customer_id, name, units, subtotal_minor, currency, saved_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (cart_id) DO UPDATE SET
            customer_id = excluded.customer_id,
            name = excluded.name,
            units = excluded.units,
            subtotal_minor = excluded.subtotal_minor,
            saved_at = excluded.saved_at",
    )
    .bind(&cart.id)
    .bind(customer_id)
    .bind(name)
    .bind(units)
    .bind(cart.subtotal.minor)
    .bind(&cart.subtotal.currency)
    .bind(cart.saved_at.unwrap_or_default() as i64)
    .execute(&db)
    .await?;
    Ok(())
}

#[evento::subscription]
async fn refresh_on_cart_saved<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<CartSaved>,
) -> anyhow::Result<()> {
    refresh(ctx, &event.aggregate_id).await
}

#[evento::subscription]
async fn refresh_on_cart_assigned_to_customer<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<CartAssignedToCustomer>,
) -> anyhow::Result<()> {
    refresh(ctx, &event.aggregate_id).await
}

#[evento::subscription]
async fn refresh_on_cart_reopened<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<CartReopened>,
) -> anyhow::Result<()> {
    refresh(ctx, &event.aggregate_id).await
}

#[evento::subscription]
async fn refresh_on_cart_discarded<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<CartDiscarded>,
) -> anyhow::Result<()> {
    refresh(ctx, &event.aggregate_id).await
}

#[evento::subscription]
async fn refresh_on_cart_checked_out<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<CartCheckedOut>,
) -> anyhow::Result<()> {
    refresh(ctx, &event.aggregate_id).await
}

#[evento::subscription]
async fn refresh_on_cart_line_added<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<CartLineAdded>,
) -> anyhow::Result<()> {
    refresh(ctx, &event.aggregate_id).await
}

#[evento::subscription]
async fn refresh_on_cart_line_quantity_changed<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<CartLineQuantityChanged>,
) -> anyhow::Result<()> {
    refresh(ctx, &event.aggregate_id).await
}

#[evento::subscription]
async fn refresh_on_cart_line_removed<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<CartLineRemoved>,
) -> anyhow::Result<()> {
    refresh(ctx, &event.aggregate_id).await
}

#[evento::subscription]
async fn refresh_on_cart_line_repriced<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<CartLineRepriced>,
) -> anyhow::Result<()> {
    refresh(ctx, &event.aggregate_id).await
}
