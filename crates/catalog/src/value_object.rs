//! Identifiers, limits, the slug rules, money parsing and formatting.

use crate::CatalogError;

pub const MAX_NAME_LEN: usize = 120;
pub const MAX_DESCRIPTION_LEN: usize = 4000;
pub use timada_core::MIN_SLUG_LEN;
pub use timada_core::{MAX_META_DESCRIPTION_LEN, MAX_META_TITLE_LEN};
pub const MAX_SLUG_LEN: usize = 64;
/// Length of the random suffix added when a derived slug is already taken.
pub use timada_core::SLUG_SUFFIX_LEN;
/// The slug of a name with no usable characters.
pub const FALLBACK_SLUG: &str = "product";
/// The largest price, in minor units: a thousand million.
pub const MAX_AMOUNT_CENTS: u64 = 100_000_000_000;
/// The currencies a product can be priced in, as ISO-4217 codes. All of them
/// have two minor digits.
/// What kind of goods a product is for VAT, with the label a page shows.
/// Each country taxes a kind at its own rate (books are 5.5 % in France,
/// 7 % in Germany); most goods are `standard`. The list only grows.
pub const TAX_CATEGORIES: &[(&str, &str)] = &[
    (TAX_CATEGORY_STANDARD, "Standard"),
    ("books", "Books"),
    ("food", "Food"),
    ("medicines", "Medicines"),
    ("press", "Newspapers and magazines"),
];
/// The kind of goods a product is unless told otherwise.
pub const TAX_CATEGORY_STANDARD: &str = "standard";

/// The label of a tax category, or the key itself if unknown.
pub fn tax_category_label(category: &str) -> &str {
    TAX_CATEGORIES
        .iter()
        .find(|(key, _)| *key == category)
        .map_or(category, |(_, label)| label)
}

pub use timada_core::CURRENCIES;
pub const MAX_CATEGORY_NAME_LEN: usize = 80;
pub const MAX_CATEGORY_DESCRIPTION_LEN: usize = 2000;
/// The longest text below a category's products.
pub const MAX_CATEGORY_BODY_LEN: usize = 20_000;
/// How deep the category tree goes: a top-level category is at depth 1, so
/// `Bedding > Mats > Cooling mats` is the most.
pub const MAX_CATEGORY_DEPTH: usize = 3;
/// The slug of a category name with no usable characters.
pub const FALLBACK_CATEGORY_SLUG: &str = "category";
/// How many categories one product belongs to, at most.
pub const MAX_CATEGORIES_PER_PRODUCT: usize = 10;
/// How many pictures one product carries, at most.
pub const MAX_IMAGES_PER_PRODUCT: usize = 20;
/// The largest upload, in bytes. What is kept of it is smaller: the two
/// renditions of [`timada_core::Picture`].
pub const MAX_IMAGE_BYTES: u64 = 8 * 1024 * 1024;
pub const MAX_ALT_LEN: usize = 200;
/// The picture formats an upload may come in, as `(extension, type)`. What
/// is kept is always WebP ([`timada_core::RENDITION_EXT`]).
pub const IMAGE_FORMATS: &[(&str, &str)] = &[
    ("jpg", "image/jpeg"),
    ("png", "image/png"),
    ("webp", "image/webp"),
    ("gif", "image/gif"),
];

