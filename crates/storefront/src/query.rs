//! What a storefront page reads: the published products of the local copy.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashSet};

use timada_catalog::{DeliveryDays, TreeNode, TreeOf, tree_of};
use timada_core::db::{Db, FromRow, FromValue, Json, Row, Value, fts_query, params_from_iter};
use timada_core::like_contains;
use timada_core::{Picture, Rendition};

use crate::money::MoneyFormat;
use crate::read_model::{OptionRow, WHOLE_SHOP, distinct_option_rows};

/// A picture of a product, in the two sizes the store keeps it
/// ([`timada_core::Rendition`]): the page rendition (`url`, `width`,
/// `height`; 1200 px on its longest side at most) and the listing one
/// (`thumb_url`, `thumb_width`, `thumb_height`; 400 px). Both are WebP.
/// The addresses are absolute once the host set
/// [`StoreConfig::media_url`](crate::StoreConfig::media_url), and the bare
/// paths below it (`400/<id>.webp`) otherwise.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq, Hash)]
pub struct Image {
    pub id: String,
    /// What the picture shows; may be empty.
    pub alt: String,
    /// Of the page rendition, in pixels.
    pub width: u32,
    pub height: u32,
    /// Of the listing rendition, in pixels.
    pub thumb_width: u32,
    pub thumb_height: u32,
    /// Filled in from the store's media address when the product is read.
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub thumb_url: String,
}

impl Image {
    /// Points the picture at `base`, the address the store's pictures are
    /// served from; without one it keeps the bare paths.
    pub fn fill_urls(&mut self, base: Option<&str>) {
        self.url = rendition_url(base, &self.id, Rendition::Page);
        self.thumb_url = rendition_url(base, &self.id, Rendition::Listing);
    }
}

/// The address of one rendition of a picture under `base`, or its bare path
/// without one.
pub fn rendition_url(base: Option<&str>, image_id: &str, rendition: Rendition) -> String {
    match base {
        Some(base) => Picture::url(base, image_id, rendition),
        None => Picture::file(image_id, rendition),
    }
}

/// A way a product varies, with the values it takes.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq, Hash)]
pub struct ProductOption {
    pub id: String,
    pub name: String,
    pub values: Vec<String>,
    /// What a shopper reads instead of the name, when the merchant set one.
    #[serde(default)]
    pub label: Option<String>,
    /// What a shopper reads instead of a value, for the values the merchant
    /// labelled.
    #[serde(default)]
    pub value_labels: BTreeMap<String, String>,
}

impl ProductOption {
    /// What a shopper reads for the option: its label, else its name.
    pub fn display_name(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.name)
    }

    /// What a shopper reads for one of its values: the value's label, else
    /// the value.
    pub fn display_value<'a>(&'a self, value: &'a str) -> &'a str {
        self.value_labels.get(value).map_or(value, String::as_str)
    }
}

/// One combination a shopper buys, with its own price.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq, Hash)]
pub struct Variant {
    pub id: String,
    /// A value for every option of the product, in the options' order.
    pub selections: Vec<Selection>,
    /// The merchant's own code for it.
    pub sku: Option<String>,
    pub amount_cents: i64,
    pub currency: String,
    /// One of the product's pictures, shown when this variant is picked.
    pub image_id: Option<String>,
}

impl Variant {
    /// The price as a person reads it (`22.90 €`).
    pub fn price(&self) -> String {
        self.price_with(&MoneyFormat::default())
    }

    /// The price as `money` writes it ([`money`](crate::money)).
    pub fn price_with(&self, money: &MoneyFormat) -> String {
        money.format(self.amount_cents.max(0) as u64, &self.currency)
    }

    /// The value it takes for an option, if it names one.
    pub fn value_of(&self, option_id: &str) -> Option<&str> {
        self.selections
            .iter()
            .find(|s| s.option_id == option_id)
            .map(|s| s.value.as_str())
    }

    /// Whether this is the variant those `(option id, value)` pairs name,
    /// whatever order they come in.
    pub fn matches(&self, picked: &[(String, String)]) -> bool {
        self.selections.len() == picked.len()
            && self.selections.iter().all(|mine| {
                picked
                    .iter()
                    .any(|(option_id, value)| *option_id == mine.option_id && *value == mine.value)
            })
    }
}

/// One option of a variant and the value it takes.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq, Hash)]
pub struct Selection {
    pub option_id: String,
    pub value: String,
}

/// A fact in a product's table.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq, Hash)]
pub struct Attribute {
    pub name: String,
    pub value: String,
}

/// What a shopper may pay for a product: the two amounts are equal when
/// nothing varies.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PriceRange {
    pub min_cents: i64,
    pub max_cents: i64,
    pub currency: String,
}

impl PriceRange {
    /// Whether the price depends on what a shopper picks.
    pub fn varies(&self) -> bool {
        self.min_cents != self.max_cents
    }

    /// `12.50 €`, or `12.00–18.00 €` when it varies.
    pub fn label(&self) -> String {
        self.label_with(&MoneyFormat::default())
    }

    /// [`label`](Self::label) as `money` writes amounts
    /// ([`money`](crate::money)): `12,50€`, or `12,00€–18,50€` when it
    /// varies (the currency is dropped from the first amount only when it
    /// ends it).
    pub fn label_with(&self, money: &MoneyFormat) -> String {
        let max = money.format(self.max_cents.max(0) as u64, &self.currency);
        if !self.varies() {
            return max;
        }
        money.range(
            self.min_cents.max(0) as u64,
            self.max_cents.max(0) as u64,
            &self.currency,
        )
    }
}

/// A product as the storefront shows it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Product {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub description: String,
    pub amount_cents: Option<i64>,
    pub currency: Option<String>,
    /// Unix seconds of the last change.
    pub updated_at: i64,
    /// Its pictures, in the order the merchant put them.
    pub images: Vec<Image>,
    /// The ways it varies that shoppers choose, with the values on sale;
    /// empty when there is nothing to choose.
    pub options: Vec<ProductOption>,
    /// The combinations on sale, each with its own price.
    pub variants: Vec<Variant>,
    /// The facts in its table.
    pub attributes: Vec<Attribute>,
    pub min_amount_cents: Option<i64>,
    pub max_amount_cents: Option<i64>,
    /// How many published reviews it has.
    pub review_count: i64,
    /// Their average times a hundred (`435` is 4.35 stars); 0 without any.
    pub rating_x100: i64,
    /// What is on the shelf of each thing the shop counts: a variant's id,
    /// or `""` for a product without variants. A thing missing here sells
    /// without limit.
    pub stock: std::collections::BTreeMap<String, u32>,
    /// Whether anything of it can still be bought.
    pub in_stock: bool,
    /// How long delivery takes: its own time, else the shop's default.
    pub delivery_days: Option<DeliveryDays>,
}

impl FromRow for Product {
    fn from_row(row: &Row<'_>) -> anyhow::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            slug: row.get("slug")?,
            name: row.get("name")?,
            description: row.get("description")?,
            amount_cents: row.get("amount_cents")?,
            currency: row.get("currency")?,
            updated_at: row.get("updated_at")?,
            images: row.get::<Json<_>>("images_json")?.0,
            options: row.get::<Json<_>>("options_json")?.0,
            variants: row.get::<Json<_>>("variants_json")?.0,
            attributes: row.get::<Json<_>>("attributes_json")?.0,
            min_amount_cents: row.get("min_amount_cents")?,
            max_amount_cents: row.get("max_amount_cents")?,
            review_count: row.get("review_count")?,
            rating_x100: row.get("rating_x100")?,
            stock: row.get::<Json<_>>("stock_json")?.0,
            in_stock: row.get("in_stock")?,
            delivery_days: match (
                row.get::<Option<i64>>("delivery_min_days")?,
                row.get::<Option<i64>>("delivery_max_days")?,
            ) {
                (Some(min), Some(max)) => Some(DeliveryDays {
                    min: u16::try_from(min)?,
                    max: u16::try_from(max)?,
                }),
                _ => None,
            },
        })
    }
}

impl Product {
    /// How many are left of the product (`variant_id` `None`) or of one of
    /// its variants; `None` when the shop does not count it (no limit).
    pub fn left(&self, variant_id: Option<&str>) -> Option<u32> {
        self.stock.get(variant_id.unwrap_or("")).copied()
    }

    /// Whether that product or variant is sold out.
    pub fn is_sold_out(&self, variant_id: Option<&str>) -> bool {
        self.left(variant_id) == Some(0)
    }

    /// The average of its published reviews, to one decimal (`4.4`); `None`
    /// before the first one.
    pub fn rating(&self) -> Option<f64> {
        (self.review_count > 0).then(|| (self.rating_x100 as f64 / 10.0).round() / 10.0)
    }

    /// Its average as whole stars, rounded (`4` for 4.4): what a row of star
    /// glyphs shows.
    pub fn rating_stars(&self) -> Option<u8> {
        (self.review_count > 0).then(|| ((self.rating_x100 + 50) / 100).clamp(1, 5) as u8)
    }

    /// The price as a listing shows it: `12.50 €`, or `12.00–18.00 €`
    /// when the variants differ. Empty when it has no price at all.
    pub fn price(&self) -> Option<String> {
        Some(self.price_range()?.label())
    }

