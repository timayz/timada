//! A snapshot taken from a WooCommerce shop through its public Store API
//! (`/wp-json/wc/store/v1`): the categories, the products and their
//! variations, mapped into what the catalog accepts. The shop's HTML
//! becomes text, its limits become the catalog's (a product's longest
//! description, its count of pictures, options, variants and facts), and a
//! variation's attribute values, which the API gives as slugs, become the
//! terms' names so the shopper reads "Bleu Foncé", not "bleu-fonce".

use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::Context;
use serde::Deserialize;
use timada_catalog::{
    CURRENCIES, MAX_ALT_LEN, MAX_ATTRIBUTE_NAME_LEN, MAX_ATTRIBUTE_VALUE_LEN, MAX_ATTRIBUTES,
    MAX_CATEGORIES_PER_PRODUCT, MAX_CATEGORY_DEPTH, MAX_CATEGORY_DESCRIPTION_LEN,
    MAX_CATEGORY_NAME_LEN, MAX_DESCRIPTION_LEN, MAX_IMAGES_PER_PRODUCT, MAX_NAME_LEN,
    MAX_OPTION_NAME_LEN, MAX_OPTION_VALUES, MAX_OPTIONS, MAX_SKU_LEN, MAX_SLUG_LEN, MAX_VARIANTS,
    slugify,
};

use crate::snapshot::{
    AttributeSeed, CategorySeed, ImageSeed, OptionSeed, ProductSeed, SelectionSeed, Snapshot,
    VariantSeed,
};
use crate::text::{clamp_chars, html_to_text, truncate_chars};

/// How the shop's categories are laid out in the snapshot.
#[derive(Debug, Clone, Default)]
pub struct SiteOptions {
    /// Top-level category slugs in the order the shop's menu shows them;
    /// the rest follow in the API's order.
    pub menu_order: Vec<String>,
    /// Category slugs taken out of the menu (kept, with their products).
    pub hidden: Vec<String>,
}

/// A page of the Store API at a time; the API caps it at 100.
const PER_PAGE: u32 = 100;

/// The API says `null` where it has no text; read as nothing.
fn text<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_default())
}

/// What the Store API says of a category.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct WooCategory {
    pub id: u64,
    pub name: String,
    pub slug: String,
    #[serde(default)]
    pub parent: u64,
    #[serde(default, deserialize_with = "text")]
    pub description: String,
    #[serde(default)]
    pub image: Option<WooImage>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct WooImage {
    pub src: String,
    #[serde(default, deserialize_with = "text")]
    pub alt: String,
}

