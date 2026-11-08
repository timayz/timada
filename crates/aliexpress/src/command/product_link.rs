//! Which AliExpress item filled a catalog product, by the product
//! ([`crate::aggregator::product_link`]): the index an order's lines are
//! resolved through when the shop buys them on AliExpress, and when a
//! payment decides whether to wait for that purchase. A decision reads the
//! store; the `aliexpress_products` read model, which holds the same link
//! a moment later, serves the pages.

use evento::{Executor, Projection, ProjectionAggregate, WriteError, metadata::Event};

use crate::aggregator::product_link::{Linked, ProductLink};
use crate::command::LinkState;
use crate::error::AliexpressError;
use crate::value_object::product_link_id;

/// How many times the index is written again when another write to it
/// landed at the same moment.
const WRITE_ATTEMPTS: usize = 3;

/// The item that filled a catalog product, as last recorded.
#[evento::projection(id = id, name = "timada-aliexpress/ProductLinkState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct ProductLinkState {
    pub id: String,
    pub product_id: String,
    /// The stream of the item ([`LinkState::id`]).
    pub link_id: String,
}

pub fn product_link_projection<E: Executor>() -> Projection<E, ProductLinkState> {
    Projection::new::<ProductLink>()
        .handler(on_linked())
        .strict()
}

#[evento::handler]
async fn on_linked(event: Event<Linked>, index: &mut ProductLinkState) -> anyhow::Result<()> {
    index.id = event.aggregate_id.to_owned();
    index.product_id = event.data.product_id.to_owned();
    index.link_id = event.data.link_id.to_owned();
    Ok(())
}

/// Records in the product's stream that the item `link_id` filled the
/// catalog product `product_id`. Recorded already, nothing more is written;
/// another item recorded before is written over, as the latest import of a
/// product is the one that fills it.
pub async fn claim_product_link<E: Executor>(
    executor: &E,
    tenant_id: &str,
    product_id: &str,
    link_id: &str,
    actor: &str,
    routing_key: Option<String>,
) -> Result<(), AliexpressError> {
    let linked = Linked {
        product_id: product_id.to_owned(),
        link_id: link_id.to_owned(),
        linked_by: actor.to_owned(),
    };
    let id = product_link_id(product_id);
    for attempt in 1..=WRITE_ATTEMPTS {
        let index = product_link_projection()
            .load(&id)
            .tenant(tenant_id)
            .execute(executor)
            .await?;
        let written = match index {
            Some(index) if index.link_id == link_id => return Ok(()),
            Some(index) => {
                index
                    .write()?
                    .routing_key_opt(routing_key.clone())
                    .event(&linked)
                    .tenant(tenant_id)
                    .commit(executor)
                    .await
            }
            None => {
                evento::append(&id)
                    .original_version(0)
                    .routing_key_opt(routing_key.clone())
                    .event(&linked)
                    .tenant(tenant_id)
                    .commit(executor)
                    .await
            }
        };
        match written {
            Ok(_) => return Ok(()),
            Err(WriteError::InvalidOriginalVersion) if attempt < WRITE_ATTEMPTS => {}
            Err(err) => return Err(err.into()),
        }
    }
    Err(WriteError::InvalidOriginalVersion.into())
}

impl<E: Executor> super::Command<E> {
    /// The linked item that filled the catalog product `product_id`, read
    /// from the store: `None` for a product no import filled, or whose item
    /// has since been unlinked or imported anew into another product.
    pub async fn load_link_of_product(
        &self,
        product_id: &str,
    ) -> anyhow::Result<Option<LinkState>> {
        let Some(index) = product_link_projection()
            .load(product_link_id(product_id))
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await?
        else {
            return Ok(None);
        };
        Ok(self
            .load_link_by_id(&index.link_id)
            .await?
            .filter(|link| link.is_linked() && link.product_id.as_deref() == Some(product_id)))
    }
}