    /// [`price`](Self::price) as `money` writes amounts
    /// ([`money`](crate::money)): what the [`price`](crate::price) component
    /// shows.
    pub fn price_with(&self, money: &MoneyFormat) -> Option<String> {
        Some(self.price_range()?.label_with(money))
    }

    /// The cheapest and the dearest a shopper may pay.
    pub fn price_range(&self) -> Option<PriceRange> {
        Some(PriceRange {
            min_cents: self.min_amount_cents?,
            max_cents: self.max_amount_cents?,
            currency: self.currency.clone()?,
        })
    }

    /// What a shopper reads for a variant: the values it takes, each as its
    /// option shows it (`Sable / Large`). Empty when the merchant hid every
    /// option.
    pub fn variant_label(&self, variant: &Variant) -> String {
        variant
            .selections
            .iter()
            .map(|selection| {
                self.options
                    .iter()
                    .find(|option| option.id == selection.option_id)
                    .map_or(selection.value.as_str(), |option| {
                        option.display_value(&selection.value)
                    })
            })
            .collect::<Vec<_>>()
            .join(" / ")
    }

    /// The variant those `(option id, value)` pairs name, if the product
    /// sells it: what a page resolves its menus to. With every option hidden
    /// there is one, named by no pair at all.
    pub fn variant(&self, picked: &[(String, String)]) -> Option<&Variant> {
        self.variants.iter().find(|variant| variant.matches(picked))
    }

    /// The picture a listing shows, when it has one.
    pub fn image(&self) -> Option<&Image> {
        self.images.first()
    }

    /// Points every picture at `base`, the address the store's pictures are
    /// served from; without one they keep their bare file name, which a host
    /// serving them itself can still use.
    ///
    /// [`products`](crate::products) and the other context helpers do this
    /// already. It is public for a host that reads the rows itself with
    /// [`published_products`] and friends.
    pub fn fill_urls(&mut self, base: Option<&str>) {
        for image in &mut self.images {
            image.fill_urls(base);
        }
    }
}

/// How many products a page shows unless asked otherwise.
pub const DEFAULT_PER_PAGE: u32 = 24;
pub const MAX_PER_PAGE: u32 = 100;

/// How a page of products is ordered.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum ProductSort {
    /// Most recently changed first.
    #[default]
    Newest,
    /// Cheapest first; a product with no price comes last.
    PriceAsc,
    /// Most expensive first; a product with no price comes last.
    PriceDesc,
    /// Best average first, the most reviewed first among equals; a product
    /// with no published review comes last.
    TopRated,
}

impl ProductSort {
    /// The value a sort `<select>` and a query string use.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Newest => "newest",
            Self::PriceAsc => "price_asc",
            Self::PriceDesc => "price_desc",
            Self::TopRated => "top_rated",
        }
    }

    /// The sort a query string asks for, or [`Newest`](Self::Newest) for
    /// anything else.
    pub fn parse(value: &str) -> Self {
        match value {
            "price_asc" => Self::PriceAsc,
            "price_desc" => Self::PriceDesc,
            "top_rated" => Self::TopRated,
            _ => Self::Newest,
        }
    }
}

/// A page of the catalog: 1-based `page`, `per_page` capped at
/// [`MAX_PER_PAGE`], an optional search over name and slug, a category whose
/// page it is (the whole shop's otherwise), a price band, options, the
/// products to leave out and the order.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProductQuery {
    pub page: u32,
    pub per_page: u32,
    pub q: Option<String>,
    /// A category whose page this is: the products in it and in everything
    /// below it. Without one, the whole shop's: every published product.
    pub category: Option<String>,
    /// Products to leave out, by id: what the home's second rail needs.
    pub exclude: Vec<String>,
    /// The least a product may cost, in minor units: one whose dearest
    /// price is under it is out.
    pub min_cents: Option<i64>,
    /// The most a product may cost: one whose cheapest price is over it is
    /// out.
    pub max_cents: Option<i64>,
    /// `(option key, value key)` pairs a product must every one of, as
    /// [`facet_key`] writes them: "Colour: Rose".
    pub options: Vec<(String, String)>,
    pub sort: ProductSort,
}

impl Default for ProductQuery {
    fn default() -> Self {
        Self {
            page: 1,
            per_page: DEFAULT_PER_PAGE,
            q: None,
            category: None,
            exclude: Vec::new(),
            min_cents: None,
            max_cents: None,
            options: Vec::new(),
            sort: ProductSort::default(),
        }
    }
}

impl ProductQuery {
    pub fn page(mut self, page: u32) -> Self {
        self.page = page.max(1);
        self
    }

    pub fn per_page(mut self, per_page: u32) -> Self {
        self.per_page = per_page.clamp(1, MAX_PER_PAGE);
        self
    }

    pub fn search(mut self, q: impl Into<String>) -> Self {
        let q = q.into();
        self.q = (!q.trim().is_empty()).then(|| q.trim().to_owned());
        self
    }

    /// Only what the page of this category lists: the products in it and in
    /// every category below it.
    pub fn in_category(mut self, id: impl Into<String>) -> Self {
        self.category = Some(id.into());
        self
    }

    pub fn exclude(mut self, id: impl Into<String>) -> Self {
        self.exclude.push(id.into());
        self
    }

    /// Leaves out every one of these products too.
    pub fn exclude_all(mut self, ids: impl IntoIterator<Item = String>) -> Self {
        self.exclude.extend(ids);
        self
    }

    /// Only products a shopper could pay between `min` and `max` for, in
    /// minor units, either bound optional: a product is in when its price
    /// range overlaps the band, so one sold from 9.90 to 59.90 shows under
    /// "up to 50" and under "50 to 70" alike. One with no price never matches
    /// a band.
    pub fn priced_between(mut self, min: Option<i64>, max: Option<i64>) -> Self {
        self.min_cents = min;
        self.max_cents = max;
        self
    }

    /// Only products whose option `name` takes `value`, both compared the way
    /// [`facet_key`] folds them. Several calls narrow further (every pair
    /// must hold).
    pub fn with_option(mut self, name: &str, value: &str) -> Self {
        self.options.push((facet_key(name), facet_key(value)));
        self
    }

    pub fn sort(mut self, sort: ProductSort) -> Self {
        self.sort = sort;
        self
    }
}

/// One value an option takes across the products of a listing, and how many
/// of them take it: what a filter's links are made of.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FacetValue {
    /// What a link carries, as [`facet_key`] writes it: `rose-fonce`.
    pub key: String,
    /// The value as a merchant wrote it: `Rose foncé`. When products spell
    /// it differently, one of the spellings.
    pub label: String,
    pub count: i64,
}

/// The products of one page and what is around it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductPage {
    pub items: Vec<Product>,
    pub page: u32,
    pub per_page: u32,
    /// How many published products match, over every page.
    pub total: i64,
}

impl ProductPage {
    pub fn pages(&self) -> u32 {
        let per_page = i64::from(self.per_page.max(1));
        ((self.total + per_page - 1) / per_page).max(1) as u32
    }

    pub fn has_next(&self) -> bool {
        self.page < self.pages()
    }

    pub fn has_prev(&self) -> bool {
        self.page > 1
    }
}

/// The columns of a product (`p`) as [`Product`] reads them.
macro_rules! product_columns {
    () => {
        "p.id, p.slug, p.name, p.description, p.amount_cents, p.currency, p.updated_at,
         p.images_json, p.options_json, p.variants_json, p.attributes_json, p.min_amount_cents,
         p.max_amount_cents, p.review_count, p.rating_x100, p.stock_json, p.in_stock,
         p.delivery_min_days, p.delivery_max_days"
    };
}

/// A statement written for one query, with its values in the order they
/// are bound. A condition the query leaves out is not written at all: over
/// `?N IS NULL OR …` Turso's planner loses its indexes.
#[derive(Default)]
struct Statement {
    sql: String,
    values: Vec<Value>,
}

impl Statement {
    /// Binds `value` and returns its placeholder.
    fn bind(&mut self, value: impl Into<Value>) -> String {
        self.values.push(value.into());
        format!("?{}", self.values.len())
    }

    /// Binds each of `values` and returns their placeholders, comma
    /// separated.
    fn bind_all<'a>(&mut self, values: impl IntoIterator<Item = &'a String>) -> String {
        values
            .into_iter()
            .map(|value| self.bind(value.as_str()))
            .collect::<Vec<_>>()
            .join(", ")
    }

    async fn fetch_all<T: FromRow>(self, db: &Db) -> anyhow::Result<Vec<T>> {
        db.fetch_all(&self.sql, params_from_iter(self.values)).await
    }

    async fn fetch_scalar<T: FromValue>(self, db: &Db) -> anyhow::Result<T> {
        db.fetch_scalar(&self.sql, params_from_iter(self.values))
            .await
    }
}

/// How a query looks for what a shopper typed.
enum Search {
    /// Through the full-text index over `slug_search` and `name_search`:
    /// `q` long enough for it, as one folded phrase ([`fts_query`]).
    Phrase(String),
    /// A `LIKE` pattern over slug and name, for a shorter `q`: a pass over
    /// the published rows.
    Pattern(String),
}

