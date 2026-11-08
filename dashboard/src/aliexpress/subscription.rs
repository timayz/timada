//! Imports the AliExpress items a shop asked for: fetches each from
//! AliExpress with the company's account, fills a draft from it (pictures,
//! options, variants, their stock) and records which combination each
//! variant is. Every node runs it; the first to take
//! an import on does it, the others leave it.
//!
//! The items already linked are read again the same way, on a cursor of their
//! own so a long import does not hold them up ([`reads_subscription`]): a
//! request names no node, and the one the item belongs to reads it. The same
//! node asks when a shopper puts an item in a cart, so what the checkout shows
//! is already what AliExpress asks.

use std::path::Path;
use std::sync::Arc;

use evento::{
    Evento, Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use timada_aliexpress::aggregator::{ImportRequested, SyncRequested};
use timada_aliexpress::{LinkStatus, link_of_product, map_product, note_tried, track_imported};
use timada_catalog_import::{ImportOptions, import_one};
use timada_orders::aggregator::ItemAdded;

use super::{ALIEXPRESS_ACTOR, Aliexpress, SyncMode};
use crate::db::Db;
use crate::media::Media;

/// The cursor identity of the subscription, in the store.
pub const SUBSCRIPTION: &str = "timada-dashboard/aliexpress-import";

/// The cursor identity of [`reads_subscription`], in the store.
pub const READS_SUBSCRIPTION: &str = "timada-dashboard/aliexpress-reads";

/// A request older than this is history being replayed (a node catching
/// up): left alone, the merchant may ask again and the sweep comes back.
const REQUEST_WINDOW_SECS: u64 = 10 * 60;

/// A cart add asks for no read when the item was tried this recently. Each
/// request takes a version of the item's stream, whose versions run out at
/// 65,535: at most one an hour leaves a product selling around the clock years
/// of them, and the payment reads it anyway.
const CART_ADD_FRESH_SECS: u64 = 60 * 60;

/// The store, to write with.
#[derive(Clone)]
struct Store(Evento);

pub fn subscription<E: Executor>(
    aliexpress: Arc<Aliexpress>,
    media: Arc<Media>,
    store: Evento,
) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(SUBSCRIPTION)
        .data(aliexpress)
        .data(media)
        .data(Store(store))
        .handler(on_import_requested())
        // A saga: it follows the node's own region (the executor's default
        // routing key), so a region's nodes do its work and the other
        // regions' nodes leave it to them.
        .retry(5)
}

#[evento::subscription]
async fn on_import_requested<E: Executor>(
    context: &Context<'_, E>,
    event: Event<ImportRequested>,
) -> anyhow::Result<()> {
    let aliexpress: Arc<Aliexpress> = context.extract();
    let media: Arc<Media> = context.extract();
    let store: Store = context.extract();
    let now = crate::auth::now();
    if now.saturating_sub(event.timestamp) > REQUEST_WINDOW_SECS {
        return Ok(());
    }
    let links = timada_aliexpress::Command::new(store.0.clone(), &event.tenant);
    let link_id = event.aggregate_id.clone();
    let Some(link) = links.load_link_by_id(&link_id).await? else {
        return Ok(());
    };
    if link.status != LinkStatus::Importing || link.started {
        return Ok(());
    }
    if !links.start_import(ALIEXPRESS_ACTOR, &link_id).await? {
        return Ok(());
    }
    let catalog = timada_catalog::Command::new(store.0.clone(), &event.tenant);
    let outcome = import(
        &aliexpress,
        media,
        &event.tenant,
        &catalog,
        &link.requested_by,
        &link.ae_product_id,
    )
    .await;
    match outcome {
        Ok((product_id, skus)) => {
            links
                .record_import(ALIEXPRESS_ACTOR, &link_id, &product_id, skus)
                .await?;
            tracing::info!(tenant = %event.tenant, ae_product_id = %link.ae_product_id, %product_id, "AliExpress product imported");
        }
        Err(reason) => {
            tracing::warn!(tenant = %event.tenant, ae_product_id = %link.ae_product_id, %reason, "AliExpress product not imported");
            links
                .record_import_failure(ALIEXPRESS_ACTOR, &link_id, &reason)
                .await?;
        }
    }
    Ok(())
}

/// Reads the linked items asked for. `db` is the read-model database, which
/// must be allowed to write (it keeps when each item was read).
pub fn reads_subscription<E: Executor>(
    aliexpress: Arc<Aliexpress>,
    db: Db,
    store: Evento,
) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(READS_SUBSCRIPTION)
        .data(aliexpress)
        .data(db)
        .data(Store(store))
        .handler(on_sync_requested())
        .handler(on_item_added())
        // A saga: it follows the node's own region (the executor's default
        // routing key), so a region's nodes do its work and the other
        // regions' nodes leave it to them.
        .retry(5)
}