/// How many ways one product varies, at most: a matrix of more than three
/// is a different product, not a variant.
pub const MAX_OPTIONS: usize = 3;
/// How many values one option takes, at most.
pub const MAX_OPTION_VALUES: usize = 20;
pub const MAX_OPTION_NAME_LEN: usize = 40;
/// The longest label an option or one of its values shows instead of its
/// name.
pub const MAX_OPTION_LABEL_LEN: usize = 40;
/// How many combinations one product sells, at most.
pub const MAX_VARIANTS: usize = 100;
pub const MAX_SKU_LEN: usize = 64;
/// How many facts one product's table holds, at most.
pub const MAX_ATTRIBUTES: usize = 30;
pub const MAX_ATTRIBUTE_NAME_LEN: usize = 40;
pub const MAX_ATTRIBUTE_VALUE_LEN: usize = 400;
/// The longest brand and the longest manufacturer's part number a shopping
/// feed takes.
pub const MAX_BRAND_LEN: usize = 70;
pub const MAX_MPN_LEN: usize = 70;
/// How many free labels a product carries for ad campaigns: a shopping
/// feed's `custom_label_0` to `custom_label_4`.
pub const FEED_LABELS: usize = 5;
pub const MAX_FEED_LABEL_LEN: usize = 100;
/// The longest category of Google's product taxonomy: its number (`2271`)
/// or its full path (`Apparel & Accessories > Clothing > Dresses`).
pub const MAX_GOOGLE_CATEGORY_LEN: usize = 750;
/// The ages a product is made for, as a shopping feed names them, with the
/// label a page shows.
pub const AGE_GROUPS: &[(&str, &str)] = &[
    ("newborn", "Newborn (up to 3 months)"),
    ("infant", "Infant (3 to 12 months)"),
    ("toddler", "Toddler (1 to 5 years)"),
    ("kids", "Kids (5 to 13 years)"),
    ("adult", "Adult"),
];
/// Whom a product is made for, as a shopping feed names them.
pub const GENDERS: &[(&str, &str)] =
    &[("female", "Female"), ("male", "Male"), ("unisex", "Unisex")];
/// What an option can be to a shopping feed and to search engines, which
/// know only these ways for a product to vary. An option of another kind
/// (where it ships from) is none of them.
pub const OPTION_KINDS: &[(&str, &str)] = &[
    ("color", "Colour"),
    ("size", "Size"),
    ("material", "Material"),
    ("pattern", "Pattern"),
];

/// One option of a variant and the value it takes: `Colour` is `Beige`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, bitcode::Encode, bitcode::Decode)]
pub struct OptionSelection {
    pub option_id: String,
    pub value: String,
}

/// A combination's new price, among those a supplier's read changed at once
/// ([`Product::VariantsRepriced`](crate::aggregator::Product)).
#[derive(Debug, Clone, PartialEq, Eq, Hash, bitcode::Encode, bitcode::Decode)]
pub struct VariantPrice {
    pub variant_id: String,
    pub amount_cents: u64,
}

/// What the shop pays for one of a combination, among those set at once
/// ([`ProductCost::VariantCostsSet`](crate::aggregator::ProductCost)); `None`
/// when it is not known.
#[derive(Debug, Clone, PartialEq, Eq, Hash, bitcode::Encode, bitcode::Decode)]
pub struct VariantCost {
    pub variant_id: String,
    pub amount_cents: Option<u64>,
}

/// The index stream of a stock code.
pub fn product_sku_id(sku: &str) -> String {
    evento::hash_ids(vec!["product-sku", &sku_key(sku)])
}

/// The stock stream of generation `generation` of a product without
/// variants (`variant_id` `None`) or of one of its variants.
pub fn stock_item_id(product_id: &str, variant_id: Option<&str>, generation: u32) -> String {
    let generation = generation.to_string();
    evento::hash_ids(vec![
        "stock",
        product_id,
        variant_id.unwrap_or(""),
        generation.as_str(),
    ])
}

/// The stream naming the current stock generation of a thing.
pub fn stock_pointer_id(product_id: &str, variant_id: Option<&str>) -> String {
    evento::hash_ids(vec!["stock-pointer", product_id, variant_id.unwrap_or("")])
}

/// The stream of what one order took off the shelves.
pub fn stock_take_id(order_id: &str) -> String {
    evento::hash_ids(vec!["stock-take", order_id])
}

/// The stream saying a product was ordered.
pub fn product_ordered_id(product_id: &str) -> String {
    evento::hash_ids(vec!["product-ordered", product_id])
}

/// How many events a stock generation holds before the next one is opened:
/// well inside a stream's 65 535 versions, and short enough that loading a
/// thing's stock stays cheap.
pub const STOCK_CHUNK: u16 = 1000;

/// How many of the last orders a stock generation remembers, so an order
/// delivered again is told apart from a new one before its own stream says
/// so.
pub const RECENT_ORDERS: usize = 256;

/// A hold carried into a new stock generation: what the checkout of
/// `cart_id` holds until `expires_at` (unix seconds, excluded).
#[derive(Debug, Clone, PartialEq, Eq, bitcode::Encode, bitcode::Decode)]
pub struct CarriedHold {
    pub cart_id: String,
    pub quantity: u32,
    pub expires_at: u64,
}

