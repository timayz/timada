//! An AliExpress product as the catalog takes it: a [`ProductSeed`] the
//! import fills a draft from, and which AliExpress combination each of its
//! variants is, with what it costs and how many are left.

use std::collections::HashSet;

use timada_catalog::{
    CURRENCIES, MAX_ATTRIBUTE_NAME_LEN, MAX_ATTRIBUTE_VALUE_LEN, MAX_ATTRIBUTES,
    MAX_DESCRIPTION_LEN, MAX_IMAGES_PER_PRODUCT, MAX_NAME_LEN, MAX_OPTION_NAME_LEN,
    MAX_OPTION_VALUES, MAX_OPTIONS, MAX_VARIANTS, slugify_or,
};
use timada_catalog_import::woocommerce::fit_sku;
use timada_catalog_import::{
    AttributeSeed, ImageSeed, OptionSeed, ProductSeed, SelectionSeed, VariantSeed, decode_entities,
    html_to_text, truncate_chars,
};
use timada_tenancy::PricingRule;

use crate::model::AeProduct;

/// The longest value an option takes.
const MAX_OPTION_VALUE_LEN: usize = 40;

/// One AliExpress combination the shop sells.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MappedSku {
    pub sku_id: String,
    pub cost_cents: u64,
    pub available: u32,
}

/// What [`map_product`] makes of a product.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mapped {
    pub seed: ProductSeed,
    /// One per variant of the seed, in its order; one alone for a product
    /// that does not vary.
    pub skus: Vec<MappedSku>,
    /// What was left out, for the merchant to read.
    pub warnings: Vec<String>,
}

/// Why a product cannot be imported.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MapError {
    #[error("AliExpress prices it in {0}, which the shop does not sell in")]
    Currency(String),
    #[error("none of its combinations has a price")]
    Unpriced,
    #[error("AliExpress no longer sells it")]
    Offline,
}

/// The shop's price of something that costs `cost_cents`: `rule`'s, or the
/// cost itself without one (the merchant prices it before publishing).
pub fn shop_price(rule: Option<PricingRule>, cost_cents: u64) -> u64 {
    rule.map_or(cost_cents, |rule| rule.price(cost_cents))
}

