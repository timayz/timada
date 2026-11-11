//! Queries over `catalog_products`, for the products pages of a tenant.

use anyhow::Result;
use timada_core::db::{Db, FromRow, Tally};
use timada_core::page::{Cursor, Page, PageFrom, Side, read_page};

use crate::command::{Price, ProductStatus};
use crate::value_object::format_money;

/// One row of `catalog_products`.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct ProductRow {
    pub tenant_id: String,
    pub id: String,
    pub slug: String,
    pub name: String,
    pub amount_cents: Option<i64>,
    pub currency: Option<String>,
    pub status: String,
    pub created_by: String,
    /// Unix seconds.
    pub created_at: i64,
    /// Unix seconds.
    pub updated_at: i64,
    /// The file name of the first picture, when it has one.
    pub thumbnail: Option<String>,
    pub image_count: i64,
    /// The cheapest and the dearest a shopper may pay: equal when nothing
    /// varies, absent when the product has no price at all.
    pub min_amount_cents: Option<i64>,
    pub max_amount_cents: Option<i64>,
    pub variant_count: i64,
    /// `untracked` (sells without limit), `in_stock` or `sold_out`, as
    /// [`StockSummary`](crate::StockSummary) says it.
    pub stock_state: String,
}

impl ProductRow {
    pub fn price(&self) -> Option<Price> {
        Some(Price {
            amount_cents: self.amount_cents? as u64,
            currency: self.currency.clone()?,
        })
    }

    pub fn status(&self) -> ProductStatus {
        ProductStatus::parse(&self.status).unwrap_or_default()
    }

    /// The price as a listing shows it: one amount, or a range when the
    /// variants differ (`12.00–18.00 €`).
    pub fn price_label(&self) -> Option<String> {
        let currency = self.currency.as_deref()?;
        let (min, max) = (self.min_amount_cents?, self.max_amount_cents?);
        if min == max {
            return Some(format_money(min as u64, currency));
        }
        Some(timada_core::format_money_range(
            min as u64, max as u64, currency,
        ))
    }
}

use timada_core::db::tenant_fts_query;
use timada_core::like_contains as contains;

/// Which products a page lists: a search over slug and name, one category
/// they must belong to and one status they must have. All are optional; the
/// default matches every product of the tenant.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProductFilter<'a> {
    /// Matched anywhere in the slug or the name.
    pub q: Option<&'a str>,
    /// The id of a category the product must be in.
    pub category: Option<&'a str>,
    /// The status the product must have.
    pub status: Option<ProductStatus>,
}

impl<'a> ProductFilter<'a> {
    pub fn search(mut self, q: Option<&'a str>) -> Self {
        self.q = q.map(str::trim).filter(|q| !q.is_empty());
        self
    }

    pub fn category(mut self, category: Option<&'a str>) -> Self {
        self.category = category.map(str::trim).filter(|c| !c.is_empty());
        self
    }

    pub fn status(mut self, status: Option<ProductStatus>) -> Self {
        self.status = status;
        self
    }

    /// The status as the statements compare it (`?4`).
    fn status_value(&self) -> Option<&'static str> {
        self.status.map(ProductStatus::as_str)
    }
}

/// A statement over the products of a tenant (`?1`) the filter matches,
/// naming only the conditions the filter sets: `?2` the search, `?3` the
/// category, `?4` the status. A condition the filter leaves out is written
/// `?N IS NULL`, so the numbering holds; written `?N IS NULL OR …` instead,
/// it hid the indexes from the planner, which then sorted the tenant's
/// rows. A phrase is matched by the full-text
/// index straight away (asked inside an `IN (SELECT …)`, the same match
/// took a second to count), and carries the tenant's token so the index
/// answers with this tenant's products only.
fn products_statement(
    select: &str,
    index: Option<&str>,
    search: Search,
    filter: &ProductFilter<'_>,
    tail: &str,
) -> String {
    let index = index
        .map(|index| format!("INDEXED BY {index}"))
        .unwrap_or_default();
    let search = match search {
        Search::None => "?2 IS NULL",
        Search::Pattern => "(p.slug LIKE ?2 ESCAPE '\\' OR p.name LIKE ?2 ESCAPE '\\')",
        Search::Phrase => "fts_match(p.slug_search, p.name_search, ?2)",
    };
    let category = match filter.category {
        Some(_) => {
            "p.id IN (SELECT product_id FROM catalog_product_categories
                WHERE tenant_id = ?1 AND category_id = ?3)"
        }
        None => "?3 IS NULL",
    };
    let status = match filter.status {
        Some(_) => "p.status = ?4",
        None => "?4 IS NULL",
    };
    format!(
        "{select} FROM catalog_products p {index}
         WHERE p.tenant_id = ?1 AND {search} AND {category} AND {status} {tail}"
    )
}

/// What the columns of a page are.
const PAGE_COLUMNS: &str = "SELECT p.tenant_id, p.id, p.slug, p.name, p.amount_cents, p.currency,
    p.status, p.created_by, p.created_at, p.updated_at, p.thumbnail, p.image_count,
    p.min_amount_cents, p.max_amount_cents, p.variant_count, p.stock_state";