impl ProductQuery {
    fn searched(&self) -> Option<Search> {
        let q = self.q.as_deref()?;
        Some(match fts_query(q) {
            Some(phrase) => Search::Phrase(phrase),
            None => Search::Pattern(like_contains(q)),
        })
    }

    /// The page whose rows the query reads: its category's, else the whole
    /// shop's.
    fn page_rows(&self) -> &str {
        self.category.as_deref().unwrap_or(WHOLE_SHOP)
    }

    /// The option pairs a product must take, but those on the option
    /// `aside` (the one a facet counts).
    fn options_but<'a>(
        &'a self,
        aside: Option<&'a str>,
    ) -> impl Iterator<Item = &'a (String, String)> {
        self.options
            .iter()
            .filter(move |(name, _)| Some(name.as_str()) != aside)
    }

    /// Whether a price band narrows the page's rows.
    fn banded(&self) -> bool {
        self.min_cents.is_some() || self.max_cents.is_some()
    }
}

impl ProductSort {
    /// The order a page's rows (`cp`) are read in, and the index holding
    /// them in that order.
    fn order(self) -> (&'static str, &'static str) {
        match self {
            Self::Newest => (
                "storefront_category_products_newest",
                "cp.updated_at DESC, cp.product_id DESC",
            ),
            Self::PriceAsc => (
                "storefront_category_products_price_asc",
                "cp.min_amount_cents IS NULL, cp.min_amount_cents ASC, cp.product_id DESC",
            ),
            Self::PriceDesc => (
                "storefront_category_products_price_desc",
                "cp.max_amount_cents IS NULL, cp.max_amount_cents DESC, cp.product_id DESC",
            ),
            Self::TopRated => (
                "storefront_category_products_top_rated",
                "cp.review_count = 0, cp.rating_x100 DESC, cp.review_count DESC, \
                 cp.product_id DESC",
            ),
        }
    }

    /// [`order`](Self::order), over what a search found.
    fn compare(self, a: &Found, b: &Found) -> Ordering {
        let by = match self {
            Self::Newest => b.updated_at.cmp(&a.updated_at),
            Self::PriceAsc => (a.min_amount_cents.is_none(), a.min_amount_cents)
                .cmp(&(b.min_amount_cents.is_none(), b.min_amount_cents)),
            Self::PriceDesc => a
                .max_amount_cents
                .is_none()
                .cmp(&b.max_amount_cents.is_none())
                .then(b.max_amount_cents.cmp(&a.max_amount_cents)),
            Self::TopRated => (a.review_count == 0)
                .cmp(&(b.review_count == 0))
                .then(b.rating_x100.cmp(&a.rating_x100))
                .then(b.review_count.cmp(&a.review_count)),
        };
        by.then_with(|| b.id.cmp(&a.id))
    }
}

/// The rows of the query's page (`cp`) it keeps: its band, the option pairs
/// they must take but those on `aside`, and none of the products it leaves
/// out.
fn page_conditions(statement: &mut Statement, query: &ProductQuery, aside: Option<&str>) -> String {
    let mut conditions = vec![format!(
        "cp.category_id = {}",
        statement.bind(query.page_rows())
    )];
    if let Some(min) = query.min_cents {
        conditions.push(format!("cp.max_amount_cents >= {}", statement.bind(min)));
    }
    if let Some(max) = query.max_cents {
        conditions.push(format!("cp.min_amount_cents <= {}", statement.bind(max)));
    }
    for (name, value) in query.options_but(aside) {
        let (name, value) = (
            statement.bind(name.as_str()),
            statement.bind(value.as_str()),
        );
        conditions.push(format!(
            "EXISTS (SELECT 1 FROM storefront_product_options po
                     WHERE po.product_id = cp.product_id AND po.name_key = {name}
                       AND po.value_key = {value})"
        ));
    }
    if !query.exclude.is_empty() {
        let ids = statement.bind_all(&query.exclude);
        conditions.push(format!("cp.product_id NOT IN ({ids})"));
    }
    conditions.join(" AND ")
}

/// One page of the query's rows, read off the index that holds them in its
/// order: it costs its own length and its offset, however large the shop.
/// The index is named, so the plan never rests on the planner's pick.
fn page_statement(query: &ProductQuery, limit: i64, offset: i64) -> Statement {
    let (index, order) = query.sort.order();
    let mut statement = Statement::default();
    let conditions = page_conditions(&mut statement, query, None);
    let (limit, offset) = (statement.bind(limit), statement.bind(offset));
    statement.sql = format!(
        "SELECT {} FROM storefront_category_products cp INDEXED BY {index}
         JOIN storefront_products p ON p.id = cp.product_id
         WHERE {conditions} ORDER BY {order} LIMIT {limit} OFFSET {offset}",
        product_columns!()
    );
    statement
}

/// How many of the query's rows there are, counted over them.
fn count_statement(query: &ProductQuery) -> Statement {
    let mut statement = Statement::default();
    let conditions = page_conditions(&mut statement, query, None);
    statement.sql =
        format!("SELECT count(*) FROM storefront_category_products cp WHERE {conditions}");
    statement
}

/// Which of the products the query leaves out its page lists, each looked
/// up on its own through the index on the product: given `product_id IN
/// (…)` there, Turso read the whole index.
fn left_out_statement(query: &ProductQuery) -> Statement {
    let mut statement = Statement::default();
    let page = statement.bind(query.page_rows());
    let ids: BTreeSet<&String> = query.exclude.iter().collect();
    statement.sql = ids
        .into_iter()
        .map(|id| {
            format!(
                "SELECT product_id FROM storefront_category_products
                    INDEXED BY storefront_category_products_product
                 WHERE product_id = {} AND category_id = {page}",
                statement.bind(id.as_str())
            )
        })
        .collect::<Vec<_>>()
        .join(" UNION ALL ");
    statement
}

/// How many products the query lists over every page: the count kept for
/// its page when nothing narrows the rows, less the products left out that
/// the page lists; the count kept for one value when that value alone
/// narrows them; counted over the rows otherwise.
async fn listed_total(db: &Db, query: &ProductQuery) -> anyhow::Result<i64> {
    if query.banded() {
        return count_statement(query).fetch_scalar(db).await;
    }
    match (query.options.as_slice(), query.exclude.is_empty()) {
        ([], true) => page_count(db, query.page_rows()).await,
        ([], false) => {
            let listed = page_count(db, query.page_rows()).await?;
            let left_out: Vec<(String,)> = left_out_statement(query).fetch_all(db).await?;
            Ok(listed - left_out.len() as i64)
        }
        ([(name_key, value_key)], true) => {
            let kept: Option<(i64,)> = db
                .fetch_optional(
                    "SELECT products FROM storefront_category_facets
                     WHERE category_id = ?1 AND name_key = ?2 AND value_key = ?3",
                    (query.page_rows(), name_key.as_str(), value_key.as_str()),
                )
                .await?;
            Ok(kept.map_or(0, |(products,)| products))
        }
        _ => count_statement(query).fetch_scalar(db).await,
    }
}

/// How many products the page of `category_id` lists, as its rows keep it.
async fn page_count(db: &Db, category_id: &str) -> anyhow::Result<i64> {
    let count: Option<(i64,)> = if category_id == WHOLE_SHOP {
        db.fetch_optional("SELECT products FROM storefront_shop", ())
            .await?
    } else {
        db.fetch_optional(
            "SELECT subtree_count FROM storefront_categories WHERE id = ?1",
            (category_id,),
        )
        .await?
    };
    Ok(count.map_or(0, |(count,)| count))
}

/// What a search reads of each product it finds: enough to narrow, order
/// and count them here.
struct Found {
    id: String,
    updated_at: i64,
    min_amount_cents: Option<i64>,
    max_amount_cents: Option<i64>,
    review_count: i64,
    rating_x100: i64,
    /// Its option rows, as `storefront_product_options` holds them: read
    /// only when the search narrows by options or counts their values.
    options: Vec<OptionRow>,
}

impl FromRow for Found {
    fn from_row(row: &Row<'_>) -> anyhow::Result<Self> {
        let options: Vec<ProductOption> = row.get::<Json<_>>("options_json")?.0;
        Ok(Self {
            id: row.get("id")?,
            updated_at: row.get("updated_at")?,
            min_amount_cents: row.get("min_amount_cents")?,
            max_amount_cents: row.get("max_amount_cents")?,
            review_count: row.get("review_count")?,
            rating_x100: row.get("rating_x100")?,
            options: distinct_option_rows(&options),
        })
    }
}

impl Found {
    /// Whether it takes every one of `pairs`.
    fn takes<'a>(&self, mut pairs: impl Iterator<Item = &'a (String, String)>) -> bool {
        pairs.all(|(name, value)| {
            self.options
                .iter()
                .any(|row| row.name_key == *name && row.value_key == *value)
        })
    }
}

