//! What a shopping feed says of a shop's products, and the feed as Google
//! Merchant Center reads it.
//!
//! `catalog_feed_products` holds one row per product a feed carries
//! (published, priced, not taken out of the feed), with everything a feed
//! says of it but its stock, which moves too often to be copied there and is
//! read beside it. `catalog_feeds` says which tenant a feed's address
//! belongs to. Both are written by the read-model subscription.
//!
//! [`write_feed_items`] turns a product into the items of the XML a
//! [`FeedReader`] takes (Merchant Center's, which Meta's and TikTok's
//! catalogues read too, each with its own wording): one per combination on
//! sale, grouped by `item_group_id`, or one for a product sold as is. The
//! rows know nothing of their readers.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

use anyhow::Result;
use timada_core::db::{Connection, Db, Json, execute, fetch_optional};
use timada_core::format_amount;
use timada_core::xml::xml_text as xml;

use super::paths::CategoryPaths;
use crate::command::ProductState;
use crate::margin::{Margin, MarginTiers, ProductCosts, ShopCosts};
use crate::value_object::DeliveryDays;

/// A product as a feed carries it: one row of `catalog_feed_products`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FeedProduct {
    pub id: String,
    pub slug: String,
    /// The categories it is filed under, its main one first: what its
    /// address on the storefront is made of ([`CategoryPaths::product`]).
    #[serde(default)]
    pub categories: Vec<String>,
    pub name: String,
    pub description: String,
    pub brand: Option<String>,
    /// Of a product sold as is.
    pub gtin: Option<String>,
    pub mpn: Option<String>,
    pub no_identifier: bool,
    pub google_category: Option<String>,
    pub age_group: Option<String>,
    pub gender: Option<String>,
    /// The campaign labels, empty where there is none.
    pub labels: Vec<String>,
    pub ai_title: bool,
    pub ai_description: bool,
    /// Its pictures by id, in the order the merchant put them.
    pub images: Vec<String>,
    pub currency: String,
    /// The price of a product sold as is.
    pub amount_cents: Option<u64>,
    /// The price its reduction is announced against, when it has one.
    #[serde(default)]
    pub compare_at_cents: Option<u64>,
    /// The combinations on sale; empty for a product sold as is.
    pub variants: Vec<FeedVariant>,
    /// Its own delivery time; `None` is the shop's default.
    pub delivery_days: Option<(u16, u16)>,
    /// What kind of goods it is for VAT.
    #[serde(default)]
    pub tax_category: String,
    /// What the shop pays for the things it sells. No feed says it: the
    /// margin tier of a label is worked out from it ([`FeedMargin`]). Read
    /// from the cost rows, not kept in the product's.
    #[serde(skip)]
    pub costs: ProductCosts,
    /// The things of it the shop counts and has none left of: a variant's
    /// id, or `""` for a product sold as is. Read from the stock rows, not
    /// kept in the product's.
    #[serde(skip)]
    pub sold_out: BTreeSet<String>,
}

/// One combination on sale, as a feed carries it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FeedVariant {
    pub id: String,
    /// The values a shopper chose, as the shop shows them (`Sable / Large`);
    /// `None` when every option is hidden.
    pub label: Option<String>,
    pub amount_cents: u64,
    /// The price its reduction is announced against, when it has one.
    #[serde(default)]
    pub compare_at_cents: Option<u64>,
    pub sku: Option<String>,
    pub gtin: Option<String>,
    pub mpn: Option<String>,
    pub image_id: Option<String>,
    /// What it takes for each option a feed knows the kind of, as
    /// `(kind, value)`: `("color", "Sable")`. A hidden option's value is
    /// what the variant is, so it counts too.
    pub kinds: Vec<(String, String)>,
}

impl FeedProduct {
    /// What a feed says of the product, or `None` when no feed carries it.
    pub fn of(product: &ProductState) -> Option<Self> {
        if !product.is_in_feed() {
            return None;
        }
        Some(Self {
            id: product.id.clone(),
            slug: product.slug.clone(),
            categories: product
                .filed_under()
                .into_iter()
                .map(str::to_owned)
                .collect(),
            name: product.name.clone(),
            description: product.description.clone(),
            brand: product.brand.clone(),
            gtin: product.gtin.clone(),
            mpn: product.mpn.clone(),
            no_identifier: product.no_identifier,
            google_category: product.google_category.clone(),
            age_group: product.age_group.clone(),
            gender: product.gender.clone(),
            labels: product.feed_labels.to_vec(),
            ai_title: product.ai_title,
            ai_description: product.ai_description,
            images: product.images.iter().map(|i| i.id.clone()).collect(),
            currency: product.currency()?.to_owned(),
            amount_cents: product.price.as_ref().map(|price| price.amount_cents),
            compare_at_cents: product.compare_at(None),
            variants: product
                .sold_variants()
                .map(|variant| FeedVariant {
                    id: variant.id.clone(),
                    label: product.variant_label(variant),
                    amount_cents: variant.price.amount_cents,
                    compare_at_cents: product.compare_at(Some(&variant.id)),
                    sku: variant.sku.clone(),
                    gtin: variant.gtin.clone(),
                    mpn: variant.mpn.clone(),
                    image_id: variant.image_id.clone(),
                    kinds: variant
                        .selections
                        .iter()
                        .filter_map(|selection| {
                            let option = product.option(&selection.option_id)?;
                            let kind = option.kind.clone()?;
                            Some((kind, option.display_value(&selection.value).to_owned()))
                        })
                        .collect(),
                })
                .collect(),
            delivery_days: product.delivery_days.map(|days| (days.min, days.max)),
            tax_category: product.tax_category().to_owned(),
            costs: ProductCosts::default(),
            sold_out: BTreeSet::new(),
        })
    }
}

