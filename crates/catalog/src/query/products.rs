//! Queries over `catalog_products`, for the products pages of a tenant.

use anyhow::Result;
use timada_core::db::{Db, FromRow};

use crate::command::{Price, ProductStatus};
use crate::value_object::format_money;

/// One row of `catalog_products`.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct ProductRow {
    pub tenant_id: String,
    pub id: String,
    pub slug: String,
    pub name: String,
    pub description: String,
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

use timada_core::like_contains as contains;

/// Which products a page lists: a search over slug and name, and one
/// category they must belong to. Both are optional; the default matches
/// every product of the tenant.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProductFilter<'a> {
    /// Matched anywhere in the slug or the name.
    pub q: Option<&'a str>,
    /// The id of a category the product must be in.
    pub category: Option<&'a str>,
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
}

use timada_core::db::fts_query as phrase;

/// Where a page of products comes from, `?2` being what was searched for.
/// With a phrase the full-text index narrows the tenant's products first;
/// without one, `?2` is a `LIKE` pattern or `NULL`.
macro_rules! products_from {
    (phrase) => {
        "FROM catalog_products p
         WHERE p.tenant_id = ?1
           AND p.id IN (SELECT id FROM catalog_products
                        WHERE tenant_id = ?1 AND fts_match(slug_search, name_search, ?2)) "
    };
    (pattern) => {
        "FROM catalog_products p
         WHERE p.tenant_id = ?1
           AND (?2 IS NULL OR p.slug LIKE ?2 ESCAPE '\\' OR p.name LIKE ?2 ESCAPE '\\') "
    };
}

/// The rest of the filter, shared by the list and the count.
macro_rules! products_where {
    () => {
        "AND (?3 IS NULL OR p.id IN (SELECT product_id FROM catalog_product_categories
                WHERE tenant_id = ?1 AND category_id = ?3)) "
    };
}

macro_rules! products_page {
    ($from:ident) => {
        concat!(
            "SELECT p.tenant_id, p.id, p.slug, p.name, p.description, p.amount_cents,
                p.currency, p.status, p.created_by, p.created_at, p.updated_at, p.thumbnail,
                p.image_count, p.min_amount_cents, p.max_amount_cents, p.variant_count,
                p.stock_state ",
            products_from!($from),
            products_where!(),
            "ORDER BY p.updated_at DESC, p.id DESC LIMIT ?4 OFFSET ?5"
        )
    };
}

macro_rules! products_count {
    ($from:ident) => {
        concat!("SELECT count(*) ", products_from!($from), products_where!())
    };
}

/// What `?2` is and which statements read it: the index for a phrase, the
/// rows for a short search or none.
fn search_of(filter: &ProductFilter<'_>) -> (Option<String>, bool) {
    match filter.q.and_then(phrase) {
        Some(phrase) => (Some(phrase), true),
        None => (filter.q.map(contains), false),
    }
}

/// The products of a tenant the filter matches, most recently changed first.
pub async fn products_of_tenant(
    db: &Db,
    tenant_id: &str,
    filter: &ProductFilter<'_>,
    offset: i64,
    limit: i64,
) -> Result<Vec<ProductRow>> {
    let (search, indexed) = search_of(filter);
    let statement = if indexed {
        products_page!(phrase)
    } else {
        products_page!(pattern)
    };
    db.fetch_all(
        statement,
        (tenant_id, search, filter.category, limit, offset),
    )
    .await
}

/// How many products [`products_of_tenant`] would list.
pub async fn count_products(db: &Db, tenant_id: &str, filter: &ProductFilter<'_>) -> Result<i64> {
    let (search, indexed) = search_of(filter);
    let statement = if indexed {
        products_count!(phrase)
    } else {
        products_count!(pattern)
    };
    db.fetch_scalar(statement, (tenant_id, search, filter.category))
        .await
}

/// One product of a tenant, by id.
pub async fn product_by_id(db: &Db, tenant_id: &str, id: &str) -> Result<Option<ProductRow>> {
    db.fetch_optional(
        "SELECT tenant_id, id, slug, name, description, amount_cents, currency, status,
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
        "SELECT tenant_id, id, slug, name, description, amount_cents, currency, status,
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