/// What the Store API says of a product, or of a variation (then `parent`
/// names the product and `prices` are the variation's own).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct WooProduct {
    pub id: u64,
    pub name: String,
    pub slug: String,
    #[serde(default)]
    pub parent: u64,
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default, deserialize_with = "text")]
    pub description: String,
    #[serde(default, deserialize_with = "text")]
    pub short_description: String,
    #[serde(default, deserialize_with = "text")]
    pub sku: String,
    #[serde(default)]
    pub prices: WooPrices,
    #[serde(default)]
    pub images: Vec<WooImage>,
    #[serde(default)]
    pub categories: Vec<WooCategoryRef>,
    #[serde(default)]
    pub attributes: Vec<WooAttribute>,
    #[serde(default)]
    pub variations: Vec<WooVariationRef>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct WooPrices {
    /// Minor units, as a string.
    #[serde(default, deserialize_with = "text")]
    pub price: String,
    #[serde(default, deserialize_with = "text")]
    pub currency_code: String,
    #[serde(default)]
    pub price_range: Option<WooPriceRange>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct WooPriceRange {
    #[serde(default, deserialize_with = "text")]
    pub min_amount: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct WooCategoryRef {
    pub slug: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct WooAttribute {
    pub name: String,
    #[serde(default)]
    pub has_variations: bool,
    #[serde(default)]
    pub terms: Vec<WooTerm>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct WooTerm {
    pub name: String,
    #[serde(default, deserialize_with = "text")]
    pub slug: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct WooVariationRef {
    pub id: u64,
    #[serde(default)]
    pub attributes: Vec<WooVariationAttribute>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct WooVariationAttribute {
    pub name: String,
    /// A term's slug (`bleu-fonce`), or empty for "any".
    #[serde(default, deserialize_with = "text")]
    pub value: String,
}

/// Fetches a whole shop. `site` is its base address (`https://shop.example`).
pub async fn snapshot_site(
    client: &reqwest::Client,
    site: &str,
    options: &SiteOptions,
) -> anyhow::Result<Snapshot> {
    let base = format!("{}/wp-json/wc/store/v1", site.trim_end_matches('/'));
    let categories: Vec<WooCategory> = fetch_all(
        client,
        &format!("{base}/products/categories?per_page={PER_PAGE}"),
    )
    .await?;
    tracing::info!(count = categories.len(), "categories fetched");
    let products: Vec<WooProduct> =
        fetch_all(client, &format!("{base}/products?per_page={PER_PAGE}")).await?;
    tracing::info!(count = products.len(), "products fetched");
    let variations: Vec<WooProduct> = fetch_all(
        client,
        &format!("{base}/products?type=variation&per_page={PER_PAGE}"),
    )
    .await?;
    tracing::info!(count = variations.len(), "variations fetched");
    let mut snapshot = map_catalog(categories, products, variations, options);
    snapshot.source = site.to_owned();
    snapshot.fetched_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    Ok(snapshot)
}

/// Every page of a listing, one request at a time, each tried twice.
async fn fetch_all<T: serde::de::DeserializeOwned>(
    client: &reqwest::Client,
    url: &str,
) -> anyhow::Result<Vec<T>> {
    let mut out = Vec::new();
    for page in 1.. {
        let url = format!("{url}&page={page}");
        let mut attempt = 0;
        let items: Vec<T> = loop {
            attempt += 1;
            let result = async {
                client
                    .get(&url)
                    .send()
                    .await?
                    .error_for_status()?
                    .json::<Vec<T>>()
                    .await
            }
            .await;
            match result {
                Ok(items) => break items,
                Err(err) if attempt < 2 => {
                    tracing::warn!(%url, %err, "retrying");
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                }
                Err(err) => return Err(err).with_context(|| format!("GET {url}")),
            }
        };
        let count = items.len();
        out.extend(items);
        tracing::debug!(%url, count, total = out.len(), "page fetched");
        if count < PER_PAGE as usize {
            break;
        }
    }
    Ok(out)
}

/// Maps a shop's listings into a snapshot. `variations` are the shop's
/// variation listings (their own prices, codes and picture), joined to the
/// products through their parent id.
pub fn map_catalog(
    categories: Vec<WooCategory>,
    products: Vec<WooProduct>,
    variations: Vec<WooProduct>,
    options: &SiteOptions,
) -> Snapshot {
    let categories = map_categories(categories, options);
    let known: HashSet<&str> = categories.iter().map(|c| c.slug.as_str()).collect();
    let by_parent: HashMap<u64, Vec<&WooProduct>> =
        variations.iter().fold(HashMap::new(), |mut map, v| {
            map.entry(v.parent).or_default().push(v);
            map
        });
    let mut used_slugs = HashSet::new();
    let products = products
        .iter()
        .filter_map(|product| {
            let variations = by_parent.get(&product.id).map(Vec::as_slice).unwrap_or(&[]);
            let seed = map_product(product, variations, &known)?;
            if !used_slugs.insert(seed.slug.clone()) {
                tracing::warn!(slug = %seed.slug, id = product.id, "duplicate slug, product skipped");
                return None;
            }
            Some(seed)
        })
        .collect();
    Snapshot {
        source: String::new(),
        fetched_at: 0,
        categories,
        products,
    }
}

fn map_categories(categories: Vec<WooCategory>, options: &SiteOptions) -> Vec<CategorySeed> {
    let by_id: HashMap<u64, &WooCategory> = categories.iter().map(|c| (c.id, c)).collect();
    let depth_of = |category: &WooCategory| {
        let mut depth = 1;
        let mut parent = category.parent;
        while let Some(above) = by_id.get(&parent) {
            depth += 1;
            parent = above.parent;
            if depth > MAX_CATEGORY_DEPTH + 1 {
                break;
            }
        }
        depth
    };
    let kept: Vec<&WooCategory> = categories
        .iter()
        .filter(|c| {
            if c.slug == "uncategorized" {
                return false;
            }
            if depth_of(c) > MAX_CATEGORY_DEPTH {
                tracing::warn!(slug = %c.slug, "category too deep, skipped");
                return false;
            }
            true
        })
        .collect();
    let kept_ids: HashSet<u64> = kept.iter().map(|c| c.id).collect();
    // Siblings in the API's order (alphabetical), the top level as the menu
    // shows it.
    let menu_rank = |c: &WooCategory| {
        options
            .menu_order
            .iter()
            .position(|slug| *slug == c.slug)
            .unwrap_or(options.menu_order.len())
    };
    let mut groups: BTreeMap<u64, Vec<&WooCategory>> = BTreeMap::new();
    for category in &kept {
        let parent = if kept_ids.contains(&category.parent) {
            category.parent
        } else {
            0
        };
        groups.entry(parent).or_default().push(category);
    }
    if let Some(roots) = groups.get_mut(&0) {
        roots.sort_by_key(|c| menu_rank(c));
    }
    let mut out = Vec::with_capacity(kept.len());
    for (parent, siblings) in &groups {
        for (position, category) in siblings.iter().enumerate() {
            out.push(CategorySeed {
                slug: fit_slug(&category.slug, &category.name),
                name: clamp_chars(&category.name, MAX_CATEGORY_NAME_LEN),
                description: truncate_chars(
                    &html_to_text(&category.description),
                    MAX_CATEGORY_DESCRIPTION_LEN,
                ),
                parent: (*parent != 0)
                    .then(|| by_id.get(parent).map(|p| fit_slug(&p.slug, &p.name)))
                    .flatten(),
                position: position as u32,
                in_menu: !options.hidden.contains(&category.slug),
                image: category.image.as_ref().map(|image| ImageSeed {
                    url: image.src.clone(),
                    alt: clamp_chars(&image.alt, MAX_ALT_LEN),
                }),
            });
        }
    }
    out
}

fn map_product(
    product: &WooProduct,
    variations: &[&WooProduct],
    known: &HashSet<&str>,
) -> Option<ProductSeed> {
    let name = clamp_chars(&product.name, MAX_NAME_LEN);
    if name.is_empty() {
        tracing::warn!(id = product.id, "product without a name, skipped");
        return None;
    }
    let currency = product.prices.currency_code.trim().to_ascii_uppercase();
    let currency = if CURRENCIES.contains(&currency.as_str()) {
        currency
    } else {
        tracing::warn!(id = product.id, %currency, "currency not sold, EUR assumed");
        "EUR".to_owned()
    };
    let short = html_to_text(&product.short_description);
    let long = html_to_text(&product.description);
    let description = if long.is_empty() || long == short {
        short
    } else if short.is_empty() {
        long
    } else {
        format!("{short}\n\n{long}")
    };
    let description = truncate_chars(&description, MAX_DESCRIPTION_LEN);

    let mut seen = HashSet::new();
    let mut images: Vec<ImageSeed> = product
        .images
        .iter()
        .filter(|image| !image.src.is_empty() && seen.insert(image.src.as_str()))
        .take(MAX_IMAGES_PER_PRODUCT)
        .map(|image| ImageSeed {
            url: image.src.clone(),
            alt: clamp_chars(&image.alt, MAX_ALT_LEN),
        })
        .collect();

    let mut categories: Vec<String> = Vec::new();
    for category in &product.categories {
        if known.contains(category.slug.as_str()) && !categories.contains(&category.slug) {
            categories.push(category.slug.clone());
        }
    }
    categories.truncate(MAX_CATEGORIES_PER_PRODUCT);

    let mut options: Vec<OptionSeed> = Vec::new();
    let mut attributes: Vec<AttributeSeed> = Vec::new();
    for attribute in &product.attributes {
        if attribute.has_variations && !product.variations.is_empty() {
            if options.len() >= MAX_OPTIONS {
                tracing::warn!(id = product.id, option = %attribute.name, "too many options, dropped");
                continue;
            }
            let name = clamp_chars(&attribute.name, MAX_OPTION_NAME_LEN);
            let mut values: Vec<String> = Vec::new();
            for term in &attribute.terms {
                let value = clamp_chars(&term.name, MAX_OPTION_NAME_LEN);
                if value.is_empty() || values.iter().any(|v| v.eq_ignore_ascii_case(&value)) {
                    continue;
                }
                if values.len() >= MAX_OPTION_VALUES {
                    tracing::warn!(id = product.id, option = %name, %value, "too many values, dropped");
                    continue;
                }
                values.push(value);
            }
            if name.is_empty() || values.is_empty() {
                continue;
            }
            options.push(OptionSeed { name, values });
        } else {
            let value = attribute
                .terms
                .iter()
                .map(|t| t.name.trim())
                .filter(|t| !t.is_empty())
                .collect::<Vec<_>>()
                .join(", ");
            let name = clamp_chars(&attribute.name, MAX_ATTRIBUTE_NAME_LEN);
            if name.is_empty() || value.is_empty() || attributes.len() >= MAX_ATTRIBUTES {
                continue;
            }
            attributes.push(AttributeSeed {
                name,
                value: clamp_chars(&value, MAX_ATTRIBUTE_VALUE_LEN),
            });
        }
    }

    let mut variants: Vec<VariantSeed> = Vec::new();
    if !options.is_empty() {
        let by_id: HashMap<u64, &&WooProduct> = variations.iter().map(|v| (v.id, v)).collect();
        let mut combos: HashSet<Vec<String>> = HashSet::new();
        for variation in &product.variations {
            if variants.len() >= MAX_VARIANTS {
                tracing::warn!(id = product.id, "too many variants, the rest dropped");
                break;
            }
            let Some(selections) = selections_of(product, &options, variation) else {
                tracing::debug!(
                    id = product.id,
                    variation = variation.id,
                    "variation without a full selection, skipped"
                );
                continue;
            };
            let combo: Vec<String> = selections.iter().map(|s| s.value.to_lowercase()).collect();
            if !combos.insert(combo) {
                continue;
            }
            let listed = by_id.get(&variation.id);
            let amount_cents = listed
                .and_then(|v| parse_minor(&v.prices.price))
                .or_else(|| parse_minor(&product.prices.price))
                .unwrap_or_default();
            if amount_cents == 0 {
                tracing::debug!(
                    id = product.id,
                    variation = variation.id,
                    "variation without a price, skipped"
                );
                continue;
            }
            let image = listed
                .and_then(|v| v.images.first())
                .filter(|image| !image.src.is_empty())
                .and_then(|image| {
                    if let Some(index) = images.iter().position(|i| i.url == image.src) {
                        return Some(index);
                    }
                    if images.len() < MAX_IMAGES_PER_PRODUCT {
                        images.push(ImageSeed {
                            url: image.src.clone(),
                            alt: clamp_chars(&image.alt, MAX_ALT_LEN),
                        });
                        return Some(images.len() - 1);
                    }
                    None
                });
            variants.push(VariantSeed {
                selections,
                sku: listed.and_then(|v| fit_sku(&v.sku)),
                amount_cents,
                image,
            });
        }
    }

    let amount_cents = if variants.is_empty() {
        options.clear();
        let own = parse_minor(&product.prices.price).filter(|&cents| cents > 0);
        let floor = product
            .prices
            .price_range
            .as_ref()
            .and_then(|range| parse_minor(&range.min_amount))
            .filter(|&cents| cents > 0);
        match own.or(floor) {
            Some(cents) => Some(cents),
            None => {
                tracing::warn!(id = product.id, slug = %product.slug, "product without a price, skipped");
                return None;
            }
        }
    } else {
        None
    };

    Some(ProductSeed {
        slug: fit_slug(&product.slug, &product.name),
        name,
        description,
        sku: fit_sku(&product.sku),
        currency,
        amount_cents,
        categories,
        images,
        options,
        variants,
        attributes,
    })
}

/// A variation's value for every option, as the terms' names.
fn selections_of(
    product: &WooProduct,
    options: &[OptionSeed],
    variation: &WooVariationRef,
) -> Option<Vec<SelectionSeed>> {
    let mut selections = Vec::with_capacity(options.len());
    for option in options {
        let given = variation
            .attributes
            .iter()
            .find(|a| a.name.trim().eq_ignore_ascii_case(&option.name))?;
        let value = given.value.trim();
        if value.is_empty() {
            return None;
        }
        let attribute = product
            .attributes
            .iter()
            .find(|a| a.name.trim().eq_ignore_ascii_case(&option.name))?;
        let term = attribute
            .terms
            .iter()
            .find(|t| t.slug == value || t.name.eq_ignore_ascii_case(value))
            .map(|t| clamp_chars(&t.name, MAX_OPTION_NAME_LEN))
            .or_else(|| {
                // A value the attribute does not list: taken as typed when
                // the option knows it.
                option
                    .values
                    .iter()
                    .find(|v| v.eq_ignore_ascii_case(value))
                    .cloned()
            })?;
        if !option.values.iter().any(|v| v.eq_ignore_ascii_case(&term)) {
            return None;
        }
        selections.push(SelectionSeed {
            option: option.name.clone(),
            value: term,
        });
    }
    Some(selections)
}

/// The Store API's amounts are minor units as strings (`"1990"`).
fn parse_minor(amount: &str) -> Option<u64> {
    amount.trim().parse().ok()
}

/// The shop's slug made to fit the catalog's: lowercase, `[a-z0-9-]`, at
/// most 64 characters cut at a dash; the name's slug when nothing is left.
pub fn fit_slug(slug: &str, name: &str) -> String {
    let lowered = slug.trim().to_ascii_lowercase();
    let mut out = String::with_capacity(lowered.len());
    for c in lowered.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let mut out = out.trim_matches('-').to_owned();
    if out.len() > MAX_SLUG_LEN {
        let cut = out[..MAX_SLUG_LEN].rfind('-').unwrap_or(MAX_SLUG_LEN);
        out.truncate(cut);
        out = out.trim_matches('-').to_owned();
    }
    if out.len() < 2 {
        return slugify(name);
    }
    out
}

/// A shop's code made to fit the catalog's: letters, digits, dot, dash and
/// underscore, at most 64; `None` when nothing is left.
fn fit_sku(sku: &str) -> Option<String> {
    let kept: String = sku
        .trim()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        .take(MAX_SKU_LEN)
        .collect();
    (!kept.is_empty()).then_some(kept)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_and_skus_fit_the_catalog() {
        assert_eq!(
            fit_slug("Tapis-Rafraichissant", "x"),
            "tapis-rafraichissant"
        );
        assert_eq!(fit_slug("a--b__c", "x"), "a-b-c");
        assert_eq!(fit_slug("", "Un Chat"), "un-chat");
        let long = "abcde-".repeat(20);
        let fitted = fit_slug(&long, "x");
        assert!(
            fitted.len() <= MAX_SLUG_LEN && !fitted.ends_with('-'),
            "{fitted}"
        );
        assert_eq!(
            fit_sku(" 1005001599932565-1 "),
            Some("1005001599932565-1".into())
        );
        assert_eq!(fit_sku("ab c/d"), Some("abcd".into()));
        assert_eq!(fit_sku("  "), None);
        assert_eq!(parse_minor("1990"), Some(1990));
        assert_eq!(parse_minor(""), None);
    }
}