/// Writes, rewrites or drops the feed row of a product inside the
/// transaction that writes its other rows. A row that already says the same
/// is left alone: most events of a product in a feed (a hold, a category)
/// change nothing a feed says.
pub(crate) async fn settle_feed_product(
    tx: &Connection,
    tenant_id: &str,
    product: &ProductState,
) -> Result<()> {
    let wanted = FeedProduct::of(product)
        .map(|item| Json(item).to_text())
        .transpose()?;
    let was: Option<(String,)> = fetch_optional(
        tx,
        "SELECT item_json FROM catalog_feed_products WHERE tenant_id = ?1 AND id = ?2",
        (tenant_id, product.id.as_str()),
    )
    .await?;
    let was = was.map(|(json,)| json);
    if was == wanted {
        return Ok(());
    }
    match wanted {
        Some(json) => {
            execute(
                tx,
                "INSERT INTO catalog_feed_products (tenant_id, id, item_json)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT (tenant_id, id) DO UPDATE SET item_json = excluded.item_json",
                (tenant_id, product.id.as_str(), json.as_str()),
            )
            .await?;
        }
        None => {
            execute(
                tx,
                "DELETE FROM catalog_feed_products WHERE tenant_id = ?1 AND id = ?2",
                (tenant_id, product.id.as_str()),
            )
            .await?;
        }
    }
    Ok(())
}

/// The tenant whose feed answers at `token`, if one does.
pub async fn feed_tenant(db: &Db, token: &str) -> Result<Option<String>> {
    let row: Option<(String,)> = db
        .fetch_optional(
            "SELECT tenant_id FROM catalog_feeds WHERE token = ?1",
            (token,),
        )
        .await?;
    Ok(row.map(|(tenant_id,)| tenant_id))
}

/// How many products the tenant's feed carries.
pub async fn count_feed_products(db: &Db, tenant_id: &str) -> Result<i64> {
    let row: Option<(i64,)> = db
        .fetch_optional(
            "SELECT count(*) FROM catalog_feed_products WHERE tenant_id = ?1",
            (tenant_id,),
        )
        .await?;
    Ok(row.map_or(0, |(n,)| n))
}

/// What the tenant's feed says of one product; `None` when no feed
/// carries it. Without its stock, which this does not read.
pub async fn feed_product(db: &Db, tenant_id: &str, id: &str) -> Result<Option<FeedProduct>> {
    let row: Option<(Json<FeedProduct>,)> = db
        .fetch_optional(
            "SELECT item_json FROM catalog_feed_products WHERE tenant_id = ?1 AND id = ?2",
            (tenant_id, id),
        )
        .await?;
    Ok(row.map(|(json,)| json.0))
}

/// The next `limit` products of the tenant's feed after the product
/// `after`, in id order, each with what it has none left of. A feed is read
/// a page at a time, so a shop of any size is written without being held
/// in memory.
pub async fn feed_products(
    db: &Db,
    tenant_id: &str,
    after: Option<&str>,
    limit: i64,
) -> Result<Vec<FeedProduct>> {
    let after = after.unwrap_or("");
    let rows: Vec<(Json<FeedProduct>,)> =
        db.fetch_all(FEED_PAGE, (tenant_id, after, limit)).await?;
    let mut products: Vec<FeedProduct> = rows.into_iter().map(|(json,)| json.0).collect();
    let Some(last) = products.last().map(|product| product.id.clone()) else {
        return Ok(products);
    };
    // The stock of the page's products, over the same range of ids.
    let sold_out: Vec<(String, String)> = db
        .fetch_all(FEED_SOLD_OUT, (tenant_id, after, last.as_str()))
        .await?;
    for (product_id, variant_id) in sold_out {
        if let Some(product) = products.iter_mut().find(|p| p.id == product_id) {
            product.sold_out.insert(variant_id);
        }
    }
    // And what the shop pays for them, over the same range.
    let costs: Vec<(String, Json<ProductCosts>)> = db
        .fetch_all(FEED_COSTS, (tenant_id, after, last.as_str()))
        .await?;
    for (product_id, costs) in costs {
        if let Some(product) = products.iter_mut().find(|p| p.id == product_id) {
            product.costs = costs.0;
        }
    }
    Ok(products)
}

/// What the shop pays for the things a page's products sell, read off the
/// cost rows' key over the page's range of ids.
const FEED_COSTS: &str = "SELECT product_id, costs_json FROM catalog_product_costs
    WHERE tenant_id = ?1 AND product_id > ?2 AND product_id <= ?3";

