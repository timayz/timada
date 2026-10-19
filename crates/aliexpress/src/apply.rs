//! What a read of AliExpress changes in the catalog: each combination's
//! stock, its price when a rule prices it, and the product taken off sale
//! once AliExpress no longer sells it. A read also finds the combinations
//! AliExpress sells and the shop does not; the merchant adds those as
//! variants ([`add_combinations`]) or ignores them. No HTTP here but the
//! pictures of what is added: the caller fetched the product.

use std::collections::HashMap;

use evento::Executor;
use timada_catalog::{
    AddImage, CatalogError, DefineVariant, MAX_OPTION_VALUES, MAX_STOCK, MAX_VARIANTS,
    OptionSelection, ProductState,
};
use timada_catalog_import::Images;
use timada_catalog_import::woocommerce::fit_sku;
use timada_tenancy::PricingRule;

use crate::aggregator::{NewSku, SkuLink};
use crate::command::LinkState;
use crate::map::{MappedSku, picture_url, selections_of, shop_price};
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
    /// The priced combinations AliExpress sells that the link does not.
    pub found: Vec<NewSku>,
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
        found: link.found.clone(),
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
    outcome.found = match product {
        Some(product) if !outcome.offline => found_in(product, &skus),
        _ => Vec::new(),
    };
    outcome.skus = skus;
    if outcome.offline && state.is_published() {
        catalog.unpublish_product(actor, product_id).await?;
        outcome.unpublished = true;
    }
    Ok(outcome)
}

/// The priced combinations of `product` that none of `linked` is.
fn found_in(product: &AeProduct, linked: &[SkuLink]) -> Vec<NewSku> {
    product
        .skus
        .iter()
        .filter(|sku| !linked.iter().any(|link| link.sku_id == sku.id))
        .filter_map(|sku| {
            Some(NewSku {
                sku_id: sku.id.clone(),
                selections: selections_of(sku),
                cost_cents: sku.cost_cents?,
                available: sku.available,
                image: sku
                    .properties
                    .iter()
                    .find_map(|property| property.image.as_deref())
                    .map(picture_url),
            })
        })
        .take(MAX_VARIANTS)
        .collect()
}

/// Whether a combination found can become a variant of the product as it
/// now is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fit {
    /// It can: these are its selections, once each value of `new_values`
    /// is added to its option (by id).
    Fits {
        selections: Vec<OptionSelection>,
        new_values: Vec<(String, String)>,
    },
    /// A variant of the product is that combination already.
    Exists(String),
    /// The product does not vary.
    NoVariants,
    /// It varies in a way the product does not.
    NewOption(String),
    /// It names no value for one of the product's options.
    MissingOption(String),
    /// One of its options has all the values it can take.
    TooManyValues(String),
    /// The product has all the variants it can sell.
    TooManyVariants,
}

impl Fit {
    pub fn fits(&self) -> bool {
        matches!(self, Self::Fits { .. } | Self::Exists(_))
    }

    /// Why it cannot be added, for the merchant; `None` when it can.
    pub fn reason(&self) -> Option<String> {
        Some(match self {
            Self::Fits { .. } | Self::Exists(_) => return None,
            Self::NoVariants => {
                "The product does not vary: import it again to sell its combinations.".into()
            }
            Self::NewOption(name) => {
                format!("It varies by {name}, which the product does not: import it again.")
            }
            Self::MissingOption(name) => format!("It names no {name}."),
            Self::TooManyValues(name) => format!("{name} has all the values it can take."),
            Self::TooManyVariants => "The product has all the variants it can sell.".into(),
        })
    }
}

/// Whether `found` can become a variant of `product`: its options matched
/// by name, its values by spelling (case aside), as an import made them.
pub fn fit(product: &ProductState, found: &NewSku) -> Fit {
    if product.options.is_empty() || product.variants.is_empty() {
        return Fit::NoVariants;
    }
    if let Some((name, _)) = found.selections.iter().find(|(name, _)| {
        !product
            .options
            .iter()
            .any(|option| option.name.eq_ignore_ascii_case(name.trim()))
    }) {
        return Fit::NewOption(name.clone());
    }
    let mut selections = Vec::with_capacity(product.options.len());
    let mut new_values = Vec::new();
    for option in &product.options {
        let Some((_, value)) = found
            .selections
            .iter()
            .find(|(name, _)| option.name.eq_ignore_ascii_case(name.trim()))
        else {
            return Fit::MissingOption(option.name.clone());
        };
        let value = value.trim();
        let value = match option
            .values
            .iter()
            .find(|known| known.eq_ignore_ascii_case(value))
        {
            Some(known) => known.clone(),
            None => {
                if option.values.len() >= MAX_OPTION_VALUES || value.is_empty() {
                    return Fit::TooManyValues(option.name.clone());
                }
                new_values.push((option.id.clone(), value.to_owned()));
                value.to_owned()
            }
        };
        selections.push(OptionSelection {
            option_id: option.id.clone(),
            value,
        });
    }
    if let Some(variant) = product.variant_for(&selections) {
        return Fit::Exists(variant.id.clone());
    }
    if product.variants.len() >= MAX_VARIANTS {
        return Fit::TooManyVariants;
    }
    Fit::Fits {
        selections,
        new_values,
    }
}

