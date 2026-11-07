//! That a product was ordered: what a delete is refused on.
//!
//! The catalog cannot see the orders, so the orders tell it: their
//! stock-usage subscription marks every product of a placed order. The mark
//! is the write side's own record, read when a product is deleted, instead
//! of the orders' read model, which lags behind the store and is empty on a
//! node that is copying its store back.

use evento::{Executor, Projection, WriteError, metadata::Event};

use crate::aggregator::ordered::{Ordered, ProductOrdered};
use crate::error::CatalogError;
use crate::value_object::product_ordered_id;

/// The first order that sold a product.
#[evento::projection(id = id, name = "timada-catalog/ProductOrderedState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct ProductOrderedState {
    pub id: String,
    pub order_id: String,
}

pub fn product_ordered_projection<E: Executor>() -> Projection<E, ProductOrderedState> {
    Projection::new::<ProductOrdered>()
        .handler(on_ordered())
        .strict()
}

#[evento::handler]
async fn on_ordered(
    event: Event<Ordered>,
    ordered: &mut ProductOrderedState,
) -> anyhow::Result<()> {
    ordered.id = event.aggregate_id.to_owned();
    ordered.order_id = event.data.order_id.to_owned();
    Ok(())
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Records that `order_id` sold `product_id`, the first time anybody
    /// orders it. Told again, or by a later order, nothing more is written.
    pub async fn mark_ordered(
        &self,
        actor: &str,
        product_id: &str,
        order_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        // Read first: every node marks the products of every order, and a
        // write refused on its version still costs a round of the cluster.
        if self.load_ordered(product_id).await?.is_some() {
            return Ok(());
        }
        let written = evento::append(product_ordered_id(product_id))
            .original_version(0)
            .routing_key_opt(routing_key)
            .event(&Ordered {
                order_id: order_id.to_owned(),
                ordered_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await;
        match written {
            // Marked by another order, or another node, meanwhile.
            Ok(_) | Err(WriteError::InvalidOriginalVersion) => Ok(()),
            Err(err) => Err(err.into()),
        }
    }
}

impl<E: Executor> super::Command<E> {
    /// The first order that sold `product_id`; `None` while nobody ordered
    /// it.
    pub async fn load_ordered(
        &self,
        product_id: &str,
    ) -> anyhow::Result<Option<ProductOrderedState>> {
        product_ordered_projection()
            .load(product_ordered_id(product_id))
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await
    }
}