/// Writes what the shop pays for the things a product sells, or drops the
/// row when it says nothing any more.
pub(crate) async fn settle_product_costs(
    db: &Db,
    tenant_id: &str,
    product_id: &str,
    costs: &ProductCosts,
) -> Result<()> {
    if costs.is_empty() {
        db.execute(
            "DELETE FROM catalog_product_costs WHERE tenant_id = ?1 AND product_id = ?2",
            (tenant_id, product_id),
        )
        .await?;
        return Ok(());
    }
    let json = Json(costs).to_text()?;
    db.execute(
        "INSERT INTO catalog_product_costs (tenant_id, product_id, costs_json)
         VALUES (?1, ?2, ?3)
         ON CONFLICT (tenant_id, product_id) DO UPDATE SET costs_json = excluded.costs_json",
        (tenant_id, product_id, json.as_str()),
    )
    .await?;
    Ok(())
}

/// What the read model has of the costs of one product; nothing said is
/// the default.
pub async fn product_costs(db: &Db, tenant_id: &str, product_id: &str) -> Result<ProductCosts> {
    let row: Option<(Json<ProductCosts>,)> = db
        .fetch_optional(
            "SELECT costs_json FROM catalog_product_costs
             WHERE tenant_id = ?1 AND product_id = ?2",
            (tenant_id, product_id),
        )
        .await?;
    Ok(row.map(|(json,)| json.0).unwrap_or_default())
}

/// A page of a tenant's feed, read off the table's key in its order.
const FEED_PAGE: &str = "SELECT item_json FROM catalog_feed_products
    WHERE tenant_id = ?1 AND id > ?2 ORDER BY id LIMIT ?3";

/// What a page's products have none left of, read off the stock's key over
/// the page's range of ids.
const FEED_SOLD_OUT: &str = "SELECT product_id, variant_id FROM catalog_stock
    WHERE tenant_id = ?1 AND product_id > ?2 AND product_id <= ?3
      AND tracked = 1 AND on_hand = 0";

/// Who reads a shop's products: each takes the same items, worded its way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedReader {
    /// Google Merchant Center.
    Google,
    /// A Meta catalogue (Facebook and Instagram ads, retargeting).
    Meta,
    /// A TikTok catalogue.
    Tiktok,
}

impl FeedReader {
    /// The longest title it takes.
    fn title_chars(self) -> usize {
        match self {
            Self::Google | Self::Tiktok => 150,
            Self::Meta => 200,
        }
    }

    /// The longest description it takes.
    fn description_chars(self) -> usize {
        match self {
            Self::Google | Self::Tiktok => 5000,
            Self::Meta => 9999,
        }
    }

    /// How it words whether an item can be bought.
    fn availability(self, sold_out: bool) -> &'static str {
        match (self, sold_out) {
            (Self::Google, false) => "in_stock",
            (Self::Google, true) => "out_of_stock",
            (Self::Meta | Self::Tiktok, false) => "in stock",
            (Self::Meta | Self::Tiktok, true) => "out of stock",
        }
    }
}

/// How many pictures an item carries beside its first.
const GOOGLE_MORE_IMAGES: usize = 10;
/// How many delivery prices an item carries.
const GOOGLE_SHIPPINGS: usize = 100;
/// What Merchant Center calls a text generative AI wrote.
const GOOGLE_AI_SOURCE: &str = "trained_algorithmic_media";

/// What delivering to one country costs, as a shop's delivery method says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedShipping {
    /// Two letters.
    pub country: String,
    /// The method's name.
    pub service: String,
    pub price_cents: u64,
    pub currency: String,
    /// The goods' worth from which it is free.
    pub free_above_cents: Option<u64>,
}

/// What a feed's writer needs to know of the shop, beside its products.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FeedShop {
    /// The shop's name.
    pub name: String,
    /// Where shoppers find the shop, without a trailing slash. A product's
    /// page is at its default address under it (`paths`), and opens on a
    /// variant at `?v={id}`.
    pub storefront_url: String,
    /// Where the shop's categories are, for a product's address.
    pub paths: CategoryPaths,
    /// The delivery prices, one a country a method names. A method that
    /// ships everywhere names none and is left to the account's own
    /// delivery settings: a feed gives a price per country.
    pub shippings: Vec<FeedShipping>,
    /// The delivery time of a product that gives none.
    pub delivery_days: Option<DeliveryDays>,
    /// How one of the labels grades what each item leaves, in a shop that
    /// asked for it.
    pub margin: Option<FeedMargin>,
}

/// What a feed's writer grades an item's margin with: the label that says
/// the tier in place of the merchant's own, the shop's costs of a sale, and
/// the VAT in a price.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FeedMargin {
    /// Which label carries the tier, from 0.
    pub label: usize,
    pub tiers: MarginTiers,
    pub costs: Option<ShopCosts>,
    /// The VAT rate of each kind of goods where the shop sells, in basis
    /// points; a kind it does not list carries none.
    pub vat_bps: BTreeMap<String, u32>,
}

impl FeedMargin {
    /// The tier of one of `product` (`variant_id` `None`) or of one of its
    /// variants, sold at `amount_cents`; `None` when its cost is not
    /// known.
    fn tier(
        &self,
        product: &FeedProduct,
        variant_id: Option<&str>,
        amount_cents: u64,
    ) -> Option<&'static str> {
        // A row written before it said its kind of goods is standard ones.
        let category = if product.tax_category.is_empty() {
            crate::TAX_CATEGORY_STANDARD
        } else {
            &product.tax_category
        };
        let vat_bps = self.vat_bps.get(category).copied().unwrap_or(0);
        let margin = Margin::of(
            amount_cents,
            &product.currency,
            vat_bps,
            product.costs.unit_costs(variant_id),
            self.costs.as_ref(),
        )?;
        Some(self.tiers.tier(&margin).as_str())
    }
}

