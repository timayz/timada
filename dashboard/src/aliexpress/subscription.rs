//! Imports the AliExpress items a shop asked for: fetches each from
//! AliExpress with the company's account, fills a draft from it (pictures,
//! options, variants, their stock) and records which combination each
//! variant is. Every node runs it; the first to take
//! an import on does it, the others leave it.

use std::path::Path;
use std::sync::Arc;

use evento::{
    Evento, Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use timada_aliexpress::aggregator::ImportRequested;
use timada_aliexpress::{LinkStatus, map_product, track_imported};
use timada_catalog_import::{ImportOptions, import_one};

use super::{ALIEXPRESS_ACTOR, Aliexpress};
use crate::media::Media;

/// The cursor identity of the subscription, in the store.
pub const SUBSCRIPTION: &str = "timada-dashboard/aliexpress-import";

/// A request older than this is history being replayed (a node catching
/// up): left alone, the merchant may ask again.
const IMPORT_WINDOW_SECS: u64 = 10 * 60;

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
        .any_routing_key()
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
    if now.saturating_sub(event.timestamp) > IMPORT_WINDOW_SECS {
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
        .product(ae_product_id, &market)
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
    let mapped = map_product(&product, &settings.currency, settings.pricing.as_ref())
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
    Ok((imported.product_id, skus))
}