/// The published products a search finds that the rest of the query keeps,
/// but for its option pairs on `aside`, in no particular order; with their
/// option rows when `options` asks for them.
///
/// The full-text index answers on its own: asked inside a join, or for ids
/// a join then looked up, Turso ran the match once per row of the other
/// side, seconds for a common word. So the statement reads the band off the
/// product rows as well, and the rest is worked out here: the category's
/// page (its rows' ids), the products left out, and the options, from the
/// options a row shows, folded as the option rows are.
async fn found(
    db: &Db,
    query: &ProductQuery,
    search: &Search,
    aside: Option<&str>,
    options: bool,
) -> anyhow::Result<Vec<Found>> {
    let mut found: Vec<Found> = found_statement(query, search, aside, options)
        .fetch_all(db)
        .await?;
    if let Some(category) = query.category.as_deref() {
        let listed: HashSet<String> = db
            .fetch_all::<(String,)>(
                "SELECT product_id FROM storefront_category_products WHERE category_id = ?1",
                (category,),
            )
            .await?
            .into_iter()
            .map(|(id,)| id)
            .collect();
        found.retain(|product| listed.contains(&product.id));
    }
    found.retain(|product| {
        !query.exclude.contains(&product.id) && product.takes(query.options_but(aside))
    });
    Ok(found)
}

/// What [`found`] asks the full-text index (or the rows, for a pattern):
/// the published products matching, within the band, with their option rows
/// when `options` asks for them or the query narrows by options other than
/// `aside`.
fn found_statement(
    query: &ProductQuery,
    search: &Search,
    aside: Option<&str>,
    options: bool,
) -> Statement {
    let mut statement = Statement::default();
    let mut conditions = vec![match search {
        Search::Phrase(phrase) => format!(
            "fts_match(p.slug_search, p.name_search, {})",
            statement.bind(phrase.as_str())
        ),
        Search::Pattern(pattern) => {
            let pattern = statement.bind(pattern.as_str());
            format!("(p.slug LIKE {pattern} ESCAPE '\\' OR p.name LIKE {pattern} ESCAPE '\\')")
        }
    }];
    conditions.push("p.status = 'published'".to_owned());
    if let Some(min) = query.min_cents {
        conditions.push(format!("p.max_amount_cents >= {}", statement.bind(min)));
    }
    if let Some(max) = query.max_cents {
        conditions.push(format!("p.min_amount_cents <= {}", statement.bind(max)));
    }
    let options = options || query.options_but(aside).next().is_some();
    statement.sql = format!(
        "SELECT p.id, p.updated_at, p.min_amount_cents, p.max_amount_cents, p.review_count,
            p.rating_x100, {} AS options_json
         FROM storefront_products p WHERE {}",
        if options { "p.options_json" } else { "'[]'" },
        conditions.join(" AND ")
    );
    statement
}

/// One page of what a search finds, ordered here, and how many it finds.
async fn found_page(
    db: &Db,
    query: &ProductQuery,
    search: &Search,
    limit: i64,
    offset: i64,
) -> anyhow::Result<(Vec<Product>, i64)> {
    let mut found = found(db, query, search, None, false).await?;
    found.sort_by(|a, b| query.sort.compare(a, b));
    let total = found.len() as i64;
    let ids = found
        .into_iter()
        .skip(usize::try_from(offset).unwrap_or(usize::MAX))
        .take(usize::try_from(limit).unwrap_or(0))
        .map(|product| product.id)
        .collect();
    Ok((products_in_order(db, ids).await?, total))
}

/// The published products of `ids`, in that order.
async fn products_in_order(db: &Db, ids: Vec<String>) -> anyhow::Result<Vec<Product>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    // One placeholder per id, read through the primary key (see
    // [`published_products_by_ids`]).
    let statement = format!(
        concat!(
            "SELECT ",
            product_columns!(),
            " FROM storefront_products p WHERE p.id IN ({}) AND p.status = 'published'"
        ),
        timada_core::db::placeholders(ids.len())
    );
    let mut products: Vec<Product> = db.fetch_all(&statement, ids.clone()).await?;
    products.sort_by_key(|product| ids.iter().position(|id| *id == product.id));
    Ok(products)
}

/// The values option `name_key` takes among what a search found, each with
/// how many take it, in key order: the facet statement's count, worked out
/// here.
fn facet_of(found: &[Found], name_key: &str) -> Vec<FacetValue> {
    let mut values: BTreeMap<&str, (&str, i64)> = BTreeMap::new();
    for row in found
        .iter()
        .flat_map(|product| &product.options)
        .filter(|row| row.name_key == name_key)
    {
        let (label, count) = values
            .entry(row.value_key.as_str())
            .or_insert((row.value.as_str(), 0));
        *label = (*label).min(row.value.as_str());
        *count += 1;
    }
    values
        .into_iter()
        .map(|(key, (label, count))| FacetValue {
            key: key.to_owned(),
            label: label.to_owned(),
            count,
        })
        .collect()
}

/// The published products a query matches, in the order it asks for.
///
/// Without a search, the page's rows are read in the order they are kept in
/// and the count is the one kept for the page when only products left out
/// narrow it: a listing costs its own length, whatever the size of the
/// shop. A search reads what it finds off the full-text index (a `LIKE`
/// pass for a `q` too short for it) and orders and counts it here.
pub async fn published_products(db: &Db, query: &ProductQuery) -> anyhow::Result<ProductPage> {
    let per_page = query.per_page.clamp(1, MAX_PER_PAGE);
    let page = query.page.max(1);
    let limit = i64::from(per_page);
    let offset = i64::from(page - 1) * limit;
    let (items, total) = match query.searched() {
        Some(search) => found_page(db, query, &search, limit, offset).await?,
        None => (
            page_statement(query, limit, offset).fetch_all(db).await?,
            listed_total(db, query).await?,
        ),
    };
    Ok(ProductPage {
        items,
        page,
        per_page,
        total,
    })
}

/// The values `option` takes among the published products `query` matches,
/// each with how many take it, in key order: the links of a filter. The
/// query's own pairs on `option` are set aside, so the listing filtered to
/// "Colour: Rose" still offers the other colours; its band and its other
/// options hold.
pub async fn published_option_values(
    db: &Db,
    query: &ProductQuery,
    option: &str,
) -> anyhow::Result<Vec<FacetValue>> {
    let name_key = facet_key(option);
    if let Some(search) = query.searched() {
        let found = found(db, query, &search, Some(name_key.as_str()), true).await?;
        return Ok(facet_of(&found, &name_key));
    }
    // Nothing narrows the page but the option's own values: the counts the
    // rows keep for it.
    let statement = if !query.banded()
        && query.exclude.is_empty()
        && query.options_but(Some(&name_key)).next().is_none()
    {
        kept_facet_statement(query.page_rows(), &name_key)
    } else {
        facet_statement(query, &name_key)
    };
    let rows: Vec<(String, String, i64)> = statement.fetch_all(db).await?;
    Ok(rows
        .into_iter()
        .map(|(key, label, count)| FacetValue { key, label, count })
        .collect())
}

/// The values option `name_key` takes among what the page of `category_id`
/// lists, as its rows keep them.
fn kept_facet_statement(category_id: &str, name_key: &str) -> Statement {
    let mut statement = Statement::default();
    let (page, name) = (statement.bind(category_id), statement.bind(name_key));
    statement.sql = format!(
        "SELECT value_key, value, products FROM storefront_category_facets
         WHERE category_id = {page} AND name_key = {name} ORDER BY value_key"
    );
    statement
}

/// The values option `name_key` takes among the query's rows, each with how
/// many take it, the query's own pairs on it set aside.
fn facet_statement(query: &ProductQuery, name_key: &str) -> Statement {
    let mut statement = Statement::default();
    let conditions = page_conditions(&mut statement, query, Some(name_key));
    let name = statement.bind(name_key);
    statement.sql = format!(
        "SELECT po.value_key, min(po.value), count(*)
         FROM storefront_category_products cp
         JOIN storefront_product_options po ON po.product_id = cp.product_id AND po.name_key = {name}
         WHERE {conditions}
         GROUP BY po.value_key ORDER BY po.value_key"
    );
    statement
}

/// What goes well with a cart holding `product_ids`: the published products
/// the pages of their categories list, newest first, none of them already
/// in it. With no category to go by, the newest products.
pub async fn published_cart_recommendations(
    db: &Db,
    product_ids: &[String],
    limit: u32,
) -> anyhow::Result<Vec<Product>> {
    let mut categories: Vec<String> = if product_ids.is_empty() {
        Vec::new()
    } else {
        db.fetch_all::<(String,)>(
            &format!(
                "SELECT DISTINCT category_id FROM storefront_product_categories
                 WHERE product_id IN ({})",
                timada_core::db::placeholders(product_ids.len())
            ),
            product_ids.to_vec(),
        )
        .await?
        .into_iter()
        .map(|(id,)| id)
        .collect()
    };
    if categories.is_empty() {
        categories.push(WHOLE_SHOP.to_owned());
    }
    newest_listed_on(db, &categories, product_ids, limit).await
}