/// The opening of Merchant Center's XML, up to the first item.
pub fn google_feed_head(shop: &FeedShop) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <rss version=\"2.0\" xmlns:g=\"http://base.google.com/ns/1.0\">\n<channel>\n\
         <title>{}</title>\n<link>{}</link>\n<description>{}</description>\n",
        xml(&shop.name),
        xml(&shop.storefront_url),
        xml(&shop.name),
    )
}

/// What closes Merchant Center's XML after the last item.
pub const GOOGLE_FEED_TAIL: &str = "</channel>\n</rss>\n";

/// [`write_feed_items`] for Merchant Center.
pub fn write_google_items(
    out: &mut String,
    product: &FeedProduct,
    shop: &FeedShop,
    image_url: impl Fn(&str) -> String,
) {
    write_feed_items(out, FeedReader::Google, product, shop, image_url);
}

/// Writes the items of one product as `reader` takes them: one per
/// combination on sale, or one for a product sold as is. `image_url` gives
/// the address of a picture in the size a feed wants (the large rendition:
/// Merchant Center refuses small ones).
///
/// An item's id is the variant's, or the product's when it is sold as is:
/// the id the storefront's Google tag and Meta pixel name it by, so an ad
/// platform matches what a visitor saw to its catalogue.
pub fn write_feed_items(
    out: &mut String,
    reader: FeedReader,
    product: &FeedProduct,
    shop: &FeedShop,
    image_url: impl Fn(&str) -> String,
) {
    let page = format!(
        "{}{}",
        shop.storefront_url,
        shop.paths.product(&product.categories, &product.slug)
    );
    if product.variants.is_empty() {
        let Some(amount_cents) = product.amount_cents else {
            return;
        };
        write_item(
            out,
            reader,
            product,
            shop,
            &image_url,
            &Item {
                id: &product.id,
                grouped: false,
                label: None,
                link: page,
                amount_cents,
                compare_at_cents: product.compare_at_cents,
                image_id: None,
                gtin: product.gtin.as_deref(),
                mpn: product.mpn.as_deref(),
                kinds: &[],
                sold_out: product.sold_out.contains(""),
            },
        );
        return;
    }
    for variant in &product.variants {
        // The merchant's own code stands in for the maker's part number
        // where a brand is all the product is known by.
        let stand_in = (product.brand.is_some() && variant.gtin.is_none())
            .then_some(variant.sku.as_deref())
            .flatten();
        write_item(
            out,
            reader,
            product,
            shop,
            &image_url,
            &Item {
                id: &variant.id,
                grouped: true,
                label: variant.label.as_deref(),
                link: format!("{page}?v={}", variant.id),
                amount_cents: variant.amount_cents,
                compare_at_cents: variant.compare_at_cents,
                image_id: variant.image_id.as_deref(),
                gtin: variant.gtin.as_deref(),
                mpn: variant.mpn.as_deref().or(stand_in),
                kinds: &variant.kinds,
                sold_out: product.sold_out.contains(&variant.id),
            },
        );
    }
}

/// What differs between the items of one product.
struct Item<'a> {
    id: &'a str,
    /// One of several items of the product.
    grouped: bool,
    label: Option<&'a str>,
    link: String,
    amount_cents: u64,
    compare_at_cents: Option<u64>,
    /// The picture shown first, when it has one of its own.
    image_id: Option<&'a str>,
    gtin: Option<&'a str>,
    mpn: Option<&'a str>,
    kinds: &'a [(String, String)],
    sold_out: bool,
}

