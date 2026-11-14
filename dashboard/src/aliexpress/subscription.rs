//! Imports the AliExpress items a shop asked for: fetches each from
//! AliExpress with the company's account, fills a draft from it (pictures,
//! options, variants, their stock) and records which combination each
//! variant is. Every node runs it; the first to take
//! an import on does it, the others leave it.
//!
//! When a shopper puts an imported item in a cart, the node the item belongs
//! to reads it again, on a cursor of its own so a long import does not hold
//! it up ([`reads_subscription`]): what the checkout shows is then already
//! what AliExpress asks. The cart's own event is the request; nothing is
//! written to the item's stream unless the read finds something (#472).

use std::path::Path;
use std::sync::Arc;

use evento::{
    Evento, Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use timada_aliexpress::aggregator::ImportRequested;
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

/// A cart add reads nothing when the item was tried this recently: each read
/// spends a call or two of the app's day, and the payment reads it anyway.
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

/// Reads the linked items shoppers put in a cart. `db` is the read-model
/// database, which must be allowed to write (it keeps when each item was
/// read).
pub fn reads_subscription<E: Executor>(
    aliexpress: Arc<Aliexpress>,
    db: Db,
    store: Evento,
) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(READS_SUBSCRIPTION)
        .data(aliexpress)
        .data(db)
        .data(Store(store))
        .handler(on_item_added())
        // A saga: it follows the node's own region (the executor's default
        // routing key), so a region's nodes do its work and the other
        // regions' nodes leave it to them.
        .retry(5)
}

/// A shopper put something in a cart: an imported item not read lately is
/// read, by the node it belongs to. A store or AliExpress that fails here
/// fails this read alone, or it would hold up the carts behind it: the
/// payment reads the item anyway, and the sweep comes back.
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
    let read: anyhow::Result<()> = async {
        let Some(link) = link_of_product(&db, &event.tenant, &event.data.product_id).await? else {
            return Ok(());
        };
        let tried_lately = link
            .last_tried_at
            .is_some_and(|at| now.saturating_sub(at as u64) < CART_ADD_FRESH_SECS);
        if tried_lately
            || !aliexpress
                .share_in(event.routing_key.as_deref())
                .owns(&link.id)
        {
            return Ok(());
        }
        // Tried, whatever comes of it: the next cart waits its hour.
        note_tried(&db, &event.tenant, &link.id, now).await?;
        let Some((tenant, company)) = aliexpress
            .connected_shop(&store.0, &event.tenant, now)
            .await?
        else {
            return Ok(());
        };
        aliexpress
            .sync_link(
                &db,
                &store.0,
                &tenant,
                &company,
                &link.id,
                SyncMode::Routine,
            )
            .await
            .map(|_| ())
    }
    .await;
    if let Err(err) = read {
        tracing::warn!(tenant = %event.tenant, product = %event.data.product_id, error = %err, "AliExpress product not read after a cart add");
    }
    Ok(())
}

/// Fetches, maps and imports one item: the draft's id and the links of its
/// variants, or what the merchant reads about why there is none.
async fn import<E: Executor + Clone>(
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
    let Some(merchant) = aliexpress
        .active_tenant(catalog.executor(), tenant_id)
        .await
        .map_err(failed)?
    else {
        return Err("The shop is suspended.".into());
    };
    let Some(settings) = merchant.aliexpress.as_ref() else {
        return Err("Set what the shop buys on AliExpress first.".into());
    };
    let Some(company) = aliexpress
        .company_of(catalog.executor(), &merchant)
        .await
        .map_err(failed)?
    else {
        return Err("Link the shop to its company first.".into());
    };
    let Some(client) = aliexpress.client_of(&company).map_err(failed)? else {
        return Err("Connect the company's AliExpress account first.".into());
    };
    let market = Aliexpress::market_of(settings);
    let product = aliexpress
        .read_delivered(&client, ae_product_id, &market)
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
    let images = Arc::new(aliexpress.images(media, &merchant));
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