/// The newest published products the pages of `categories` list, none of
/// `exclude`, at most `limit`: each page's newest read off its index and
/// merged here, so the cost is the number of pages, not their length.
async fn newest_listed_on(
    db: &Db,
    categories: &[String],
    exclude: &[String],
    limit: u32,
) -> anyhow::Result<Vec<Product>> {
    let mut newest: Vec<(i64, String)> = Vec::new();
    for category in categories {
        newest.extend(
            newest_statement(category, exclude, limit)
                .fetch_all::<(i64, String)>(db)
                .await?,
        );
    }
    // Newest first; a product two of the pages list comes twice, alike.
    newest.sort_unstable_by(|a, b| b.cmp(a));
    newest.dedup();
    newest.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
    products_in_order(db, newest.into_iter().map(|(_, id)| id).collect()).await
}

/// The newest of the page of `category`, none of `exclude`, `limit` at
/// most, read off the index holding them newest first.
fn newest_statement(category: &str, exclude: &[String], limit: u32) -> Statement {
    let mut statement = Statement::default();
    let page = statement.bind(category);
    let left_out = if exclude.is_empty() {
        String::new()
    } else {
        format!("AND product_id NOT IN ({})", statement.bind_all(exclude))
    };
    let limit = statement.bind(i64::from(limit));
    statement.sql = format!(
        "SELECT updated_at, product_id FROM storefront_category_products
            INDEXED BY storefront_category_products_newest
         WHERE category_id = {page} {left_out}
         ORDER BY updated_at DESC, product_id DESC LIMIT {limit}"
    );
    statement
}

/// One published product, by slug. The slug's index keeps every product not
/// archived: named, with its condition repeated, so the product is read off
/// it; left to choose, Turso walked every published product.
pub async fn published_product_by_slug(db: &Db, slug: &str) -> anyhow::Result<Option<Product>> {
    db.fetch_optional(
        concat!(
            "SELECT ",
            product_columns!(),
            " FROM storefront_products p INDEXED BY storefront_products_slug
             WHERE p.slug = ?1 AND p.status <> 'archived' AND p.status = 'published'"
        ),
        (slug,),
    )
    .await
}

/// The published products among `ids`, in no particular order: what a cart
/// page shows its lines with (a product missing here is off sale).
pub async fn published_products_by_ids(db: &Db, ids: &[&str]) -> anyhow::Result<Vec<Product>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    // One placeholder per id rather than a `json_each` list: given the list
    // Turso walks every published product through the status index
    // (`SEARCH p USING INDEX storefront_products_price (status=?)`), so a
    // cart cost the whole catalogue; given the ids it reads them one by one
    // through the primary key (see the `probe` tests).
    let statement = format!(
        concat!(
            "SELECT ",
            product_columns!(),
            " FROM storefront_products p WHERE p.id IN ({}) AND p.status = 'published'"
        ),
        timada_core::db::placeholders(ids.len())
    );
    // Owned before the await: a parameter iterator borrowing `&[&str]`
    // carries two lifetimes into the future, and the pages that call this
    // through `cart` then fail to prove the future `Send` for every one of
    // them ("implementation of `Send` is not general enough").
    let params: Vec<String> = ids.iter().map(|id| id.to_string()).collect();
    db.fetch_all(&statement, params).await
}

// =============================================================================
// Categories
// =============================================================================

/// A category of the store, as a page shows it.
#[derive(Debug, Clone, FromRow, PartialEq, Eq, Hash)]
pub struct Category {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub description: String,
    /// `None` for a top-level category.
    pub parent_id: Option<String>,
    /// Where it sorts among its siblings, lowest first.
    pub position: i64,
    /// Shown in the shop's menu. A hidden category keeps its page; it and
    /// everything under it are only left out of [`menu_of`].
    pub in_menu: bool,
}

impl TreeNode for Category {
    fn node_id(&self) -> &str {
        &self.id
    }

    fn node_parent_id(&self) -> Option<&str> {
        self.parent_id.as_deref()
    }
}

/// A category and what hangs under it, with how many published products are
/// in it and its sub-categories.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CategoryNode {
    pub category: Category,
    pub children: Vec<CategoryNode>,
    /// Distinct published products this category's page lists: those in it
    /// and in every category below it, each counted once.
    pub product_count: i64,
}

impl CategoryNode {
    /// The categories from this one down to `id`, excluding `id` itself, if
    /// `id` is in this subtree.
    fn path_to(&self, id: &str) -> Option<Vec<Category>> {
        if self.category.id == id {
            return Some(Vec::new());
        }
        self.children.iter().find_map(|child| {
            let mut path = child.path_to(id)?;
            path.insert(0, self.category.clone());
            Some(path)
        })
    }

    /// The node of `slug` in this subtree, if any.
    pub fn find(&self, slug: &str) -> Option<&CategoryNode> {
        if self.category.slug == slug {
            return Some(self);
        }
        self.children.iter().find_map(|child| child.find(slug))
    }
}

/// The node of `slug` among these trees, if any.
pub fn find_category<'a>(nodes: &'a [CategoryNode], slug: &str) -> Option<&'a CategoryNode> {
    nodes.iter().find_map(|node| node.find(slug))
}

/// The trees a menu shows: `nodes` without the categories the merchant hid
/// from it, each taking its subtree along.
pub fn menu_of(nodes: &[CategoryNode]) -> Vec<CategoryNode> {
    nodes
        .iter()
        .filter(|node| node.category.in_menu)
        .map(|node| CategoryNode {
            category: node.category.clone(),
            children: menu_of(&node.children),
            product_count: node.product_count,
        })
        .collect()
}

/// What a search compares: `text` without accents or case, its whitespace
/// collapsed, so "LITIERE" finds "Litière chat".
pub fn search_key(text: &str) -> String {
    deunicode::deunicode(text)
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// What a filter carries and compares: [`search_key`] with its spaces as
/// dashes, so "Rose foncé" is `rose-fonce` in a link and in the rows alike.
pub fn facet_key(text: &str) -> String {
    search_key(text).replace(' ', "-")
}

/// The categories among `nodes` (and everything under them) whose name
/// contains `q`, accents and case aside: the ones whose name starts with it
/// first, then the rest, each group in tree order; at most `limit`. Nothing
/// for a blank `q`.
pub fn search_categories<'a>(
    nodes: &'a [CategoryNode],
    q: &str,
    limit: usize,
) -> Vec<&'a CategoryNode> {
    let q = search_key(q);
    if q.is_empty() || limit == 0 {
        return Vec::new();
    }
    let mut starting = Vec::new();
    let mut containing = Vec::new();
    fn walk<'a>(
        nodes: &'a [CategoryNode],
        q: &str,
        starting: &mut Vec<&'a CategoryNode>,
        containing: &mut Vec<&'a CategoryNode>,
    ) {
        for node in nodes {
            let name = search_key(&node.category.name);
            if name.starts_with(q) {
                starting.push(node);
            } else if name.contains(q) {
                containing.push(node);
            }
            walk(&node.children, q, starting, containing);
        }
    }
    walk(nodes, &q, &mut starting, &mut containing);
    starting.extend(containing);
    starting.truncate(limit);
    starting
}

/// A category row with the count its page lists, as the subscription stored
/// it.
struct CountedCategory {
    category: Category,
    subtree_count: i64,
}

impl FromRow for CountedCategory {
    fn from_row(row: &Row<'_>) -> anyhow::Result<Self> {
        Ok(Self {
            category: Category::from_row(row)?,
            subtree_count: row.get("subtree_count")?,
        })
    }
}

impl TreeNode for CountedCategory {
    fn node_id(&self) -> &str {
        &self.category.id
    }

    fn node_parent_id(&self) -> Option<&str> {
        self.category.parent_id.as_deref()
    }
}

/// The live categories of the store as trees, each level in its own order,
/// with how many published products each one's page lists.
pub async fn published_categories(db: &Db) -> anyhow::Result<Vec<CategoryNode>> {
    let categories: Vec<CountedCategory> = db
        .fetch_all(
            "SELECT id, slug, name, description, parent_id, position, in_menu, subtree_count
             FROM storefront_categories WHERE archived = 0
             ORDER BY position, name, id",
            (),
        )
        .await?;
    Ok(tree_of(categories).into_iter().map(node_of).collect())
}

/// Turns a tree of counted categories into one of [`CategoryNode`].
fn node_of(node: TreeOf<CountedCategory>) -> CategoryNode {
    CategoryNode {
        category: node.item.category,
        children: node.children.into_iter().map(node_of).collect(),
        product_count: node.item.subtree_count,
    }
}

/// One live category, by slug.
pub async fn published_category_by_slug(db: &Db, slug: &str) -> anyhow::Result<Option<Category>> {
    db.fetch_optional(
        "SELECT id, slug, name, description, parent_id, position, in_menu
         FROM storefront_categories WHERE slug = ?1 AND archived = 0",
        (slug,),
    )
    .await
}

/// The text a live category's page shows below its products, empty when
/// it has none. Apart from [`Category`], so the trees every page loads do
/// not carry it.
pub async fn published_category_body(db: &Db, category_id: &str) -> anyhow::Result<String> {
    let body: Option<(String,)> = db
        .fetch_optional(
            "SELECT body FROM storefront_categories WHERE id = ?1 AND archived = 0",
            (category_id,),
        )
        .await?;
    Ok(body.map(|(body,)| body).unwrap_or_default())
}