/// A page of products on either side of `(?5, ?6)` (`updated_at`, `id`),
/// `?7` rows at most: walking down from it (`older`), as the list reads, or
/// up, nearest first. Either way the order is the one of
/// `catalog_products_changed` and `catalog_products_status_changed`, so a
/// page reads its rows off the index instead of sorting the tenant's. The
/// index is named: left to choose, the planner sorted the tenant's rows
/// off the primary key as soon as the cursor bounded `updated_at`, and
/// without that bound a deep page walked the index down from the top. A
/// phrase is left to the full-text index, which narrows to a few rows.
/// Public for the query-plan probe.
#[doc(hidden)]
pub fn page_statement(search: Search, filter: &ProductFilter<'_>, older: bool) -> String {
    let index = match (search, filter.status) {
        (Search::Phrase, _) => None,
        (_, Some(_)) => Some("catalog_products_status_changed"),
        (_, None) => Some("catalog_products_changed"),
    };
    let tail = if older {
        "AND p.updated_at <= ?5 AND (p.updated_at < ?5 OR p.id < ?6)
         ORDER BY p.updated_at DESC, p.id DESC LIMIT ?7"
    } else {
        "AND p.updated_at >= ?5 AND (p.updated_at > ?5 OR p.id > ?6)
         ORDER BY p.updated_at ASC, p.id ASC LIMIT ?7"
    };
    products_statement(PAGE_COLUMNS, index, search, filter, tail)
}

/// How `?2` searches.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Search {
    /// Nothing was searched for.
    None,
    /// A `LIKE` pattern, for a search too short for the index.
    Pattern,
    /// A phrase for the full-text index.
    Phrase,
}

/// What `?2` is and how the statements read it: the index for a phrase,
/// the rows for a short search.
fn search_of(tenant_id: &str, filter: &ProductFilter<'_>) -> (Option<String>, Search) {
    match (
        filter.q,
        filter.q.and_then(|q| tenant_fts_query(tenant_id, q)),
    ) {
        (_, Some(phrase)) => (Some(phrase), Search::Phrase),
        (Some(q), None) => (Some(contains(q)), Search::Pattern),
        (None, None) => (None, Search::None),
    }
}

/// Where a row stands in the list: its last change and its id.
fn cursor_of(row: &ProductRow) -> Cursor {
    Cursor {
        at: row.updated_at,
        id: row.id.clone(),
    }
}

/// A page of the products of a tenant the filter matches, most recently
/// changed first, `limit` rows at most (see [`timada_core::page`]).
pub async fn products_of_tenant(
    db: &Db,
    tenant_id: &str,
    filter: &ProductFilter<'_>,
    from: PageFrom<'_>,
    limit: i64,
) -> Result<Page<ProductRow>> {
    let (search, how) = search_of(tenant_id, filter);
    let older_statement = page_statement(how, filter, true);
    let newer_statement = page_statement(how, filter, false);
    let read = |side: Side, cursor: Option<Cursor>, n: i64| {
        let statement = match side {
            Side::Older => older_statement.as_str(),
            Side::Newer => newer_statement.as_str(),
        };
        let (at, id) = cursor
            .map(|cursor| (cursor.at, cursor.id))
            .unwrap_or((i64::MAX, String::new()));
        db.fetch_all::<ProductRow>(
            statement,
            (
                tenant_id.to_owned(),
                search.clone(),
                filter.category.map(str::to_owned),
                filter.status_value(),
                at,
                id,
                n,
            ),
        )
    };
    read_page(from, limit, read, cursor_of).await
}

/// How many products [`products_of_tenant`] lists over all its pages: an
/// index count for a status, the rows only for a search or a category.
pub async fn count_products(db: &Db, tenant_id: &str, filter: &ProductFilter<'_>) -> Result<i64> {
    let (search, how) = search_of(tenant_id, filter);
    db.fetch_scalar(
        &products_statement("SELECT count(*)", None, how, filter, ""),
        (tenant_id, search, filter.category, filter.status_value()),
    )
    .await
}

/// How many products a tenant has, whatever their status: what its home
/// shows. Kept by the read model as it writes the rows.
pub const PRODUCTS: Tally = Tally {
    table: "catalog_tallies",
    name: "products",
    counted: "SELECT count(*) FROM catalog_products WHERE tenant_id = ?1 AND id = ?2",
};

/// How many products a tenant has, read from [`PRODUCTS`].
pub async fn product_count(db: &Db, tenant_id: &str) -> Result<i64> {
    PRODUCTS.read(db, tenant_id).await
}

/// One product of a tenant, by id.
pub async fn product_by_id(db: &Db, tenant_id: &str, id: &str) -> Result<Option<ProductRow>> {
    db.fetch_optional(
        "SELECT tenant_id, id, slug, name, amount_cents, currency, status,
            created_by, created_at, updated_at, thumbnail, image_count,
            min_amount_cents, max_amount_cents, variant_count, stock_state
         FROM catalog_products WHERE tenant_id = ?1 AND id = ?2",
        (tenant_id, id),
    )
    .await
}

/// A tenant's published products made after `since` (unix seconds), newest
/// first: what a newsletter shows as just in.
pub async fn new_products(
    db: &Db,
    tenant_id: &str,
    since: i64,
    limit: i64,
) -> Result<Vec<ProductRow>> {
    db.fetch_all(
        "SELECT tenant_id, id, slug, name, amount_cents, currency, status,
            created_by, created_at, updated_at, thumbnail, image_count,
            min_amount_cents, max_amount_cents, variant_count, stock_state
         FROM catalog_products
         WHERE tenant_id = ?1 AND status = 'published' AND created_at > ?2
         ORDER BY created_at DESC, id DESC
         LIMIT ?3",
        (tenant_id, since, limit),
    )
    .await
}