#[evento::subscription]
async fn on_sync_requested<E: Executor>(
    context: &Context<'_, E>,
    event: Event<SyncRequested>,
) -> anyhow::Result<()> {
    let aliexpress: Arc<Aliexpress> = context.extract();
    let now = crate::auth::now();
    if now.saturating_sub(event.timestamp) > REQUEST_WINDOW_SECS
        || !aliexpress.share().owns(&event.aggregate_id)
    {
        return Ok(());
    }
    let db: Db = context.extract();
    let store: Store = context.extract();
    let mode = if event.data.requested_by == ALIEXPRESS_ACTOR {
        SyncMode::Routine
    } else {
        SyncMode::Asked {
            by: &event.data.requested_by,
        }
    };
    // A store that fails here fails this read alone, or it would stop the
    // requests behind it: the merchant asks again, the sweep comes back.
    let read = async {
        let Some((tenant, company)) = aliexpress.connected_shop(&event.tenant, now).await? else {
            return Ok(());
        };
        aliexpress
            .sync_link(&db, &store.0, &tenant, &company, &event.aggregate_id, mode)
            .await
            .map(|_| ())
    };
    if let Err(err) = read.await {
        tracing::warn!(tenant = %event.tenant, link = %event.aggregate_id, error = %err, "AliExpress product not read");
    }
    Ok(())
}

/// A shopper put something in a cart: an imported item not read lately is
/// asked for, by the node it belongs to.
#[evento::subscription]
async fn on_item_added<E: Executor>(
    context: &Context<'_, E>,
    event: Event<ItemAdded>,
) -> anyhow::Result<()> {
    let aliexpress: Arc<Aliexpress> = context.extract();
    let now = crate::auth::now();
    if !aliexpress.enabled() || now.saturating_sub(event.timestamp) > REQUEST_WINDOW_SECS {
        return Ok(());
    }
    let db: Db = context.extract();
    let store: Store = context.extract();
    let asked: anyhow::Result<()> = async {
        let Some(link) = link_of_product(&db, &event.tenant, &event.data.product_id).await? else {
            return Ok(());
        };
        let tried_lately = link
            .last_tried_at
            .is_some_and(|at| now.saturating_sub(at as u64) < CART_ADD_FRESH_SECS);
        if tried_lately || !aliexpress.share().owns(&link.id) {
            return Ok(());
        }
        timada_aliexpress::Command::new(store.0.clone(), &event.tenant)
            .request_sync(ALIEXPRESS_ACTOR, &link.id)
            .await?;
        note_tried(&db, &event.tenant, &link.id, now).await
    }
    .await;
    if let Err(err) = asked {
        tracing::warn!(tenant = %event.tenant, product = %event.data.product_id, error = %err, "AliExpress product not asked for after a cart add");
    }
    Ok(())
}

/// Fetches, maps and imports one item: the draft's id and the links of its
/// variants, or what the merchant reads about why there is none.
async fn import<E: Executor>(
    aliexpress: &Aliexpress,
    media: Arc<Media>,
    tenant_id: &str,
    catalog: &timada_catalog::Command<E>,
    actor: &str,
    ae_product_id: &str,
) -> Result<(String, Vec<timada_aliexpress::aggregator::SkuLink>), String> {
    let failed = |err: anyhow::Error| {
        tracing::error!(%tenant_id, %ae_product_id, error = %err, "AliExpress import failed");
        "The import failed on our side; try again.".to_owned()
    };
    let Some(merchant) = aliexpress.active_tenant(tenant_id).await.map_err(failed)? else {
        return Err("The shop is suspended.".into());
    };
    let Some(settings) = merchant.aliexpress.as_ref() else {
        return Err("Set what the shop buys on AliExpress first.".into());
    };
    let Some(company) = aliexpress.company_of(&merchant).await.map_err(failed)? else {
        return Err("Link the shop to its company first.".into());
    };
    let Some(client) = aliexpress.client_of(&company).map_err(failed)? else {
        return Err("Connect the company's AliExpress account first.".into());
    };
    let market = Aliexpress::market_of(settings);
    let product = client
        .product_delivered(ae_product_id, &market)
        .await
        .map_err(|err| {
            if err.gone {
                "AliExpress does not sell this product.".to_owned()
            } else if err.retryable {
                "AliExpress did not answer; try again in a moment.".to_owned()
            } else {
                format!("AliExpress refused: {}", err.message)
            }
        })?;
    let mapped = map_product(
        &product,
        &settings.currency,
        merchant.aliexpress_pricing.as_ref(),
    )
    .map_err(|err| err.to_string())?;
    let images = Arc::new(aliexpress.images(media, tenant_id));
    let imported = import_one(
        catalog,
        actor,
        &mapped.seed,
        Path::new(""),
        images,
        &ImportOptions {
            publish: false,
            concurrency: 4,
            ..Default::default()
        },
    )
    .await
    .map_err(|err| format!("The catalog refused it: {err:#}"))?;
    for warning in mapped.warnings.iter().chain(&imported.warnings) {
        tracing::info!(%tenant_id, %ae_product_id, %warning, "AliExpress import");
    }
    let skus = track_imported(
        catalog,
        actor,
        &imported.product_id,
        &imported.variant_ids,
        &mapped.skus,
    )
    .await
    .map_err(|err| format!("The stock could not be set: {err}"))?;
    if let Err(err) = catalog
        .set_product_delivery_days(actor, &imported.product_id, product.delivery_days)
        .await
    {
        tracing::warn!(%tenant_id, %ae_product_id, error = %err, "AliExpress delivery time not set");
    }
    Ok((imported.product_id, skus))
}
