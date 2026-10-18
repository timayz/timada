//! What a read of AliExpress changes in the catalog: each combination's
//! stock, its price when a rule prices it, and the product taken off sale
//! once AliExpress no longer sells it. No HTTP here: the caller fetched the
//! product.

use evento::Executor;
use timada_catalog::{CatalogError, MAX_STOCK};
use timada_tenancy::PricingRule;

use crate::aggregator::SkuLink;
use crate::command::LinkState;
use crate::map::MappedSku;
use crate::model::AeProduct;

/// What [`apply_sync`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncOutcome {
    /// The combinations as now read, for the link to record.
    pub skus: Vec<SkuLink>,
    pub offline: bool,
    /// Prices changed.
    pub repriced: usize,
    /// Whether the product was taken off sale by this read.
    pub unpublished: bool,
    /// What could not be done, for the merchant to read.
    pub warnings: Vec<String>,
}

/// Counts what AliExpress has of each combination an import created: the
/// links the import records. `variant_ids` are the seed's, in its order
/// (`None` for one the catalog refused); empty for a product that does not
/// vary.
pub async fn track_imported<E: Executor>(
    catalog: &timada_catalog::Command<E>,
    actor: &str,
    product_id: &str,
    variant_ids: &[Option<String>],
    skus: &[MappedSku],
) -> Result<Vec<SkuLink>, CatalogError> {
    let mut links = Vec::with_capacity(skus.len());
    if variant_ids.is_empty() {
        if let Some(sku) = skus.first() {
            catalog
                .set_stock(actor, product_id, None, sku.available.min(MAX_STOCK))
                .await?;
            links.push(link_of(sku, None));
        }
        return Ok(links);
    }
    for (sku, variant_id) in skus.iter().zip(variant_ids) {
        let Some(variant_id) = variant_id else {
            continue;
        };
        catalog
            .set_stock(
                actor,
                product_id,
                Some(variant_id),
                sku.available.min(MAX_STOCK),
            )
            .await?;
        links.push(link_of(sku, Some(variant_id.clone())));
    }
    Ok(links)
}

fn link_of(sku: &MappedSku, variant_id: Option<String>) -> SkuLink {
    SkuLink {
        sku_id: sku.sku_id.clone(),
        variant_id,
        cost_cents: sku.cost_cents,
        available: sku.available,
    }
}

/// Brings the linked product in step with `product` as just read (`None`:
/// AliExpress no longer has it). `rule` is the price rule that applies, the
/// link's own or the shop's; without one prices are left alone.
pub async fn apply_sync<E: Executor>(
    catalog: &timada_catalog::Command<E>,
    actor: &str,
    link: &LinkState,
    product: Option<&AeProduct>,
    rule: Option<PricingRule>,
) -> Result<SyncOutcome, CatalogError> {
    let mut outcome = SyncOutcome {
        skus: link.skus.clone(),
        offline: product.is_none_or(|product| !product.on_sale()),
        ..Default::default()
    };
    let Some(product_id) = link.product_id.as_deref() else {
        return Ok(outcome);
    };
    let Some(state) = catalog.load_product(product_id).await? else {
        outcome
            .warnings
            .push("the product is gone from the catalog".into());
        return Ok(outcome);
    };
    if state.is_archived() {
        outcome.warnings.push("the product is archived".into());
        return Ok(outcome);
    }
    let currency = state.currency().map(str::to_owned);
    let mut skus = Vec::with_capacity(link.skus.len());
    for linked in &link.skus {
        let read = product.and_then(|product| product.sku(&linked.sku_id));
        let available = if outcome.offline {
            0
        } else {
            read.map_or(0, |sku| sku.available)
        };
        let cost_cents = read
            .and_then(|sku| sku.cost_cents)
            .unwrap_or(linked.cost_cents);
        if read.is_none() && !outcome.offline {
            outcome
                .warnings
                .push(format!("combination {} is no longer sold", linked.sku_id));
        }
        match catalog
            .set_stock(
                actor,
                product_id,
                linked.variant_id.as_deref(),
                available.min(MAX_STOCK),
            )
            .await
        {
            Ok(()) => {}
            Err(CatalogError::VariantNotFound) => {
                outcome.warnings.push(format!(
                    "the variant of combination {} was removed",
                    linked.sku_id
                ));
                skus.push(SkuLink {
                    available,
                    cost_cents,
                    ..linked.clone()
                });
                continue;
            }
            Err(err) => return Err(err),
        }
        if let (Some(rule), Some(currency)) = (rule, currency.as_deref())
            && !outcome.offline
            && read.is_some()
        {
            let price = rule.price(cost_cents);
            let current = match linked.variant_id.as_deref() {
                Some(variant_id) => state
                    .variant(variant_id)
                    .map(|variant| variant.price.amount_cents),
                None => state.price.as_ref().map(|price| price.amount_cents),
            };
            if current != Some(price) {
                let priced = match linked.variant_id.as_deref() {
                    Some(variant_id) => {
                        catalog
                            .price_variant(actor, product_id, variant_id, price, currency)
                            .await
                    }
                    None => {
                        catalog
                            .price_product(actor, product_id, price, currency)
                            .await
                    }
                };
                match priced {
                    Ok(()) => outcome.repriced += 1,
                    Err(err) if err.is_user_facing() => outcome
                        .warnings
                        .push(format!("combination {}: {err}", linked.sku_id)),
                    Err(err) => return Err(err),
                }
            }
        }
        skus.push(SkuLink {
            available,
            cost_cents,
            ..linked.clone()
        });
    }
    outcome.skus = skus;
    if outcome.offline && state.is_published() {
        catalog.unpublish_product(actor, product_id).await?;
        outcome.unpublished = true;
    }
    Ok(outcome)
}