/// An order a stock generation remembers: the one made from `cart_id`
/// (unknown when only its restoration is remembered), and whether what it
/// took is back on the shelf.
#[derive(Debug, Clone, PartialEq, Eq, bitcode::Encode, bitcode::Decode)]
pub struct CarriedTake {
    pub order_id: String,
    pub cart_id: Option<String>,
    pub restored: bool,
}

/// Some of a counted thing an order took: a product without variants
/// (`variant_id` `None`) or one variant.
#[derive(Debug, Clone, PartialEq, Eq, bitcode::Encode, bitcode::Decode)]
pub struct TakenLine {
    pub product_id: String,
    pub variant_id: Option<String>,
    pub quantity: u32,
}

/// The most a shop counts of one thing.
pub const MAX_STOCK: u32 = 1_000_000;

/// The longest delivery a shop can announce, in days.
pub const MAX_DELIVERY_DAYS: u16 = 365;

/// How long delivery takes, in days from the order: between `min` and `max`,
/// the same number when the shop gives one.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    bitcode::Encode,
    bitcode::Decode,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct DeliveryDays {
    pub min: u16,
    pub max: u16,
}

impl DeliveryDays {
    /// Checks 1 ≤ `min` ≤ `max` ≤ [`MAX_DELIVERY_DAYS`].
    pub fn new(min: u16, max: u16) -> Result<Self, CatalogError> {
        if min == 0 || min > max || max > MAX_DELIVERY_DAYS {
            return Err(CatalogError::InvalidDeliveryDays);
        }
        Ok(Self { min, max })
    }

    /// Reads a form's two fields: both empty is `None`, one empty takes the
    /// other's number.
    pub fn parse(min: &str, max: &str) -> Result<Option<Self>, CatalogError> {
        let read = |field: &str| -> Result<Option<u16>, CatalogError> {
            match field.trim() {
                "" => Ok(None),
                text => text
                    .parse()
                    .map(Some)
                    .map_err(|_| CatalogError::InvalidDeliveryDays),
            }
        };
        match (read(min)?, read(max)?) {
            (None, None) => Ok(None),
            (Some(min), None) => Self::new(min, min).map(Some),
            (None, Some(max)) => Self::new(max, max).map(Some),
            (Some(min), Some(max)) => Self::new(min, max).map(Some),
        }
    }

    /// The window an order of several things arrives in: the latest of the
    /// earliest days and the latest of the latest ones. `None` when no line
    /// says.
    pub fn window(days: impl IntoIterator<Item = Option<Self>>) -> Option<Self> {
        days.into_iter().flatten().reduce(|a, b| Self {
            min: a.min.max(b.min),
            max: a.max.max(b.max),
        })
    }
}

/// How far back a reduction looks for the price it is measured from: the
/// lowest the thing was on sale at over these days before its current
/// price (article L. 112-1-1 of the French consumer code, from directive
/// (EU) 2019/2161).
pub const PRIOR_PRICE_DAYS: u64 = 30;

/// What a thing was on sale at, change after change: what a reduction may
/// be measured from. Only what shoppers could pay counts, so a draft leaves
/// no trace and an unpublished product a gap. Kept no further back than
/// [`PRIOR_PRICE_DAYS`] before the current price.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PriceTrail {
    points: Vec<PricePoint>,
}

/// A change in what a thing is on sale at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PricePoint {
    /// Unix seconds.
    pub at: u64,
    /// `None` while it is off sale.
    pub amount_cents: Option<u64>,
}

impl PriceTrail {
    /// Notes what the thing is on sale at from `at` on; `None` takes it
    /// off sale. The same as before changes nothing.
    pub fn record(&mut self, at: u64, amount_cents: Option<u64>) {
        let last = self.points.last().map(|point| point.amount_cents);
        if last == Some(amount_cents) || (last.is_none() && amount_cents.is_none()) {
            return;
        }
        self.points.push(PricePoint { at, amount_cents });
        // What ended before the window of the current price is of no use
        // to any later question.
        if let Some(began) = self.current_began() {
            let opens = self.points[began]
                .at
                .saturating_sub(PRIOR_PRICE_DAYS * 86_400);
            let keep = (0..began)
                .find(|at| self.points[at + 1].at >= opens)
                .unwrap_or(began);
            self.points.drain(..keep);
        }
    }

