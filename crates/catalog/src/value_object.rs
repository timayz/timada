//! Identifiers, limits, the slug rules, money parsing and formatting.

use crate::CatalogError;

pub const MAX_NAME_LEN: usize = 120;
pub const MAX_DESCRIPTION_LEN: usize = 4000;
pub use timada_core::MIN_SLUG_LEN;
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
/// The largest picture, in bytes.
pub const MAX_IMAGE_BYTES: u64 = 8 * 1024 * 1024;
pub const MAX_ALT_LEN: usize = 200;
/// The picture formats a browser shows everywhere, as `(extension, type)`.
/// The extension is what the stored file is named with.
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
/// How many combinations one product sells, at most.
pub const MAX_VARIANTS: usize = 100;
pub const MAX_SKU_LEN: usize = 64;
/// How many facts one product's table holds, at most.
pub const MAX_ATTRIBUTES: usize = 30;
pub const MAX_ATTRIBUTE_NAME_LEN: usize = 40;
pub const MAX_ATTRIBUTE_VALUE_LEN: usize = 400;

/// One option of a variant and the value it takes: `Colour` is `Beige`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, bitcode::Encode, bitcode::Decode)]
pub struct OptionSelection {
    pub option_id: String,
    pub value: String,
}

/// The index stream of a stock code.
pub fn product_sku_id(sku: &str) -> String {
    evento::hash_ids(vec!["product-sku", &sku_key(sku)])
}

/// The stock stream of a product without variants (`variant_id` `None`) or
/// of one of its variants.
pub fn stock_item_id(product_id: &str, variant_id: Option<&str>) -> String {
    evento::hash_ids(vec!["stock", product_id, variant_id.unwrap_or("")])
}

/// The most a shop counts of one thing.
pub const MAX_STOCK: u32 = 1_000_000;

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

/// A fresh id for a variant or an option.
pub fn new_part_id() -> String {
    ulid::Ulid::generate().to_string()
}

/// A fresh id for a picture. It names the stored file and travels in its
/// address, so it is a ULID: unguessable, and sortable by when it was made.
pub fn new_image_id() -> String {
    ulid::Ulid::generate().to_string()
}

/// The media type of a known extension.
pub fn image_mime(ext: &str) -> Option<&'static str> {
    IMAGE_FORMATS
        .iter()
        .find(|(known, _)| *known == ext)
        .map(|(_, mime)| *mime)
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