fn write_item(
    out: &mut String,
    reader: FeedReader,
    product: &FeedProduct,
    shop: &FeedShop,
    image_url: &impl Fn(&str) -> String,
    item: &Item<'_>,
) {
    out.push_str("<item>\n");
    // TikTok names an item's id `sku_id`; its reader of this XML is said to
    // take `id` for it too, so both are written.
    if reader == FeedReader::Tiktok {
        tag(out, "sku_id", item.id);
    }
    tag(out, "id", item.id);
    if item.grouped {
        tag(out, "item_group_id", &product.id);
    }
    let title = match item.label {
        Some(label) => format!("{} – {label}", product.name),
        None => product.name.clone(),
    };
    // Only Merchant Center has a way to say a machine wrote a text.
    let says_ai = reader == FeedReader::Google;
    text(
        out,
        "title",
        &title,
        reader.title_chars(),
        says_ai && product.ai_title,
    );
    // Merchant Center wants a description: a product without one gives its
    // name again.
    let (description, by_ai) = if product.description.trim().is_empty() {
        (title.as_str(), product.ai_title)
    } else {
        (product.description.as_str(), product.ai_description)
    };
    text(
        out,
        "description",
        description,
        reader.description_chars(),
        says_ai && by_ai,
    );
    tag(out, "link", &item.link);
    // Its own picture first, then the product's others.
    let first = item
        .image_id
        .filter(|id| product.images.iter().any(|image| image == id))
        .or(product.images.first().map(String::as_str));
    if let Some(first) = first {
        tag(out, "image_link", &image_url(first));
        for image in product
            .images
            .iter()
            .filter(|image| image.as_str() != first)
            .take(GOOGLE_MORE_IMAGES)
        {
            tag(out, "additional_image_link", &image_url(image));
        }
    }
    tag(out, "availability", reader.availability(item.sold_out));
    // A reduction: the price it is down from is the price, and what a
    // shopper pays the sale price.
    match item.compare_at_cents {
        Some(compare_at) => {
            tag(out, "price", &money(compare_at, &product.currency));
            tag(
                out,
                "sale_price",
                &money(item.amount_cents, &product.currency),
            );
        }
        None => tag(out, "price", &money(item.amount_cents, &product.currency)),
    }
    tag(out, "condition", "new");
    match (&product.brand, reader) {
        (Some(brand), _) => tag(out, "brand", brand),
        (None, FeedReader::Google) => {}
        // Meta and TikTok refuse an item without a brand: the shop sells it
        // under its own name.
        (None, FeedReader::Meta | FeedReader::Tiktok) => tag(out, "brand", &shop.name),
    }
    if product.no_identifier {
        // Merchant Center's own way to say there is no number to give.
        if reader == FeedReader::Google {
            tag(out, "identifier_exists", "no");
        }
    } else {
        if let Some(gtin) = item.gtin {
            tag(out, "gtin", gtin);
        }
        if let Some(mpn) = item.mpn {
            tag(out, "mpn", mpn);
        }
    }
    if let Some(category) = &product.google_category {
        tag(out, "google_product_category", category);
    }
    if let Some(age_group) = &product.age_group {
        tag(out, "age_group", age_group);
    }
    if let Some(gender) = &product.gender {
        tag(out, "gender", gender);
    }
    for (kind, value) in item.kinds {
        tag(out, kind, value);
    }
    for (n, label) in product.labels.iter().enumerate() {
        // The label a shop gave to its margins says the item's tier, and
        // nothing where its cost is not known: never the merchant's own
        // label, which a campaign would take for a tier.
        let label = match &shop.margin {
            Some(margin) if margin.label == n => margin
                .tier(product, item.grouped.then_some(item.id), item.amount_cents)
                .unwrap_or(""),
            _ => label,
        };
        if !label.is_empty() {
            tag(out, &format!("custom_label_{n}"), label);
        }
    }
    let days = product
        .delivery_days
        .or(shop.delivery_days.map(|days| (days.min, days.max)));
    let shippings = shop
        .shippings
        .iter()
        .filter(|shipping| shipping.currency == product.currency)
        .take(GOOGLE_SHIPPINGS)
        .map(|shipping| {
            let free = shipping
                .free_above_cents
                .is_some_and(|threshold| item.amount_cents >= threshold);
            (shipping, if free { 0 } else { shipping.price_cents })
        });
    match reader {
        FeedReader::Google => {}
        // Meta takes the prices as one text, `FR::Colissimo:4.90 EUR`, a
        // comma between two.
        FeedReader::Meta => {
            let plain = |text: &str| text.replace([':', ','], " ");
            let prices: Vec<String> = shippings
                .map(|(shipping, cents)| {
                    format!(
                        "{}::{}:{}",
                        shipping.country,
                        plain(&shipping.service),
                        money(cents, &shipping.currency)
                    )
                })
                .collect();
            if !prices.is_empty() {
                tag(out, "shipping", &prices.join(","));
            }
            out.push_str("</item>\n");
            return;
        }
        // TikTok's delivery prices are set on the catalogue itself.
        FeedReader::Tiktok => {
            out.push_str("</item>\n");
            return;
        }
    }
    for (shipping, cents) in shippings {
        out.push_str("<g:shipping>\n");
        tag(out, "country", &shipping.country);
        tag(out, "service", &shipping.service);
        tag(out, "price", &money(cents, &shipping.currency));
        // The shop's delivery time runs from the order to the door: no
        // handling counted apart from it.
        if let Some((min, max)) = days {
            tag(out, "min_handling_time", "0");
            tag(out, "max_handling_time", "0");
            tag(out, "min_transit_time", &min.to_string());
            tag(out, "max_transit_time", &max.to_string());
        }
        out.push_str("</g:shipping>\n");
    }
    out.push_str("</item>\n");
}

/// `12.50 EUR`, as a feed writes an amount.
fn money(cents: u64, currency: &str) -> String {
    format!("{} {currency}", format_amount(cents))
}

/// `<g:name>value</g:name>`, the value escaped.
fn tag(out: &mut String, name: &str, value: &str) {
    let _ = writeln!(out, "<g:{name}>{}</g:{name}>", xml(value));
}