    /// The changes kept, oldest first.
    pub fn points(&self) -> &[PricePoint] {
        &self.points
    }

    /// Where the current price began: the first point of the run that ends
    /// the trail and holds nothing but that price and times off sale, so
    /// taking a product off sale and back does not make its price a new
    /// one. `None` for a thing that was never on sale.
    fn current_began(&self) -> Option<usize> {
        let current = self.points.iter().rev().find_map(|p| p.amount_cents)?;
        let run = self
            .points
            .iter()
            .rev()
            .take_while(|point| point.amount_cents.is_none_or(|amount| amount == current))
            .count();
        // The run may open on a time off sale: the price began after it.
        (self.points.len() - run..self.points.len())
            .find(|at| self.points[*at].amount_cents.is_some())
    }

    /// The lowest the thing was on sale at over the [`PRIOR_PRICE_DAYS`]
    /// before its current price began: the most a reduction may be measured
    /// from. `None` when it was on sale at nothing else over those days.
    pub fn lowest_before_current(&self) -> Option<u64> {
        let began = self.current_began()?;
        let opens = self.points[began]
            .at
            .saturating_sub(PRIOR_PRICE_DAYS * 86_400);
        (0..began)
            .filter(|at| self.points[at + 1].at >= opens)
            .filter_map(|at| self.points[at].amount_cents)
            .min()
    }
}

/// The stream of the shop's shopping feed: one per tenant store.
pub fn feed_id() -> String {
    evento::hash_ids(vec!["feed"])
}

/// A fresh address for a shopping feed: 32 random bytes in hexadecimal,
/// from the thread generator (seeded by the OS).
pub fn new_feed_token() -> String {
    use rand::RngCore;
    use std::fmt::Write;

    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    bytes
        .iter()
        .fold(String::with_capacity(64), |mut token, byte| {
            let _ = write!(token, "{byte:02x}");
            token
        })
}

/// The stream of what the shop pays for the things a product sells.
pub fn product_cost_id(product_id: &str) -> String {
    evento::hash_ids(vec!["product-cost", product_id])
}

/// The stream of what a sale costs the shop: one per tenant store.
pub fn sale_costs_id() -> String {
    evento::hash_ids(vec!["sale-costs"])
}

/// The stream of the shop's default delivery time: one per tenant store.
pub fn delivery_default_id() -> String {
    evento::hash_ids(vec!["delivery-default"])
}

/// What two codes are compared by: case and surrounding space do not make
/// two different codes.
pub fn sku_key(sku: &str) -> String {
    sku.trim().to_ascii_uppercase()
}

/// Trims a stock code and checks it is 1 to 64 of letters, digits, dot,
/// dash and underscore.
pub fn normalize_sku(sku: &str) -> Result<String, CatalogError> {
    let sku = sku.trim();
    let valid = (1..=MAX_SKU_LEN).contains(&sku.len())
        && sku
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'_');
    if valid {
        Ok(sku.to_owned())
    } else {
        Err(CatalogError::InvalidSku)
    }
}

/// Trims a label of an option or of one of its values and checks it is at
/// most 40 characters; an empty one is no label.
pub fn normalize_option_label(label: Option<&str>) -> Result<Option<String>, CatalogError> {
    let Some(label) = label.map(str::trim).filter(|label| !label.is_empty()) else {
        return Ok(None);
    };
    if label.chars().count() > MAX_OPTION_LABEL_LEN {
        return Err(CatalogError::InvalidOptionLabel);
    }
    Ok(Some(label.to_owned()))
}

/// Trims an option's name and checks it is 1 to 40 characters.
pub fn normalize_option_name(name: &str) -> Result<String, CatalogError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > MAX_OPTION_NAME_LEN {
        return Err(CatalogError::InvalidOptionName);
    }
    Ok(name.to_owned())
}