/// The live categories above `category`, top-level first: a breadcrumb,
/// read off the menu a page has already loaded. Stops where a parent is
/// archived, since the tree does.
pub fn category_ancestors(nodes: &[CategoryNode], category: &Category) -> Vec<Category> {
    nodes
        .iter()
        .find_map(|node| node.path_to(&category.id))
        .unwrap_or_default()
}

/// The live categories a product is filed under, in tree order.
pub async fn published_product_categories(
    db: &Db,
    product_id: &str,
) -> anyhow::Result<Vec<Category>> {
    db.fetch_all(
        "SELECT c.id, c.slug, c.name, c.description, c.parent_id, c.position, c.in_menu
         FROM storefront_categories c
         JOIN storefront_product_categories pc ON pc.category_id = c.id
         WHERE pc.product_id = ?1 AND c.archived = 0
         ORDER BY c.position, c.name, c.id",
        (product_id,),
    )
    .await
}

/// The published products the pages of `product`'s categories list, newest
/// first, without `product` itself: more from the shelves it is on.
pub async fn published_related_products(
    db: &Db,
    product: &Product,
    limit: u32,
) -> anyhow::Result<Vec<Product>> {
    let categories: Vec<String> = db
        .fetch_all::<(String,)>(
            "SELECT category_id FROM storefront_product_categories WHERE product_id = ?1",
            (product.id.as_str(),),
        )
        .await?
        .into_iter()
        .map(|(id,)| id)
        .collect();
    if categories.is_empty() {
        return Ok(Vec::new());
    }
    newest_listed_on(db, &categories, std::slice::from_ref(&product.id), limit).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(name: &str, children: Vec<CategoryNode>) -> CategoryNode {
        CategoryNode {
            category: Category {
                id: name.to_owned(),
                slug: name.to_lowercase(),
                name: name.to_owned(),
                description: String::new(),
                parent_id: None,
                position: 0,
                in_menu: true,
            },
            children,
            product_count: 0,
        }
    }

    fn tree() -> Vec<CategoryNode> {
        vec![
            node("Couchage", vec![node("Petit lit", vec![])]),
            node("Litière chat", vec![node("Litière de voyage", vec![])]),
            node("Jouets", vec![]),
        ]
    }

    fn names<'a>(found: &[&'a CategoryNode]) -> Vec<&'a str> {
        found.iter().map(|n| n.category.name.as_str()).collect()
    }

    #[test]
    fn facet_key_dashes_the_spaces() {
        assert_eq!(facet_key("Rose foncé"), "rose-fonce");
        assert_eq!(facet_key("  Bleu   Foncé "), "bleu-fonce");
        assert_eq!(facet_key("36-37"), "36-37");
        assert_eq!(facet_key(""), "");
    }

    #[test]
    fn search_key_drops_accents_case_and_extra_spaces() {
        assert_eq!(search_key("  Litière   DE voyage "), "litiere de voyage");
        assert_eq!(search_key("Été"), "ete");
    }

    #[test]
    fn categories_matching_a_prefix_come_first_then_the_rest_in_tree_order() {
        let tree = tree();
        assert_eq!(
            names(&search_categories(&tree, "lit", 10)),
            ["Litière chat", "Litière de voyage", "Petit lit"]
        );
    }

    #[test]
    fn categories_match_without_accents_or_case() {
        let tree = tree();
        assert_eq!(
            names(&search_categories(&tree, "LITIERE DE", 10)),
            ["Litière de voyage"]
        );
    }

    #[test]
    fn category_search_honours_the_limit_and_ignores_a_blank_query() {
        let tree = tree();
        assert_eq!(
            names(&search_categories(&tree, "lit", 2)),
            ["Litière chat", "Litière de voyage"]
        );
        assert!(search_categories(&tree, "   ", 10).is_empty());
        assert!(search_categories(&tree, "lit", 0).is_empty());
        assert!(search_categories(&tree, "chien", 10).is_empty());
    }
}

/// The filters over a small catalog in an in-memory database.
#[cfg(test)]
mod sql {
    use super::*;
    use crate::migrations::migrate;

    async fn db() -> Db {
        let db = Db::from(
            timada_core::db::Turso::in_memory_concurrent()
                .await
                .unwrap(),
        );
        migrate(&db).await.unwrap();
        db
    }