/// A title or a description, cut to what Merchant Center takes: as the
/// plain attribute, or as the structured one that says a machine wrote it.
fn text(out: &mut String, name: &str, value: &str, max: usize, by_ai: bool) {
    let value: String = value.chars().take(max).collect();
    if by_ai {
        let _ = writeln!(
            out,
            "<g:structured_{name}>\n<g:digital_source_type>{GOOGLE_AI_SOURCE}</g:digital_source_type>\n\
             <g:content>{}</g:content>\n</g:structured_{name}>",
            xml(&value)
        );
    } else {
        tag(out, name, &value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mug() -> FeedProduct {
        FeedProduct {
            id: "01MUG".into(),
            slug: "blue-mug".into(),
            categories: Vec::new(),
            name: "Blue Mug".into(),
            description: "Holds 50 cl & more.".into(),
            brand: Some("Acme".into()),
            gtin: Some("4006381333931".into()),
            mpn: Some("MUG-01".into()),
            no_identifier: false,
            google_category: Some("Home & Garden > Kitchen".into()),
            age_group: Some("adult".into()),
            gender: Some("unisex".into()),
            labels: vec!["high margin".into(), String::new(), "eu".into()],
            ai_title: false,
            ai_description: false,
            images: vec!["a".into(), "b".into()],
            currency: "EUR".into(),
            amount_cents: Some(1250),
            compare_at_cents: None,
            variants: Vec::new(),
            delivery_days: None,
            tax_category: "standard".into(),
            costs: ProductCosts::default(),
            sold_out: BTreeSet::new(),
        }
    }

    fn variant(id: &str, amount_cents: u64) -> FeedVariant {
        FeedVariant {
            id: id.into(),
            label: None,
            amount_cents,
            compare_at_cents: None,
            sku: None,
            gtin: None,
            mpn: None,
            image_id: None,
            kinds: Vec::new(),
        }
    }

    fn shop() -> FeedShop {
        FeedShop {
            name: "Acme".into(),
            storefront_url: "https://shop.example".into(),
            paths: CategoryPaths::default(),
            shippings: vec![FeedShipping {
                country: "FR".into(),
                service: "Colissimo".into(),
                price_cents: 490,
                currency: "EUR".into(),
                free_above_cents: Some(5000),
            }],
            delivery_days: Some(DeliveryDays { min: 2, max: 4 }),
            margin: None,
        }
    }

    fn items(product: &FeedProduct, shop: &FeedShop) -> String {
        let mut out = String::new();
        write_google_items(&mut out, product, shop, |id| {
            format!("https://media.example/1200/{id}.webp")
        });
        out
    }

    /// A feed is read by a shop's every product: each page must be a walk
    /// of a key, never a scan of every shop's rows nor a sort.
    #[tokio::test]
    async fn a_page_is_read_off_a_key() {
        let store = evento::Turso::in_memory_concurrent().await.unwrap();
        let mut conn = store.connect().unwrap();
        crate::MIGRATIONS
            .to_latest(&mut conn)
            .await
            .map_err(|(_, err)| err)
            .unwrap();
        let db = Db::from(store);
        for (sql, params) in [
            (
                FEED_PAGE,
                timada_core::db::params_from_iter(vec![
                    timada_core::db::Value::from("acme"),
                    timada_core::db::Value::from(""),
                    timada_core::db::Value::from(500_i64),
                ]),
            ),
            (
                FEED_SOLD_OUT,
                timada_core::db::params_from_iter(vec![
                    timada_core::db::Value::from("acme"),
                    timada_core::db::Value::from(""),
                    timada_core::db::Value::from("z"),
                ]),
            ),
        ] {
            let plan: Vec<(i64, i64, i64, String)> = db
                .fetch_all(&format!("EXPLAIN QUERY PLAN {sql}"), params)
                .await
                .unwrap();
            let plan: Vec<String> = plan.into_iter().map(|(_, _, _, detail)| detail).collect();
            assert!(
                plan.iter().any(|step| step.contains("SEARCH"))
                    && !plan
                        .iter()
                        .any(|step| step.contains("SCAN") || step.contains("B-TREE")),
                "{sql}\n{}",
                plan.join("\n")
            );
        }
    }

    #[test]
    fn an_item_links_the_product_under_its_main_category() {
        let shop = FeedShop {
            paths: CategoryPaths::of(&[
                ("c1".into(), "kitchen".into(), None),
                ("c2".into(), "mugs".into(), Some("c1".into())),
            ]),
            ..shop()
        };
        let filed = FeedProduct {
            categories: vec!["c2".into(), "c1".into()],
            ..mug()
        };
        assert!(
            items(&filed, &shop)
                .contains("<g:link>https://shop.example/kitchen/mugs/blue-mug</g:link>")
        );
        // Filed nowhere, it keeps its own route.
        assert!(items(&mug(), &shop).contains("<g:link>https://shop.example/p/blue-mug</g:link>"));
    }

    #[test]
    fn a_product_sold_as_is_is_one_item() {
        assert_eq!(
            items(&mug(), &shop()),
            "<item>\n\
             <g:id>01MUG</g:id>\n\
             <g:title>Blue Mug</g:title>\n\
             <g:description>Holds 50 cl &amp; more.</g:description>\n\
             <g:link>https://shop.example/p/blue-mug</g:link>\n\
             <g:image_link>https://media.example/1200/a.webp</g:image_link>\n\
             <g:additional_image_link>https://media.example/1200/b.webp</g:additional_image_link>\n\
             <g:availability>in_stock</g:availability>\n\
             <g:price>12.50 EUR</g:price>\n\
             <g:condition>new</g:condition>\n\
             <g:brand>Acme</g:brand>\n\
             <g:gtin>4006381333931</g:gtin>\n\
             <g:mpn>MUG-01</g:mpn>\n\
             <g:google_product_category>Home &amp; Garden &gt; Kitchen</g:google_product_category>\n\
             <g:age_group>adult</g:age_group>\n\
             <g:gender>unisex</g:gender>\n\
             <g:custom_label_0>high margin</g:custom_label_0>\n\
             <g:custom_label_2>eu</g:custom_label_2>\n\
             <g:shipping>\n\
             <g:country>FR</g:country>\n\
             <g:service>Colissimo</g:service>\n\
             <g:price>4.90 EUR</g:price>\n\
             <g:min_handling_time>0</g:min_handling_time>\n\
             <g:max_handling_time>0</g:max_handling_time>\n\
             <g:min_transit_time>2</g:min_transit_time>\n\
             <g:max_transit_time>4</g:max_transit_time>\n\
             </g:shipping>\n\
             </item>\n"
        );
    }

    #[test]
    fn a_variant_is_an_item_of_its_group() {
        let mut harness = mug();
        harness.gtin = None;
        harness.mpn = None;
        harness.amount_cents = None;
        harness.delivery_days = Some((5, 9));
        harness.variants = vec![
            FeedVariant {
                id: "v1".into(),
                label: Some("Sable / S".into()),
                amount_cents: 2290,
                compare_at_cents: Some(2990),
                sku: Some("H-SABLE-S".into()),
                gtin: Some("036000291452".into()),
                mpn: None,
                image_id: Some("b".into()),
                kinds: vec![
                    ("color".into(), "Sable".into()),
                    ("size".into(), "S".into()),
                ],
            },
            FeedVariant {
                id: "v2".into(),
                label: None,
                amount_cents: 6000,
                compare_at_cents: None,
                sku: Some("H-BLUE".into()),
                gtin: None,
                mpn: None,
                image_id: None,
                kinds: Vec::new(),
            },
        ];
        harness.sold_out.insert("v2".into());
        let out = items(&harness, &shop());
        let (first, second) = out.split_once("</item>\n").unwrap();
        assert!(first.contains("<g:id>v1</g:id>\n<g:item_group_id>01MUG</g:item_group_id>\n"));
        assert!(first.contains("<g:title>Blue Mug – Sable / S</g:title>"));
        assert!(first.contains("<g:link>https://shop.example/p/blue-mug?v=v1</g:link>"));
        // Its own picture first, the product's other one after.
        assert!(first.contains(
            "<g:image_link>https://media.example/1200/b.webp</g:image_link>\n\
             <g:additional_image_link>https://media.example/1200/a.webp</g:additional_image_link>"
        ));
        assert!(first.contains("<g:availability>in_stock</g:availability>"));
        // Down from 29.90 €: that is the price, and 22.90 € the sale price.
        assert!(
            first.contains("<g:price>29.90 EUR</g:price>\n<g:sale_price>22.90 EUR</g:sale_price>")
        );
        // A GTIN is enough: the merchant's code is not passed off as the
        // maker's.
        assert!(first.contains("<g:gtin>036000291452</g:gtin>") && !first.contains("<g:mpn>"));
        assert!(first.contains("<g:color>Sable</g:color>\n<g:size>S</g:size>"));
        // The product's own delivery time, not the shop's.
        assert!(first.contains("<g:min_transit_time>5</g:min_transit_time>"));
        assert!(first.contains("<g:price>4.90 EUR</g:price>"));

        assert!(second.contains("<g:title>Blue Mug</g:title>"));
        assert!(second.contains("<g:availability>out_of_stock</g:availability>"));
        assert!(second.contains("<g:price>60.00 EUR</g:price>") && !second.contains("sale_price"));
        assert!(second.contains("<g:mpn>H-BLUE</g:mpn>"), "{second}");
        // Free from 50 €.
        assert!(second.contains("<g:service>Colissimo</g:service>\n<g:price>0.00 EUR</g:price>"));
    }

    #[test]
    fn what_a_machine_wrote_says_so() {
        let mut mug = mug();
        mug.ai_title = true;
        mug.ai_description = true;
        let out = items(&mug, &shop());
        assert!(out.contains(
            "<g:structured_title>\n\
             <g:digital_source_type>trained_algorithmic_media</g:digital_source_type>\n\
             <g:content>Blue Mug</g:content>\n\
             </g:structured_title>"
        ));
        assert!(out.contains("<g:structured_description>"));
        assert!(!out.contains("<g:title>") && !out.contains("<g:description>"));
    }

    #[test]
    fn a_product_without_identifier_says_so_and_gives_none() {
        let mut mug = mug();
        mug.no_identifier = true;
        mug.gtin = None;
        mug.mpn = None;
        let out = items(&mug, &shop());
        assert!(out.contains("<g:identifier_exists>no</g:identifier_exists>"));
        assert!(!out.contains("<g:gtin>") && !out.contains("<g:mpn>"));
        assert!(out.contains("<g:brand>Acme</g:brand>"));
    }

    #[test]
    fn what_the_shop_does_not_know_is_left_out() {
        let mut mug = mug();
        mug.description = " ".into();
        mug.images.clear();
        mug.brand = None;
        let mut shop = shop();
        shop.delivery_days = None;
        shop.shippings[0].currency = "USD".into();
        let out = items(&mug, &shop);
        assert!(out.contains("<g:description>Blue Mug</g:description>"));
        assert!(!out.contains("image_link") && !out.contains("<g:brand>"));
        assert!(
            !out.contains("<g:shipping>"),
            "a price in another currency is none"
        );
        shop.shippings[0].currency = "EUR".into();
        let out = items(&mug, &shop);
        assert!(out.contains("<g:shipping>") && !out.contains("transit_time"));
    }

    #[test]
    fn text_is_cut_and_cleaned_for_xml() {
        let mut mug = mug();
        mug.name = format!("{}\u{1}<b>", "é".repeat(200));
        let out = items(&mug, &shop());
        assert!(out.contains(&format!("<g:title>{}</g:title>", "é".repeat(150))));
        assert_eq!(xml("a\u{0}b\u{B}c\n<&>\""), "abc\n&lt;&amp;&gt;&quot;");
        let head = google_feed_head(&FeedShop {
            name: "Tom & Co".into(),
            ..shop()
        });
        assert!(head.contains("<title>Tom &amp; Co</title>"));
        assert!(head.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
    }

    fn items_for(reader: FeedReader, product: &FeedProduct, shop: &FeedShop) -> String {
        let mut out = String::new();
        write_feed_items(&mut out, reader, product, shop, |id| {
            format!("https://media.example/1200/{id}.webp")
        });
        out
    }

    #[test]
    fn meta_reads_the_same_item_in_its_own_words() {
        let mut product = mug();
        product.ai_title = true;
        product.no_identifier = true;
        product.sold_out.insert(String::new());
        let xml = items_for(FeedReader::Meta, &product, &shop());
        assert!(xml.contains("<g:id>01MUG</g:id>"), "{xml}");
        assert!(!xml.contains("sku_id"));
        assert!(xml.contains("<g:availability>out of stock</g:availability>"));
        // No structured title, no `identifier_exists`: Merchant Center's own.
        assert!(xml.contains("<g:title>Blue Mug</g:title>"));
        assert!(!xml.contains("structured_title"));
        assert!(!xml.contains("identifier_exists"));
        assert!(xml.contains("<g:price>12.50 EUR</g:price>"));
        assert!(xml.contains("<g:shipping>FR::Colissimo:4.90 EUR</g:shipping>"));
        assert!(!xml.contains("transit_time"));

        // Free delivery above a threshold the item reaches, and a method's
        // name that would break the text.
        let mut shop = shop();
        shop.shippings[0].free_above_cents = Some(1000);
        shop.shippings[0].service = "Colissimo: 48h, suivi".into();
        let xml = items_for(FeedReader::Meta, &mug(), &shop);
        assert!(
            xml.contains("<g:shipping>FR::Colissimo  48h  suivi:0.00 EUR</g:shipping>"),
            "{xml}"
        );
    }

    #[test]
    fn tiktok_gets_its_id_and_a_brand_whatever_the_product_says() {
        let mut product = mug();
        product.brand = None;
        let xml = items_for(FeedReader::Tiktok, &product, &shop());
        assert!(xml.contains("<g:sku_id>01MUG</g:sku_id>"), "{xml}");
        assert!(xml.contains("<g:id>01MUG</g:id>"));
        assert!(xml.contains("<g:availability>in stock</g:availability>"));
        // The shop's name stands in for a brand the product does not give.
        assert!(xml.contains("<g:brand>Acme</g:brand>"));
        assert!(!xml.contains("<g:shipping>"));
        assert!(xml.ends_with("</item>\n"));
        // Merchant Center takes an item without a brand as it is.
        let xml = items_for(FeedReader::Google, &product, &shop());
        assert!(!xml.contains("<g:brand>"));
        assert!(xml.contains("<g:availability>in_stock</g:availability>"));
    }

    /// A shop that gave a label to its margins has it say each item's
    /// tier, worked out from the item's own price and cost; an item whose
    /// cost is not known says nothing there, not even the merchant's label.
    #[test]
    fn a_label_given_to_the_margins_says_each_item_s_tier() {
        let mut shop = shop();
        shop.margin = Some(FeedMargin {
            label: 0,
            tiers: MarginTiers::default(),
            costs: Some(ShopCosts {
                currency: "EUR".into(),
                shipping_cents: 100,
                ..ShopCosts::default()
            }),
            vat_bps: BTreeMap::from([("standard".to_owned(), 2_000)]),
        });
        let mut product = mug();
        // 12.50 with 2.08 of VAT, bought at 4.00 and sent for 1.00: 5.42
        // left, more than two fifths of the price.
        product.costs.cost_cents = Some(400);
        let out = items(&product, &shop);
        assert!(
            out.contains("<g:custom_label_0>high</g:custom_label_0>"),
            "{out}"
        );
        assert!(
            out.contains("<g:custom_label_2>eu</g:custom_label_2>"),
            "{out}"
        );

        product.costs.cost_cents = Some(1_100);
        let out = items(&product, &shop);
        assert!(
            out.contains("<g:custom_label_0>loss</g:custom_label_0>"),
            "{out}"
        );

        product.costs.cost_cents = None;
        let out = items(&product, &shop);
        assert!(!out.contains("custom_label_0"), "{out}");

        // Each variant by its own cost: one the shop said nothing of says
        // nothing.
        product.variants = vec![variant("01BEIGE", 1250), variant("01BLUE", 1250)];
        product.costs.variants.insert("01BEIGE".into(), 400);
        let out = items(&product, &shop);
        let (beige, blue) = out.split_once("<g:id>01BLUE</g:id>").unwrap();
        assert!(
            beige.contains("<g:custom_label_0>high</g:custom_label_0>"),
            "{out}"
        );
        assert!(!blue.contains("custom_label_0"), "{out}");
        product.variants.clear();

        // Without it the label is the merchant's.
        shop.margin = None;
        let out = items(&product, &shop);
        assert!(
            out.contains("<g:custom_label_0>high margin</g:custom_label_0>"),
            "{out}"
        );
    }
}