/// Trims an option's values, drops the empty ones and refuses two that
/// differ only in case. Order is the merchant's.
pub fn normalize_option_values(values: &[String]) -> Result<Vec<String>, CatalogError> {
    let mut out: Vec<String> = Vec::new();
    for value in values {
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        if value.chars().count() > MAX_OPTION_NAME_LEN {
            return Err(CatalogError::InvalidOptionValues);
        }
        if out.iter().any(|kept| kept.eq_ignore_ascii_case(value)) {
            continue;
        }
        out.push(value.to_owned());
    }
    if out.is_empty() || out.len() > MAX_OPTION_VALUES {
        return Err(CatalogError::InvalidOptionValues);
    }
    Ok(out)
}

/// Trims a fact's name and value and checks their lengths.
pub fn normalize_attribute(name: &str, value: &str) -> Result<(String, String), CatalogError> {
    let name = name.trim();
    let value = value.trim();
    if name.is_empty()
        || name.chars().count() > MAX_ATTRIBUTE_NAME_LEN
        || value.is_empty()
        || value.chars().count() > MAX_ATTRIBUTE_VALUE_LEN
    {
        return Err(CatalogError::InvalidAttribute);
    }
    Ok((name.to_owned(), value.to_owned()))
}

/// Trims a brand and checks it is at most 70 characters; an empty one is
/// no brand.
pub fn normalize_brand(brand: Option<&str>) -> Result<Option<String>, CatalogError> {
    let Some(brand) = brand.map(str::trim).filter(|brand| !brand.is_empty()) else {
        return Ok(None);
    };
    if brand.chars().count() > MAX_BRAND_LEN {
        return Err(CatalogError::InvalidBrand);
    }
    Ok(Some(brand.to_owned()))
}

/// A trade item number (the digits under a barcode) without the spaces and
/// dashes it is often written with: 8, 12, 13 or 14 digits, the last one
/// checking the others as GS1 reckons it. An empty one is none.
pub fn normalize_gtin(gtin: Option<&str>) -> Result<Option<String>, CatalogError> {
    let Some(gtin) = gtin.map(str::trim).filter(|gtin| !gtin.is_empty()) else {
        return Ok(None);
    };
    let digits: String = gtin.chars().filter(|c| *c != ' ' && *c != '-').collect();
    if !matches!(digits.len(), 8 | 12 | 13 | 14) || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(CatalogError::InvalidGtin);
    }
    // From the right, the check digit left out, the digits weigh 3 and 1 in
    // turn; the check digit brings their sum to a multiple of ten.
    let (body, check) = digits.as_bytes().split_at(digits.len() - 1);
    let sum: u32 = body
        .iter()
        .rev()
        .enumerate()
        .map(|(at, digit)| u32::from(digit - b'0') * if at % 2 == 0 { 3 } else { 1 })
        .sum();
    if (10 - sum % 10) % 10 != u32::from(check[0] - b'0') {
        return Err(CatalogError::InvalidGtin);
    }
    Ok(Some(digits))
}

/// Trims a manufacturer's part number and checks it is at most 70
/// characters; an empty one is none.
pub fn normalize_mpn(mpn: Option<&str>) -> Result<Option<String>, CatalogError> {
    let Some(mpn) = mpn.map(str::trim).filter(|mpn| !mpn.is_empty()) else {
        return Ok(None);
    };
    if mpn.chars().count() > MAX_MPN_LEN {
        return Err(CatalogError::InvalidMpn);
    }
    Ok(Some(mpn.to_owned()))
}

/// The five labels of a product, each trimmed and at most 100 characters;
/// those not given are empty, and a sixth is refused.
pub fn normalize_feed_labels<S: AsRef<str>>(
    labels: &[S],
) -> Result<[String; FEED_LABELS], CatalogError> {
    if labels.len() > FEED_LABELS {
        return Err(CatalogError::InvalidFeedLabel);
    }
    let mut out: [String; FEED_LABELS] = Default::default();
    for (slot, label) in out.iter_mut().zip(labels) {
        *slot = normalize_feed_label(label.as_ref())?;
    }
    Ok(out)
}

/// One label of a product, trimmed and at most 100 characters; empty is no
/// label.
pub fn normalize_feed_label(label: &str) -> Result<String, CatalogError> {
    let label = label.trim();
    if label.chars().count() > MAX_FEED_LABEL_LEN {
        return Err(CatalogError::InvalidFeedLabel);
    }
    Ok(label.to_owned())
}

