//! The write-side view of a product, replayed from its events.
//!
//! Unlike the SQL read models in [`crate::projections`], this is loaded
//! straight from the event store, so a caller always sees every event
//! committed so far. Commands use it to check invariants before appending, and
//! the cart uses it to snapshot title/price into a cart line right after an
//! import — a read-your-own-write the eventually-consistent tables cannot give.

use evento::ProjectionAggregate;
use evento::metadata::Event;
use evento::projection::Projection;
use timada_core::{Currency, Executor};

use crate::aggregate::{Product, ProductArchived, ProductImported, ProductPublished};

/// Everything known about one product, rebuilt from its event stream.
///
/// `published` and `archived` are tracked separately rather than as one status
/// enum because archiving is terminal: an archived product stays archived even
/// though it is also no longer published.
#[evento::projection(bitcode::Encode, bitcode::Decode)]
#[derive(Debug, PartialEq, Eq)]
pub struct ProductView {
    pub id: String,
    pub supplier_id: String,
    pub supplier_product_ref: String,
    pub title: String,
    pub description: String,
    pub price_cents: i64,
    pub currency: Currency,
    pub image_url: String,
    pub published: bool,
    pub archived: bool,
}

impl ProjectionAggregate for ProductView {
    fn aggregate_id(&self) -> String {
        self.id.to_owned()
    }
}

#[evento::handler]
async fn apply_imported(
    event: Event<ProductImported>,
    product: &mut ProductView,
) -> anyhow::Result<()> {
    product.id = event.aggregate_id.clone();
    product.supplier_id = event.data.supplier_id.clone();
    product.supplier_product_ref = event.data.supplier_product_ref.clone();
    product.title = event.data.title.clone();
    product.description = event.data.description.clone();
    product.price_cents = event.data.price.amount_cents;
    product.currency = event.data.price.currency;
    product.image_url = event.data.image_url.clone();
    Ok(())
}

#[evento::handler]
async fn apply_published(
    _event: Event<ProductPublished>,
    product: &mut ProductView,
) -> anyhow::Result<()> {
    product.published = true;
    Ok(())
}

#[evento::handler]
async fn apply_archived(
    _event: Event<ProductArchived>,
    product: &mut ProductView,
) -> anyhow::Result<()> {
    product.published = false;
    product.archived = true;
    Ok(())
}

/// Replay one product. `None` means no such aggregate.
pub async fn load_product(
    executor: &Executor,
    product_id: &str,
) -> anyhow::Result<Option<ProductView>> {
    projection().load(product_id).execute(executor).await
}

fn projection() -> Projection<Executor, ProductView> {
    Projection::<Executor, ProductView>::new::<Product>()
        .handler(apply_imported())
        .handler(apply_published())
        .handler(apply_archived())
        .strict()
}