/// What [`add_combinations`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AddOutcome {
    /// The combinations now variants, for the link to record.
    pub skus: Vec<SkuLink>,
    /// What could not be added, for the merchant to read.
    pub warnings: Vec<String>,
}

/// Adds combinations the link found (`sku_ids`; empty: every one offered)
/// as variants of its product: the values their options lack, their
/// picture (through `images`), a price by `rule` (the cost without one) and
/// their stock. One a variant is already (an add that was not recorded) is
/// linked to it.
pub async fn add_combinations<E: Executor>(
    catalog: &timada_catalog::Command<E>,
    actor: &str,
    link: &LinkState,
    sku_ids: &[String],
    rule: Option<PricingRule>,
    images: &dyn Images,
) -> Result<AddOutcome, CatalogError> {
    let mut outcome = AddOutcome::default();
    let Some(product_id) = link.product_id.as_deref() else {
        return Ok(outcome);
    };
    let wanted: Vec<&NewSku> = link
        .offered()
        .filter(|sku| sku_ids.is_empty() || sku_ids.contains(&sku.sku_id))
        .collect();
    // A picture two combinations share is added once.
    let mut pictures: HashMap<String, String> = HashMap::new();
    for found in wanted {
        let Some(product) = catalog.load_product(product_id).await? else {
            outcome
                .warnings
                .push("the product is gone from the catalog".into());
            break;
        };
        if product.is_archived() {
            outcome.warnings.push("the product is archived".into());
            break;
        }
        let variant_id = match fit(&product, found) {
            Fit::Exists(variant_id) => variant_id,
            Fit::Fits {
                selections,
                new_values,
            } => match define(
                catalog,
                actor,
                &product,
                found,
                selections,
                new_values,
                rule,
                images,
                &mut pictures,
            )
            .await
            {
                Ok(variant_id) => variant_id,
                Err(err) if err.is_user_facing() => {
                    outcome
                        .warnings
                        .push(format!("combination {}: {err}", found.sku_id));
                    continue;
                }
                Err(err) => return Err(err),
            },
            other => {
                outcome.warnings.push(format!(
                    "combination {}: {}",
                    found.sku_id,
                    other.reason().unwrap_or_default()
                ));
                continue;
            }
        };
        catalog
            .set_stock(
                actor,
                product_id,
                Some(&variant_id),
                found.available.min(MAX_STOCK),
            )
            .await?;
        outcome.skus.push(SkuLink {
            sku_id: found.sku_id.clone(),
            variant_id: Some(variant_id),
            cost_cents: found.cost_cents,
            available: found.available,
        });
    }
    Ok(outcome)
}

/// Defines one combination that fits: its option values, its picture, the
/// variant itself (without its code when another product has it).
#[allow(clippy::too_many_arguments)]
async fn define<E: Executor>(
    catalog: &timada_catalog::Command<E>,
    actor: &str,
    product: &ProductState,
    found: &NewSku,
    selections: Vec<OptionSelection>,
    new_values: Vec<(String, String)>,
    rule: Option<PricingRule>,
    images: &dyn Images,
    pictures: &mut HashMap<String, String>,
) -> Result<String, CatalogError> {
    let currency = product
        .currency()
        .ok_or(CatalogError::InvalidCurrency)?
        .to_owned();
    for (option_id, value) in new_values {
        let Some(option) = product.option(&option_id) else {
            return Err(CatalogError::OptionNotFound);
        };
        let mut values = option.values.clone();
        values.push(value);
        catalog
            .change_option_values(actor, &product.id, &option_id, values)
            .await?;
    }
    let image_id = match &found.image {
        Some(url) => picture(catalog, actor, &product.id, url, images, pictures).await,
        None => None,
    };
    let mut cmd = DefineVariant {
        selections,
        sku: fit_sku(&found.sku_id),
        amount_cents: shop_price(rule, found.cost_cents),
        currency,
        image_id,
    };
    match catalog
        .define_variant(actor, &product.id, cmd.clone())
        .await
    {
        Err(CatalogError::SkuTaken) => {
            cmd.sku = None;
            catalog.define_variant(actor, &product.id, cmd).await
        }
        other => other,
    }
}

/// The picture at `url` added to the product, if it could be; one added in
/// the same run is reused.
async fn picture<E: Executor>(
    catalog: &timada_catalog::Command<E>,
    actor: &str,
    product_id: &str,
    url: &str,
    images: &dyn Images,
    pictures: &mut HashMap<String, String>,
) -> Option<String> {
    if let Some(image_id) = pictures.get(url) {
        return Some(image_id.clone());
    }
    let stored = match images.fetch(url).await {
        Ok(Some(stored)) => stored,
        Ok(None) => return None,
        Err(err) => {
            tracing::warn!(url, %err, "picture not fetched");
            return None;
        }
    };
    let added = catalog
        .add_image(
            actor,
            product_id,
            AddImage {
                image_id: stored.image_id.clone(),
                ext: stored.ext.clone(),
                size: stored.size,
                width: stored.width,
                height: stored.height,
                alt: String::new(),
            },
        )
        .await;
    match added {
        Ok(()) => {
            pictures.insert(url.to_owned(), stored.image_id.clone());
            Some(stored.image_id)
        }
        Err(err) => {
            tracing::warn!(url, %err, "picture not added");
            images.discard(&stored).await;
            None
        }
    }
}