/// Trims a category of Google's product taxonomy, its number or its path,
/// and checks its length; an empty one is none.
pub fn normalize_google_category(category: Option<&str>) -> Result<Option<String>, CatalogError> {
    let Some(category) = category.map(str::trim).filter(|c| !c.is_empty()) else {
        return Ok(None);
    };
    if category.chars().count() > MAX_GOOGLE_CATEGORY_LEN {
        return Err(CatalogError::InvalidGoogleCategory);
    }
    Ok(Some(category.to_owned()))
}

/// One key of a closed list (`AGE_GROUPS`, `GENDERS`, `OPTION_KINDS`), in
/// lowercase; an empty one is none and an unknown one is `None` inside.
pub fn known_key(list: &[(&str, &str)], key: Option<&str>) -> Option<Option<String>> {
    let Some(key) = key.map(str::trim).filter(|key| !key.is_empty()) else {
        return Some(None);
    };
    let key = key.to_ascii_lowercase();
    list.iter()
        .any(|(known, _)| *known == key)
        .then_some(Some(key))
}

/// The label of a key of a closed list, or the key itself if unknown.
pub fn key_label<'a>(list: &[(&'a str, &'a str)], key: &'a str) -> &'a str {
    list.iter()
        .find(|(known, _)| *known == key)
        .map_or(key, |(_, label)| label)
}

/// A fresh id for a variant or an option.
pub fn new_part_id() -> String {
    ulid::Ulid::generate().to_string()
}

/// A fresh id for a picture. It names the stored file and travels in its
/// address, so it is a ULID: unguessable, and sortable by when it was made.
pub fn new_image_id() -> String {
    ulid::Ulid::generate().to_string()
}

/// Trims the text describing a picture and checks it is at most 200
/// characters. Empty is fine: a picture that adds nothing to the name is
/// better left undescribed than described badly.
pub fn normalize_alt(alt: &str) -> Result<String, CatalogError> {
    let alt = alt.trim();
    if alt.chars().count() > MAX_ALT_LEN {
        return Err(CatalogError::InvalidAlt);
    }
    Ok(alt.to_owned())
}

/// The index stream of a product slug.
pub fn product_slug_id(slug: &str) -> String {
    evento::hash_ids(vec!["product-slug", slug])
}

/// The index stream of a category slug. Its own kind: a product and a
/// category may share a slug, they live under different paths.
pub fn category_slug_id(slug: &str) -> String {
    evento::hash_ids(vec!["category-slug", slug])
}

/// Trims a category name and checks it is 1 to 80 characters.
pub fn normalize_category_name(name: &str) -> Result<String, CatalogError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > MAX_CATEGORY_NAME_LEN {
        return Err(CatalogError::InvalidName);
    }
    Ok(name.to_owned())
}

/// Trims a category description and checks it is at most 2000 characters.
pub fn normalize_category_description(description: &str) -> Result<String, CatalogError> {
    let description = description.trim();
    if description.chars().count() > MAX_CATEGORY_DESCRIPTION_LEN {
        return Err(CatalogError::InvalidDescription);
    }
    Ok(description.to_owned())
}

/// Trims the text below a category's products and checks its length.
pub fn normalize_category_body(body: &str) -> Result<String, CatalogError> {
    let body = body.trim();
    if body.chars().count() > MAX_CATEGORY_BODY_LEN {
        return Err(CatalogError::InvalidDescription);
    }
    Ok(body.to_owned())
}

/// Trims a display name and checks it is 1 to 120 characters.
pub fn normalize_name(name: &str) -> Result<String, CatalogError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME_LEN {
        return Err(CatalogError::InvalidName);
    }
    Ok(name.to_owned())
}

/// Trims a description and checks it is at most 4000 characters. Empty is
/// fine: a product may have none.
pub fn normalize_description(description: &str) -> Result<String, CatalogError> {
    let description = description.trim();
    if description.chars().count() > MAX_DESCRIPTION_LEN {
        return Err(CatalogError::InvalidDescription);
    }
    Ok(description.to_owned())
}

/// Trims the search engines' title and description of a product or a
/// category and checks their length. Empty is fine: the name and the
/// description stand in.
pub fn normalize_seo(
    meta_title: &str,
    meta_description: &str,
) -> Result<(String, String), CatalogError> {
    timada_core::normalize_seo(meta_title, meta_description).ok_or(CatalogError::InvalidSeo)
}