/// Maps `product`, priced in `currency` (what the shop asked AliExpress
/// for), with `rule` turning costs into prices.
pub fn map_product(
    product: &AeProduct,
    currency: &str,
    rule: Option<PricingRule>,
) -> Result<Mapped, MapError> {
    if !product.on_sale() {
        return Err(MapError::Offline);
    }
    let currency = if product.currency.is_empty() {
        currency.to_ascii_uppercase()
    } else {
        product.currency.to_ascii_uppercase()
    };
    if !CURRENCIES.contains(&currency.as_str()) {
        return Err(MapError::Currency(currency));
    }
    let mut warnings = Vec::new();
    let name = truncate_chars(&decode_entities(&product.subject), MAX_NAME_LEN);
    let name = if name.trim().is_empty() {
        format!("AliExpress {}", product.id)
    } else {
        name
    };
    let slug = slugify_or(&name, &format!("aliexpress-{}", product.id));
    let description = truncate_chars(&html_to_text(&product.detail), MAX_DESCRIPTION_LEN);

    // The product's own pictures, then each combination's.
    let mut images: Vec<ImageSeed> = Vec::new();
    let picture = |url: &str, images: &mut Vec<ImageSeed>| -> Option<usize> {
        let url = if url.starts_with("//") {
            format!("https:{url}")
        } else {
            url.to_owned()
        };
        if let Some(index) = images.iter().position(|image| image.url == url) {
            return Some(index);
        }
        if images.len() >= MAX_IMAGES_PER_PRODUCT || url.to_ascii_lowercase().ends_with(".avif") {
            return None;
        }
        images.push(ImageSeed {
            url,
            alt: String::new(),
            file: None,
        });
        Some(images.len() - 1)
    };
    for url in &product.images {
        picture(url, &mut images);
    }

    let priced: Vec<_> = product
        .skus
        .iter()
        .filter_map(|sku| sku.cost_cents.map(|cost| (sku, cost)))
        .collect();
    if priced.is_empty() {
        return Err(MapError::Unpriced);
    }
    let varies = priced.len() > 1 || priced.iter().any(|(sku, _)| !sku.properties.is_empty());

    let mut seed = ProductSeed {
        slug,
        name,
        description,
        sku: None,
        currency: currency.clone(),
        amount_cents: None,
        categories: Vec::new(),
        images: Vec::new(),
        options: Vec::new(),
        variants: Vec::new(),
        attributes: Vec::new(),
    };
    let mut skus = Vec::new();

    if !varies {
        let (sku, cost) = priced[0];
        seed.amount_cents = Some(shop_price(rule, cost));
        skus.push(MappedSku {
            sku_id: sku.id.clone(),
            cost_cents: cost,
            available: sku.available,
        });
    } else {
        // The options in the order AliExpress names them, each with the
        // values its combinations take.
        let mut options: Vec<OptionSeed> = Vec::new();
        for (sku, _) in &priced {
            for property in &sku.properties {
                let option_name = truncate_chars(&property.name, MAX_OPTION_NAME_LEN);
                let value = truncate_chars(&property.value, MAX_OPTION_VALUE_LEN);
                let option = match options.iter_mut().position(|o| o.name == option_name) {
                    Some(index) => &mut options[index],
                    None if options.len() < MAX_OPTIONS => {
                        options.push(OptionSeed {
                            name: option_name.clone(),
                            values: Vec::new(),
                        });
                        options.last_mut().expect("just pushed")
                    }
                    None => continue,
                };
                if !option.values.contains(&value) && option.values.len() < MAX_OPTION_VALUES {
                    option.values.push(value);
                }
            }
        }
        let mut seen: HashSet<Vec<(String, String)>> = HashSet::new();
        for (sku, cost) in &priced {
            if seed.variants.len() >= MAX_VARIANTS {
                warnings.push(format!(
                    "only the first {MAX_VARIANTS} combinations were kept"
                ));
                break;
            }
            let mut selections: Vec<SelectionSeed> = Vec::new();
            let mut image = None;
            for option in &options {
                let Some(property) = sku
                    .properties
                    .iter()
                    .find(|p| truncate_chars(&p.name, MAX_OPTION_NAME_LEN) == option.name)
                else {
                    break;
                };
                let value = truncate_chars(&property.value, MAX_OPTION_VALUE_LEN);
                if !option.values.contains(&value) {
                    break;
                }
                if image.is_none()
                    && let Some(url) = &property.image
                {
                    image = picture(url, &mut images);
                }
                selections.push(SelectionSeed {
                    option: option.name.clone(),
                    value,
                });
            }
            if selections.len() != options.len() {
                warnings.push(format!("combination {} left out", sku.id));
                continue;
            }
            let key: Vec<(String, String)> = selections
                .iter()
                .map(|s| (s.option.clone(), s.value.clone()))
                .collect();
            if !seen.insert(key) {
                continue;
            }
            seed.variants.push(VariantSeed {
                selections,
                sku: fit_sku(&sku.id),
                amount_cents: shop_price(rule, *cost),
                image,
            });
            skus.push(MappedSku {
                sku_id: sku.id.clone(),
                cost_cents: *cost,
                available: sku.available,
            });
        }
        seed.options = options;
    }
    seed.images = images;

    for (name, value) in product.properties.iter().take(MAX_ATTRIBUTES) {
        seed.attributes.push(AttributeSeed {
            name: truncate_chars(name, MAX_ATTRIBUTE_NAME_LEN),
            value: truncate_chars(value, MAX_ATTRIBUTE_VALUE_LEN),
        });
    }
    Ok(Mapped {
        seed,
        skus,
        warnings,
    })
}