    async fn insert_product(
        db: &Db,
        id: &str,
        status: &str,
        range: Option<(i64, i64)>,
        colours: &[&str],
    ) {
        let options = if colours.is_empty() {
            "[]".to_owned()
        } else {
            serde_json::to_string(&[ProductOption {
                id: "opt-colour".to_owned(),
                name: "Couleur".to_owned(),
                values: colours.iter().map(|c| (*c).to_owned()).collect(),
                label: None,
                value_labels: BTreeMap::new(),
            }])
            .unwrap()
        };
        // What the subscription writes: the row, with what a search finds
        // it by.
        db.execute(
            "INSERT INTO storefront_products (id, slug, name, description, amount_cents,
                currency, status, updated_at, images_json, options_json, variants_json,
                attributes_json, min_amount_cents, max_amount_cents, slug_search, name_search)
             VALUES (?1, ?1, ?1, '', ?2, 'EUR', ?3, 1, '[]', ?4, '[]', '[]', ?5, ?6, ?7, ?7)",
            (
                id,
                range.map(|(min, _)| min),
                status,
                options.as_str(),
                range.map(|(min, _)| min),
                range.map(|(_, max)| max),
                timada_core::db::search_key(id),
            ),
        )
        .await
        .unwrap();
        for colour in colours {
            db.execute(
                crate::read_model::INSERT_OPTION,
                (id, "couleur", facet_key(colour), "Couleur", *colour),
            )
            .await
            .unwrap();
        }
    }

    /// A collar from 9.90 to 59.90 in rose or blue, a grey basket at 34.90,
    /// an unpriced service, and a draft nobody sees; the first two filed in a
    /// category whose stored count is 2.
    async fn catalog() -> Db {
        let db = db().await;
        insert_product(
            &db,
            "collar",
            "published",
            Some((990, 5990)),
            &["Rose", "Bleu"],
        )
        .await;
        insert_product(&db, "basket", "published", Some((3490, 3490)), &["Gris"]).await;
        insert_product(&db, "service", "published", None, &[]).await;
        insert_product(&db, "draft", "draft", Some((1000, 1000)), &["Rose"]).await;
        db.execute(
            "INSERT INTO storefront_categories (id, slug, name, description, body, position,
                archived, in_menu, updated_at, product_count, subtree_count)
             VALUES ('cat', 'cat', 'Cat', '', '', 0, 0, 1, 1, 2, 2)",
            (),
        )
        .await
        .unwrap();
        for product in ["collar", "basket"] {
            db.execute(
                "INSERT INTO storefront_product_categories (product_id, category_id)
                 VALUES (?1, 'cat')",
                (product,),
            )
            .await
            .unwrap();
            db.execute(
                "INSERT INTO storefront_category_products (category_id, product_id,
                    updated_at, min_amount_cents, max_amount_cents, review_count, rating_x100)
                 SELECT 'cat', id, updated_at, min_amount_cents, max_amount_cents, review_count,
                    rating_x100
                 FROM storefront_products WHERE id = ?1",
                (product,),
            )
            .await
            .unwrap();
        }
        // The whole shop's page: every published product, and its count.
        db.execute(
            "INSERT INTO storefront_category_products (category_id, product_id, updated_at,
                min_amount_cents, max_amount_cents, review_count, rating_x100)
             SELECT '', id, updated_at, min_amount_cents, max_amount_cents, review_count,
                rating_x100
             FROM storefront_products WHERE status = 'published'",
            (),
        )
        .await
        .unwrap();
        db.execute(
            "INSERT INTO storefront_shop (id, products)
             SELECT 1, count(*) FROM storefront_products WHERE status = 'published'",
            (),
        )
        .await
        .unwrap();
        // What each page counts of the values its products take.
        db.execute(
            "INSERT INTO storefront_category_facets (category_id, name_key, value_key, value,
                products)
             SELECT cp.category_id, po.name_key, po.value_key, min(po.value), count(*)
             FROM storefront_category_products cp
             JOIN storefront_product_options po ON po.product_id = cp.product_id
             GROUP BY cp.category_id, po.name_key, po.value_key",
            (),
        )
        .await
        .unwrap();
        db
    }

    async fn ids(db: &Db, query: ProductQuery) -> (Vec<String>, i64) {
        let page = published_products(db, &query).await.unwrap();
        (page.items.into_iter().map(|p| p.id).collect(), page.total)
    }

    fn owned(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|id| (*id).to_owned()).collect()
    }

    /// Eight products named `item0` to `item7` (and found by that name), each
    /// with its own mix of change time, prices and reviews, ties and gaps
    /// included: what the sorts must agree on.
    async fn varied() -> Db {
        let db = db().await;
        /// Its change time, its price range, its reviews and their average.
        type Item = (i64, Option<(i64, i64)>, i64, i64);
        let rows: [Item; 8] = [
            (5, Some((1000, 1000)), 0, 0),
            (5, Some((500, 9000)), 3, 450),
            (9, None, 1, 500),
            (1, Some((500, 500)), 3, 450),
            (9, Some((2000, 2500)), 0, 0),
            (3, None, 10, 450),
            (7, Some((1000, 9000)), 2, 300),
            (2, Some((300, 4000)), 0, 0),
        ];
        for (n, (updated_at, range, review_count, rating)) in rows.into_iter().enumerate() {
            let id = format!("item{n}");
            db.execute(
                "INSERT INTO storefront_products (id, slug, name, description, currency, status,
                    updated_at, images_json, options_json, variants_json, attributes_json,
                    min_amount_cents, max_amount_cents, review_count, rating_x100, slug_search,
                    name_search)
                 VALUES (?1, ?1, ?1, '', 'EUR', 'published', ?2, '[]', '[]', '[]', '[]', ?3,
                    ?4, ?5, ?6, ?1, ?1)",
                (
                    id.as_str(),
                    updated_at,
                    range.map(|(min, _)| min),
                    range.map(|(_, max)| max),
                    review_count,
                    rating,
                ),
            )
            .await
            .unwrap();
        }
        db.execute(
            "INSERT INTO storefront_category_products (category_id, product_id, updated_at,
                min_amount_cents, max_amount_cents, review_count, rating_x100)
             SELECT '', id, updated_at, min_amount_cents, max_amount_cents, review_count,
                rating_x100
             FROM storefront_products",
            (),
        )
        .await
        .unwrap();
        db.execute(
            "INSERT INTO storefront_shop (id, products) VALUES (1, 8)",
            (),
        )
        .await
        .unwrap();
        db
    }

    #[tokio::test]
    async fn a_search_orders_as_the_listing_does() {
        let db = varied().await;
        for sort in [
            ProductSort::Newest,
            ProductSort::PriceAsc,
            ProductSort::PriceDesc,
            ProductSort::TopRated,
        ] {
            let listed = ids(&db, ProductQuery::default().sort(sort)).await;
            assert_eq!(listed.1, 8);
            // The index, then the rows for a query too short for it.
            for q in ["item", "it"] {
                assert_eq!(
                    ids(&db, ProductQuery::default().sort(sort).search(q)).await,
                    listed,
                    "{sort:?} searched for {q:?}"
                );
            }
            // A page further down, of either.
            let second = ProductQuery::default().sort(sort).per_page(3).page(2);
            assert_eq!(
                ids(&db, second.clone().search("item")).await,
                ids(&db, second).await,
                "{sort:?}, second page"
            );
        }
    }

    #[tokio::test]
    async fn a_search_keeps_what_the_rest_of_the_query_keeps() {
        let db = catalog().await;
        let found = |query: ProductQuery| ids(&db, query.sort(ProductSort::PriceAsc));
        // A pattern, too short for the index: the draft stays out.
        assert_eq!(
            found(ProductQuery::default().search("a")).await,
            (owned(&["collar", "basket"]), 2)
        );
        // A phrase, through the index.
        assert_eq!(
            found(ProductQuery::default().search("COLLA")).await,
            (owned(&["collar"]), 1)
        );
        let a = ProductQuery::default().search("a");
        assert_eq!(
            found(a.clone().priced_between(Some(5000), None)).await,
            (owned(&["collar"]), 1)
        );
        assert_eq!(
            found(a.clone().with_option("Couleur", "Gris")).await,
            (owned(&["basket"]), 1)
        );
        assert_eq!(
            found(a.clone().exclude("collar")).await,
            (owned(&["basket"]), 1)
        );
        assert_eq!(
            found(a.clone().in_category("cat")).await,
            (owned(&["collar", "basket"]), 2)
        );
        assert_eq!(
            found(ProductQuery::default().search("servi").in_category("cat"))
                .await
                .1,
            0
        );
        // The facet of a search: its own colour set aside, its band kept.
        async fn colours(db: &Db, query: ProductQuery) -> Vec<(String, i64)> {
            published_option_values(db, &query, "Couleur")
                .await
                .unwrap()
                .into_iter()
                .map(|value| (value.key, value.count))
                .collect()
        }
        assert_eq!(
            colours(&db, a.clone().with_option("Couleur", "Rose")).await,
            [("bleu".into(), 1), ("gris".into(), 1), ("rose".into(), 1)]
        );
        assert_eq!(
            colours(&db, a.priced_between(Some(5000), Some(7000))).await,
            [("bleu".into(), 1), ("rose".into(), 1)]
        );
    }

    #[tokio::test]
    async fn the_whole_shop_counts_what_it_lists_but_what_it_leaves_out() {
        let db = catalog().await;
        let total = |query: ProductQuery| async { ids(&db, query).await.1 };
        assert_eq!(total(ProductQuery::default()).await, 3);
        assert_eq!(total(ProductQuery::default().exclude("collar")).await, 2);
        // A product the page does not list, or one named twice, changes
        // nothing more.
        assert_eq!(total(ProductQuery::default().exclude("draft")).await, 3);
        assert_eq!(
            total(
                ProductQuery::default()
                    .exclude("collar")
                    .exclude("collar")
                    .exclude("missing")
            )
            .await,
            2
        );
        assert_eq!(
            total(ProductQuery::default().in_category("cat").exclude("basket")).await,
            1
        );
        assert_eq!(
            ids(
                &db,
                ProductQuery::default()
                    .exclude("collar")
                    .sort(ProductSort::PriceAsc)
            )
            .await
            .0,
            owned(&["basket", "service"])
        );
    }

    #[tokio::test]
    async fn related_products_come_from_the_shelves_it_is_on() {
        let db = catalog().await;
        let by_id = |id: &'static str| {
            let db = db.clone();
            async move {
                published_products_by_ids(&db, &[id])
                    .await
                    .unwrap()
                    .pop()
                    .unwrap()
            }
        };
        let related = |product: Product| {
            let db = db.clone();
            async move {
                published_related_products(&db, &product, 4)
                    .await
                    .unwrap()
                    .into_iter()
                    .map(|p| p.id)
                    .collect::<Vec<_>>()
            }
        };
        assert_eq!(related(by_id("collar").await).await, ["basket"]);
        // Filed nowhere: nothing to go by.
        assert!(related(by_id("service").await).await.is_empty());
        let offered = |ids: &[&str], limit: u32| {
            let db = db.clone();
            let ids: Vec<String> = ids.iter().map(|id| (*id).to_owned()).collect();
            async move {
                published_cart_recommendations(&db, &ids, limit)
                    .await
                    .unwrap()
                    .into_iter()
                    .map(|p| p.id)
                    .collect::<Vec<_>>()
            }
        };
        assert_eq!(offered(&["collar"], 4).await, ["basket"]);
        // A cart with nothing filed anywhere, or empty: the newest, none of
        // its own (the change times tie, so the ids decide).
        assert_eq!(offered(&["service"], 4).await, ["collar", "basket"]);
        assert_eq!(offered(&[], 2).await, ["service", "collar"]);
    }

    #[tokio::test]
    async fn a_product_is_found_by_slug_while_published() {
        let db = catalog().await;
        let found = |slug: &'static str| {
            let db = db.clone();
            async move {
                published_product_by_slug(&db, slug)
                    .await
                    .unwrap()
                    .map(|p| p.id)
            }
        };
        assert_eq!(found("collar").await.as_deref(), Some("collar"));
        assert_eq!(found("draft").await, None);
        assert_eq!(found("missing").await, None);
    }

    #[tokio::test]
    async fn the_kept_counts_agree_with_the_rows() {
        let db = catalog().await;
        for page in [None, Some("cat")] {
            let mut query = ProductQuery::default();
            if let Some(category) = page {
                query = query.in_category(category);
            }
            let kept: Vec<(String, String, i64)> =
                kept_facet_statement(query.page_rows(), "couleur")
                    .fetch_all(&db)
                    .await
                    .unwrap();
            let counted: Vec<(String, String, i64)> = facet_statement(&query, "couleur")
                .fetch_all(&db)
                .await
                .unwrap();
            assert_eq!(kept, counted, "{page:?}");
            for (key, _, _) in &counted {
                let one = query.clone().with_option("Couleur", key);
                assert_eq!(
                    listed_total(&db, &one).await.unwrap(),
                    count_statement(&one)
                        .fetch_scalar::<i64>(&db)
                        .await
                        .unwrap(),
                    "{page:?} {key}"
                );
            }
        }
        // A value no listed product takes counts nothing.
        assert_eq!(
            ids(&db, ProductQuery::default().with_option("Couleur", "Vert"))
                .await
                .1,
            0
        );
    }

    /// The `detail` of each step of `statement`'s plan.
    async fn plan_of(db: &Db, statement: &Statement) -> Vec<String> {
        db.fetch_all::<(i64, i64, i64, String)>(
            &format!("EXPLAIN QUERY PLAN {}", statement.sql),
            params_from_iter(statement.values.clone()),
        )
        .await
        .unwrap()
        .into_iter()
        .map(|(_, _, _, detail)| detail)
        .collect()
    }

    /// Fails unless a step of `plan` searches `index` (rather than scanning
    /// it whole), and when any step sorts.
    fn reads_in_order(plan: &[String], index: &str) {
        assert!(
            plan.iter()
                .any(|step| step.starts_with("SEARCH") && step.contains(index)),
            "no step searches {index}:\n{}",
            plan.join("\n")
        );
        assert!(
            !plan
                .iter()
                .any(|step| step.contains("SORTER") || step.contains("B-TREE")),
            "the plan sorts:\n{}",
            plan.join("\n")
        );
    }

    /// The plans the statements are written for: a page read off the index
    /// of its order, never sorted, whatever narrows it; a search answered
    /// by the full-text index alone; the counts and facets off the page's
    /// rows, never the product rows.
    #[tokio::test]
    async fn the_statements_read_the_indexes_they_are_written_for() {
        let db = catalog().await;
        let narrowed = ProductQuery::default()
            .in_category("cat")
            .priced_between(Some(100), Some(9000))
            .with_option("Couleur", "Rose")
            .exclude("basket");
        for sort in [
            ProductSort::Newest,
            ProductSort::PriceAsc,
            ProductSort::PriceDesc,
            ProductSort::TopRated,
        ] {
            let (index, _) = sort.order();
            for query in [ProductQuery::default(), narrowed.clone()] {
                let plan = plan_of(&db, &page_statement(&query.sort(sort), 24, 48)).await;
                reads_in_order(&plan, index);
            }
        }
        for statement in [
            count_statement(&narrowed),
            facet_statement(&narrowed, "couleur"),
        ] {
            let plan = plan_of(&db, &statement).await;
            assert!(
                !plan.iter().any(|step| step.contains("storefront_products ")
                    || step.ends_with("storefront_products")
                    || step.contains(" p ")),
                "reads the product rows:\n{}",
                plan.join("\n")
            );
        }
        let left_out = plan_of(
            &db,
            &left_out_statement(&narrowed.clone().exclude("collar").exclude("basket")),
        )
        .await;
        let lookups: Vec<&String> = left_out
            .iter()
            .filter(|step| step.contains("storefront_category_products"))
            .collect();
        assert_eq!(lookups.len(), 2, "{left_out:?}");
        assert!(
            lookups.iter().all(|step| step.starts_with("SEARCH")
                && step.contains("storefront_category_products_product (product_id=? AND")),
            "{left_out:?}"
        );
        // Ids given as a list, to the primary keys: one lookup each.
        for (sql, index) in [
            (
                "SELECT p.id FROM storefront_products p
                 WHERE p.id IN (?1, ?2) AND p.status = 'published'",
                "sqlite_autoindex_storefront_products_1 (id=?)",
            ),
            (
                "SELECT DISTINCT category_id FROM storefront_product_categories
                 WHERE product_id IN (?1, ?2)",
                "sqlite_autoindex_storefront_product_categories_1 (product_id=?)",
            ),
        ] {
            let plan = plan_of(
                &db,
                &Statement {
                    sql: sql.to_owned(),
                    values: vec![Value::from("collar"), Value::from("basket")],
                },
            )
            .await;
            assert!(
                plan.iter()
                    .any(|step| step.starts_with("SEARCH") && step.contains(index)),
                "{sql}: {plan:?}"
            );
        }
        let search = ProductQuery::default().search("collar");
        let phrase = search.searched().unwrap();
        for options in [false, true] {
            let plan = plan_of(&db, &found_statement(&search, &phrase, None, options)).await;
            assert_eq!(plan, ["QUERY INDEX METHOD fts"], "with options: {options}");
        }
        reads_in_order(
            &plan_of(&db, &newest_statement("cat", &["basket".to_owned()], 4)).await,
            "storefront_category_products_newest",
        );
        let by_slug = Statement {
            sql: concat!(
                "SELECT ",
                product_columns!(),
                " FROM storefront_products p INDEXED BY storefront_products_slug
                 WHERE p.slug = ?1 AND p.status <> 'archived' AND p.status = 'published'"
            )
            .to_owned(),
            values: vec![Value::from("collar")],
        };
        let plan = plan_of(&db, &by_slug).await;
        assert!(
            plan.iter()
                .all(|step| step.contains("storefront_products_slug")),
            "{plan:?}"
        );
    }

    #[tokio::test]
    async fn products_by_ids_keeps_the_published_ones_among_them() {
        let db = catalog().await;
        let found = published_products_by_ids(&db, &["basket", "draft", "missing", "collar"])
            .await
            .unwrap();
        let mut ids: Vec<&str> = found.iter().map(|p| p.id.as_str()).collect();
        ids.sort_unstable();
        assert_eq!(ids, ["basket", "collar"]);
        assert!(
            published_products_by_ids(&db, &[])
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn a_band_keeps_what_overlaps_it_and_drops_the_unpriced() {
        let db = catalog().await;
        let all = ProductQuery::default().sort(ProductSort::PriceAsc);
        assert_eq!(
            ids(&db, all.clone()).await,
            (owned(&["collar", "basket", "service"]), 3)
        );
        // The collar reaches into both bands; the basket sits in one.
        assert_eq!(
            ids(&db, all.clone().priced_between(Some(0), Some(5000))).await,
            (owned(&["collar", "basket"]), 2)
        );
        assert_eq!(
            ids(&db, all.clone().priced_between(Some(5000), Some(7000))).await,
            (owned(&["collar"]), 1)
        );
        assert_eq!(
            ids(&db, all.clone().priced_between(Some(7000), Some(10000)))
                .await
                .1,
            0
        );
        // Either bound alone.
        assert_eq!(
            ids(&db, all.clone().priced_between(Some(2000), None)).await,
            (owned(&["collar", "basket"]), 2)
        );
        assert_eq!(
            ids(&db, all.priced_between(None, Some(1000))).await,
            (owned(&["collar"]), 1)
        );
    }

    #[tokio::test]
    async fn an_option_keeps_the_products_taking_its_value() {
        let db = catalog().await;
        let all = ProductQuery::default().sort(ProductSort::PriceAsc);
        assert_eq!(
            ids(&db, all.clone().with_option("Couleur", "rose")).await,
            (owned(&["collar"]), 1)
        );
        // Folded both sides: the link's key and the merchant's spelling.
        assert_eq!(
            ids(&db, all.clone().with_option("couleur", "Gris")).await,
            (owned(&["basket"]), 1)
        );
        assert_eq!(
            ids(&db, all.clone().with_option("Couleur", "vert")).await.1,
            0
        );
        // Two pairs must both hold.
        let both = all
            .clone()
            .with_option("Couleur", "Rose")
            .with_option("Couleur", "Bleu");
        assert_eq!(ids(&db, both).await, (owned(&["collar"]), 1));
        let apart = all
            .clone()
            .with_option("Couleur", "Rose")
            .with_option("Couleur", "Gris");
        assert_eq!(ids(&db, apart).await.1, 0);
        // With a band.
        let dear = all
            .with_option("Couleur", "Rose")
            .priced_between(Some(7000), None);
        assert_eq!(ids(&db, dear).await.1, 0);
    }

    #[tokio::test]
    async fn a_filtered_category_counts_what_it_lists() {
        let db = catalog().await;
        let category = ProductQuery::default()
            .in_category("cat")
            .sort(ProductSort::PriceAsc);
        // Unfiltered: the stored count and order.
        assert_eq!(
            ids(&db, category.clone()).await,
            (owned(&["collar", "basket"]), 2)
        );
        assert_eq!(
            ids(&db, category.clone().with_option("Couleur", "Gris")).await,
            (owned(&["basket"]), 1)
        );
        assert_eq!(
            ids(&db, category.priced_between(Some(5000), Some(7000))).await,
            (owned(&["collar"]), 1)
        );
    }

    #[tokio::test]
    async fn the_facet_counts_the_listing_and_sets_its_own_filter_aside() {
        let db = catalog().await;
        async fn values(db: &Db, query: &ProductQuery) -> Vec<(String, String, i64)> {
            published_option_values(db, query, "Couleur")
                .await
                .unwrap()
                .into_iter()
                .map(|v| (v.key, v.label, v.count))
                .collect()
        }
        let all = ProductQuery::default();
        assert_eq!(
            values(&db, &all).await,
            vec![
                ("bleu".into(), "Bleu".into(), 1),
                ("gris".into(), "Gris".into(), 1),
                ("rose".into(), "Rose".into(), 1),
            ]
        );
        // The active colour does not hide the others; the band does narrow.
        assert_eq!(
            values(&db, &all.clone().with_option("Couleur", "rose"))
                .await
                .len(),
            3
        );
        assert_eq!(
            values(&db, &all.clone().priced_between(Some(5000), Some(7000))).await,
            vec![
                ("bleu".into(), "Bleu".into(), 1),
                ("rose".into(), "Rose".into(), 1)
            ]
        );
        // A category, a search, an unknown option.
        let category = all
            .clone()
            .in_category("cat")
            .with_option("Couleur", "gris");
        assert_eq!(values(&db, &category).await.len(), 3);
        assert_eq!(
            values(&db, &all.clone().search("bask")).await,
            vec![("gris".into(), "Gris".into(), 1)]
        );
        assert!(
            published_option_values(&db, &all, "Taille")
                .await
                .unwrap()
                .is_empty()
        );
    }
}

#[cfg(test)]
mod probe;
