//! Write-side commands for [`Product`](crate::aggregate::Product).
//!
//! All three are idempotent: the admin import flow can be double-submitted and
//! the publish/archive buttons double-clicked without producing a second event.

use evento::{AggregateExt as _, ProjectionAggregate as _};
use timada_core::{Executor, Money};
use timada_dropship::SupplierProduct;

use crate::aggregate::{ProductArchived, ProductImported, ProductPriceSet, ProductPublished};
use crate::view::load_product;

/// The catalog id of a supplier's product.
///
/// Derived rather than generated so importing the same supplier product twice
/// lands on the same aggregate instead of creating a duplicate listing.
pub fn product_id(supplier_id: &str, supplier_product_ref: &str) -> String {
    evento::hash_ids(vec![supplier_id, supplier_product_ref])
}

/// Import a supplier's product into the catalog as a draft.
///
/// Returns the catalog product id. Re-importing an already-imported product is
/// a no-op — notably it does *not* refresh title or price, because those are
/// snapshots the storefront and past orders rely on staying put.
#[tracing::instrument(skip(executor, product), fields(supplier_product_ref = %product.supplier_product_ref))]
pub async fn import_product(
    executor: &Executor,
    supplier_id: &str,
    product: SupplierProduct,
) -> anyhow::Result<String> {
    let id = product_id(supplier_id, &product.supplier_product_ref);

    if executor.has_event::<ProductImported>(&id).await? {
        tracing::info!(product_id = %id, "product already imported, skipping");
        return Ok(id);
    }

    evento::append(&id)
        .original_version(0)
        .event(&ProductImported {
            supplier_id: supplier_id.to_owned(),
            supplier_product_ref: product.supplier_product_ref,
            title: product.title,
            description: product.description,
            price: product.price,
            image_url: product.image_url,
        })
        .commit(executor)
        .await?;

    tracing::info!(product_id = %id, %supplier_id, "product imported");
    Ok(id)
}

/// Make a product visible in the storefront.
///
/// Archiving is terminal, so an archived product cannot be republished; that
/// and a repeated publish are logged and ignored rather than raised, because
/// both mean the admin's intent is already satisfied (or no longer possible)
/// and neither should surface as a 500 on a button press.
#[tracing::instrument(skip(executor))]
pub async fn publish_product(executor: &Executor, product_id: &str) -> anyhow::Result<()> {
    let Some(product) = load_product(executor, product_id).await? else {
        tracing::warn!(product_id, "cannot publish an unknown product");
        return Ok(());
    };

    if product.archived {
        tracing::warn!(product_id, "cannot publish an archived product");
        return Ok(());
    }
    if product.published {
        tracing::info!(product_id, "product already published");
        return Ok(());
    }

    product
        .write()?
        .event(&ProductPublished)
        .commit(executor)
        .await?;

    tracing::info!(product_id, "product published");
    Ok(())
}

/// Withdraw a product from the storefront.
///
/// A draft can be archived directly — archiving means "never sell this again",
/// which is meaningful whether or not it was ever published.
#[tracing::instrument(skip(executor))]
pub async fn archive_product(executor: &Executor, product_id: &str) -> anyhow::Result<()> {
    let Some(product) = load_product(executor, product_id).await? else {
        tracing::warn!(product_id, "cannot archive an unknown product");
        return Ok(());
    };

    if product.archived {
        tracing::info!(product_id, "product already archived");
        return Ok(());
    }

    product
        .write()?
        .event(&ProductArchived)
        .commit(executor)
        .await?;

    tracing::info!(product_id, "product archived");
    Ok(())
}

/// Set (or replace) this product's price in `price`'s currency.
///
/// The import price stays the base; explicit prices override it per currency,
/// latest event winning. An archived product keeps its history but refuses new
/// prices — there is nothing left to sell.
#[tracing::instrument(skip(executor))]
pub async fn set_product_price(
    executor: &Executor,
    product_id: &str,
    price: Money,
) -> anyhow::Result<()> {
    let Some(product) = load_product(executor, product_id).await? else {
        anyhow::bail!("cannot price an unknown product: {product_id}");
    };
    if product.archived {
        anyhow::bail!("cannot price an archived product: {product_id}");
    }

    product
        .write()?
        .event(&ProductPriceSet { price })
        .commit(executor)
        .await?;

    tracing::info!(product_id, %price, "product price set");
    Ok(())
}