/// Lowercases and validates a slug: `[a-z0-9]`, `-` inside, 2 to 64 characters.
pub fn normalize_slug(slug: &str) -> Result<String, CatalogError> {
    timada_core::normalize_slug(slug, MAX_SLUG_LEN).ok_or(CatalogError::InvalidSlug)
}

/// The slug a name gives: ASCII-transliterated, lowercased, runs of anything
/// else collapsed into one dash, cut to [`MAX_SLUG_LEN`]. A name with nothing
/// usable gives [`FALLBACK_SLUG`].
pub fn slugify(name: &str) -> String {
    slugify_or(name, FALLBACK_SLUG)
}

/// [`slugify`] with the fallback of the caller's kind of thing.
pub fn slugify_or(name: &str, fallback: &str) -> String {
    timada_core::slugify(name, MAX_SLUG_LEN, fallback)
}

/// `slug` with a short random suffix (`blue-mug-k3x9p`), kept within
/// [`MAX_SLUG_LEN`]. Used when the plain slug is taken.
pub fn suffixed_slug(slug: &str) -> String {
    timada_core::suffixed_slug(slug, MAX_SLUG_LEN)
}

/// Uppercases a currency code and checks it is one of [`CURRENCIES`].
pub fn normalize_currency(currency: &str) -> Result<String, CatalogError> {
    timada_core::normalize_currency(currency).ok_or(CatalogError::InvalidCurrency)
}

/// Parses an amount typed by a person into minor units: `12.50` and `12,5`
/// give `1250`, `12` gives `1200`. At most two decimals, no sign, at most
/// [`MAX_AMOUNT_CENTS`].
pub fn parse_amount(input: &str) -> Result<u64, CatalogError> {
    let input = input.trim().replace(',', ".");
    let (whole, fraction) = match input.split_once('.') {
        Some((whole, fraction)) => (whole, fraction),
        None => (input.as_str(), ""),
    };
    if whole.is_empty() && fraction.is_empty() || fraction.len() > 2 {
        return Err(CatalogError::InvalidAmount);
    }
    let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    if !digits(whole) || !digits(fraction) {
        return Err(CatalogError::InvalidAmount);
    }
    let whole: u64 = if whole.is_empty() {
        0
    } else {
        whole.parse().map_err(|_| CatalogError::InvalidAmount)?
    };
    let cents: u64 = match fraction.len() {
        0 => 0,
        1 => {
            fraction
                .parse::<u64>()
                .map_err(|_| CatalogError::InvalidAmount)?
                * 10
        }
        _ => fraction.parse().map_err(|_| CatalogError::InvalidAmount)?,
    };
    let amount = whole
        .checked_mul(100)
        .and_then(|w| w.checked_add(cents))
        .filter(|a| *a <= MAX_AMOUNT_CENTS)
        .ok_or(CatalogError::InvalidAmount)?;
    Ok(amount)
}

/// Minor units as a person reads them, locale-neutral: `1250`, `EUR` gives
/// `12.50 €`: the amount with the sign of its currency, locale-neutral.
pub fn format_money(amount_cents: u64, currency: &str) -> String {
    timada_core::format_money(amount_cents, currency)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: u64 = 86_400;

    fn trail(points: &[(u64, Option<u64>)]) -> PriceTrail {
        let mut trail = PriceTrail::default();
        for (day, amount) in points {
            trail.record(day * DAY, *amount);
        }
        trail
    }

    #[test]
    fn a_reduction_is_measured_from_the_lowest_price_of_the_thirty_days_before() {
        // Never on sale, or at one price only: nothing to compare with.
        assert_eq!(trail(&[]).lowest_before_current(), None);
        assert_eq!(trail(&[(100, Some(2000))]).lowest_before_current(), None);
        // 20 € for months, then 15 €: measured from 20 €.
        let lowered = trail(&[(100, Some(2000)), (200, Some(1500))]);
        assert_eq!(lowered.lowest_before_current(), Some(2000));
        // The lowest of the window, not the last: 20 €, 12 € for a week,
        // 18 €, then 15 €.
        let dipped = trail(&[
            (100, Some(2000)),
            (180, Some(1200)),
            (187, Some(1800)),
            (200, Some(1500)),
        ]);
        assert_eq!(dipped.lowest_before_current(), Some(1200));
        // A price that ended more than thirty days before does not count.
        let long_ago = trail(&[(100, Some(1200)), (150, Some(1800)), (200, Some(1500))]);
        assert_eq!(long_ago.lowest_before_current(), Some(1800));
        // One that ended on the thirtieth day does.
        let edge = trail(&[(100, Some(1200)), (170, Some(1800)), (200, Some(1500))]);
        assert_eq!(edge.lowest_before_current(), Some(1200));
        // Changed twice within a second, the first still was a price.
        let mut quick = PriceTrail::default();
        quick.record(5, Some(2000));
        quick.record(5, Some(1500));
        assert_eq!(quick.lowest_before_current(), Some(2000));
    }

    #[test]
    fn time_off_sale_is_no_price_and_does_not_make_the_price_a_new_one() {
        // Off sale and back at the same price: still measured from 20 €.
        let back = trail(&[
            (100, Some(2000)),
            (120, Some(1500)),
            (125, None),
            (128, Some(1500)),
        ]);
        assert_eq!(back.lowest_before_current(), Some(2000));
        // Off sale for two months, then back cheaper: 20 € was not a price
        // of the thirty days before.
        let gone = trail(&[(100, Some(2000)), (110, None), (200, Some(1500))]);
        assert_eq!(gone.lowest_before_current(), None);
        // Off sale for a week only: it was.
        let away = trail(&[(100, Some(2000)), (193, None), (200, Some(1500))]);
        assert_eq!(away.lowest_before_current(), Some(2000));
        // Off sale now: the price it had is still the current one.
        let off = trail(&[(100, Some(2000)), (120, Some(1500)), (125, None)]);
        assert_eq!(off.lowest_before_current(), Some(2000));
        // A draft leaves no trace.
        let mut draft = PriceTrail::default();
        draft.record(5, None);
        assert!(draft.points().is_empty());
    }

    #[test]
    fn a_trail_keeps_the_thirty_days_before_the_current_price_and_no_more() {
        let mut daily = PriceTrail::default();
        for day in 0..400 {
            daily.record(day * DAY, Some(1000 + day));
        }
        // The thirty days before the last change, the price in force when
        // they opened, and the current one.
        assert_eq!(daily.points().len(), 32);
        assert_eq!(daily.lowest_before_current(), Some(1000 + 368));
        // The same price again is no change.
        daily.record(500 * DAY, Some(1399));
        assert_eq!(daily.points().len(), 32);
    }

    #[test]
    fn slugs_derive_from_names() {
        assert_eq!(slugify("Blue Mug"), "blue-mug");
        assert_eq!(slugify("  Café — crème!  "), "cafe-creme");
        assert_eq!(slugify("!!"), FALLBACK_SLUG);
        assert_eq!(slugify(&"x".repeat(100)).len(), MAX_SLUG_LEN);
        let suffixed = suffixed_slug(&"y".repeat(MAX_SLUG_LEN));
        assert_eq!(suffixed.len(), MAX_SLUG_LEN);
        assert!(normalize_slug("Blue-Mug").is_ok());
        assert!(normalize_slug("-mug").is_err());
        assert!(normalize_slug("m").is_err());
    }

    #[test]
    fn amounts_parse_and_format() {
        assert_eq!(parse_amount("12.50").unwrap(), 1250);
        assert_eq!(parse_amount("12,5").unwrap(), 1250);
        assert_eq!(parse_amount("12").unwrap(), 1200);
        assert_eq!(parse_amount(".99").unwrap(), 99);
        assert_eq!(parse_amount("0").unwrap(), 0);
        for bad in ["", "abc", "12.345", "-1", "1e3", ".", "99999999999"] {
            assert!(parse_amount(bad).is_err(), "{bad:?}");
        }
        assert_eq!(format_money(1250, "EUR"), "12.50 €");
        assert_eq!(format_money(5, "USD"), "0.05 $");
        assert_eq!(normalize_currency(" eur ").unwrap(), "EUR");
        assert!(normalize_currency("XYZ").is_err());
    }
}
