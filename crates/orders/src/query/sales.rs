//! A shop's sales over a period, summed from `orders_sales` and
//! `orders_sale_lines`: the rows the read-model subscription writes for each
//! order, one for the order and one a line, each read off the index on when
//! it was placed.
//!
//! What counts: an order counts from the moment it is placed (its money
//! held) and stops counting once cancelled. It counts in the period it was
//! placed in, and so do its refunds, whenever they were made: a period's
//! figures are those of the orders placed in it. Profit is told for the
//! orders that kept what every one of their goods cost, less what was
//! refunded on them.

use timada_core::db::{Connection, Db, FromRow, execute};

use crate::command::{OrderState, OrderStatus};

/// A span of time: from `from` (included) to `to` (excluded), in Unix
/// seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Period {
    pub from: i64,
    pub to: i64,
}

/// Writes what an order counts for, and what each of its lines does, inside
/// the transaction that writes its row. The lines never change once placed:
/// they are written once, and only stop counting with their order.
pub(crate) async fn settle_sale(
    tx: &Connection,
    tenant_id: &str,
    order: &OrderState,
) -> anyhow::Result<()> {
    let counted = order.status != OrderStatus::Cancelled;
    execute(
        tx,
        "INSERT INTO orders_sales (tenant_id, order_id, placed_at, currency, counted,
            total_cents, refunded_cents, item_count, profit_cents, source)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT (tenant_id, order_id) DO UPDATE SET
            counted = excluded.counted, refunded_cents = excluded.refunded_cents",
        (
            tenant_id,
            order.id.as_str(),
            order.placed_at as i64,
            order.currency.as_str(),
            counted,
            order.total_cents as i64,
            order.refunded_cents as i64,
            i64::from(order.item_count()),
            order.profit_cents(),
            order.source.as_ref().map(|source| source.channel()),
        ),
    )
    .await?;
    let profits = order
        .costs
        .as_ref()
        .and_then(|costs| costs.line_profits(&order.lines, order.total_cents));
    for (n, line) in order.lines.iter().enumerate() {
        execute(
            tx,
            "INSERT OR IGNORE INTO orders_sale_lines (tenant_id, order_id, line, placed_at,
                currency, counted, product_id, name, quantity, amount_cents, profit_cents)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            (
                tenant_id,
                order.id.as_str(),
                n as i64,
                order.placed_at as i64,
                order.currency.as_str(),
                counted,
                line.product_id.as_str(),
                line.name.as_str(),
                i64::from(line.quantity),
                line.amount_cents() as i64,
                profits.as_ref().map(|profits| profits[n]),
            ),
        )
        .await?;
    }
    execute(
        tx,
        "UPDATE orders_sale_lines SET counted = ?3
         WHERE tenant_id = ?1 AND order_id = ?2 AND counted <> ?3",
        (tenant_id, order.id.as_str(), counted),
    )
    .await?;
    Ok(())
}

/// A period's sales in one currency.
#[derive(Debug, Clone, Default, PartialEq, Eq, FromRow)]
pub struct SalesTotals {
    pub currency: String,
    /// The orders that count.
    pub orders: i64,
    /// What they were paid, all together: goods, less discounts, plus
    /// shipping, VAT included.
    pub revenue_cents: i64,
    /// What went back to their shoppers.
    pub refunded_cents: i64,
    /// How many things they sold.
    pub items: i64,
    /// The orders placed and cancelled since, which count for nothing else.
    pub cancelled: i64,
    /// How many of the orders that count know what they left.
    pub costed_orders: i64,
    /// What those left, their refunds taken off.
    pub profit_cents: i64,
}

impl SalesTotals {
    /// The revenue once the refunds went back.
    pub fn net_cents(&self) -> i64 {
        self.revenue_cents - self.refunded_cents
    }

    /// What an order was paid on average; `None` without one.
    pub fn average_basket_cents(&self) -> Option<i64> {
        (self.orders > 0).then(|| self.revenue_cents / self.orders)
    }
}

/// The sales of a tenant over `period`, a row a currency it sold in, the
/// one that brought the most first.
pub async fn sales_totals(
    db: &Db,
    tenant_id: &str,
    period: Period,
) -> anyhow::Result<Vec<SalesTotals>> {
    db.fetch_all(SALES_TOTALS, (tenant_id, period.from, period.to))
        .await
}

/// See [`sales_totals`].
pub const SALES_TOTALS: &str = "SELECT currency,
        coalesce(sum(counted), 0) AS orders,
        coalesce(sum(total_cents * counted), 0) AS revenue_cents,
        coalesce(sum(refunded_cents * counted), 0) AS refunded_cents,
        coalesce(sum(item_count * counted), 0) AS items,
        coalesce(sum(1 - counted), 0) AS cancelled,
        coalesce(sum(CASE WHEN counted = 1 AND profit_cents IS NOT NULL THEN 1 ELSE 0 END), 0)
            AS costed_orders,
        coalesce(sum(CASE WHEN counted = 1 AND profit_cents IS NOT NULL
            THEN profit_cents - refunded_cents ELSE 0 END), 0) AS profit_cents
     FROM orders_sales INDEXED BY orders_sales_placed
     WHERE tenant_id = ?1 AND placed_at >= ?2 AND placed_at < ?3
     GROUP BY currency ORDER BY revenue_cents DESC, currency";

/// What one channel brought over a period, in one currency.
#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
pub struct SourceSales {
    pub currency: String,
    /// `source / medium` ([`OrderSource::channel`](crate::OrderSource::channel));
    /// `None` gathers the orders whose storefront did not say.
    pub source: Option<String>,
    pub orders: i64,
    pub revenue_cents: i64,
    pub refunded_cents: i64,
    pub costed_orders: i64,
    pub profit_cents: i64,
}

/// The sales of a tenant over `period` by the channel that brought the
/// shopper, the one that brought the most first.
pub async fn sales_by_source(
    db: &Db,
    tenant_id: &str,
    period: Period,
) -> anyhow::Result<Vec<SourceSales>> {
    db.fetch_all(SALES_BY_SOURCE, (tenant_id, period.from, period.to))
        .await
}

/// See [`sales_by_source`].
pub const SALES_BY_SOURCE: &str = "SELECT currency, source,
        count(*) AS orders,
        sum(total_cents) AS revenue_cents,
        sum(refunded_cents) AS refunded_cents,
        sum(CASE WHEN profit_cents IS NOT NULL THEN 1 ELSE 0 END) AS costed_orders,
        sum(CASE WHEN profit_cents IS NOT NULL THEN profit_cents - refunded_cents ELSE 0 END)
            AS profit_cents
     FROM orders_sales INDEXED BY orders_sales_placed
     WHERE tenant_id = ?1 AND placed_at >= ?2 AND placed_at < ?3 AND counted = 1
     GROUP BY currency, source ORDER BY revenue_cents DESC, source";

/// What one product sold over a period, in one currency.
#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
pub struct ProductSales {
    pub currency: String,
    pub product_id: String,
    /// Its name on the last line sold.
    pub name: String,
    /// How many of it were sold.
    pub quantity: i64,
    /// What its lines were sold for, VAT included, before any discount on
    /// the order and without shipping.
    pub sales_cents: i64,
    /// How many of them know what they left.
    pub costed_quantity: i64,
    /// What those left; refunds are an order's and are not taken off here.
    pub profit_cents: i64,
}

/// The products a tenant sold the most of over `period`, by what they were
/// sold for, `limit` at most.
pub async fn best_sellers(
    db: &Db,
    tenant_id: &str,
    period: Period,
    limit: i64,
) -> anyhow::Result<Vec<ProductSales>> {
    db.fetch_all(BEST_SELLERS, (tenant_id, period.from, period.to, limit))
        .await
}

/// See [`best_sellers`].
pub const BEST_SELLERS: &str = "SELECT currency, product_id, max(name) AS name,
        sum(quantity) AS quantity,
        sum(amount_cents) AS sales_cents,
        sum(CASE WHEN profit_cents IS NOT NULL THEN quantity ELSE 0 END) AS costed_quantity,
        sum(coalesce(profit_cents, 0)) AS profit_cents
     FROM orders_sale_lines INDEXED BY orders_sale_lines_placed
     WHERE tenant_id = ?1 AND placed_at >= ?2 AND placed_at < ?3 AND counted = 1
     GROUP BY currency, product_id
     ORDER BY sales_cents DESC, product_id LIMIT ?4";

/// The next shop, after `after` in the order of their ids, that ever sold
/// something (`""` starts from the first): one step of a walk over the
/// shops with sales, a seek each, however many lines each one has.
pub async fn next_shop_with_sales(db: &Db, after: &str) -> anyhow::Result<Option<String>> {
    let next: Option<(String,)> = db.fetch_optional(NEXT_SHOP_WITH_SALES, (after,)).await?;
    Ok(next.map(|(tenant_id,)| tenant_id))
}

/// See [`next_shop_with_sales`].
pub const NEXT_SHOP_WITH_SALES: &str =
    "SELECT tenant_id FROM orders_sale_lines INDEXED BY orders_sale_lines_placed
     WHERE tenant_id > ?1 ORDER BY tenant_id LIMIT 1";

/// The ids of the products a tenant sold the most units of since `from`
/// (unix seconds), the best first, `limit` at most: what its best sellers
/// are ranked from. Whatever the currency: a unit is a unit.
pub async fn best_selling_products(
    db: &Db,
    tenant_id: &str,
    from: i64,
    limit: i64,
) -> anyhow::Result<Vec<String>> {
    let rows: Vec<(String,)> = db
        .fetch_all(BEST_SELLING_PRODUCTS, (tenant_id, from, limit))
        .await?;
    Ok(rows.into_iter().map(|(product_id,)| product_id).collect())
}

/// See [`best_selling_products`].
pub const BEST_SELLING_PRODUCTS: &str =
    "SELECT product_id FROM orders_sale_lines INDEXED BY orders_sale_lines_placed
     WHERE tenant_id = ?1 AND placed_at >= ?2 AND counted = 1
     GROUP BY product_id
     ORDER BY sum(quantity) DESC, product_id LIMIT ?3";

/// What the products of one category sold over a period, in one currency.
#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
pub struct CategorySales {
    pub currency: String,
    /// `None` gathers what was sold of products in no category.
    pub category_id: Option<String>,
    pub name: Option<String>,
    pub quantity: i64,
    pub sales_cents: i64,
    pub costed_quantity: i64,
    pub profit_cents: i64,
}

/// The sales of a tenant over `period` by category, as the catalog files
/// its products now (the catalog's `catalog_product_categories` and
/// `catalog_categories`, in the same database): a product in two categories
/// counts in both, so the rows do not add up to the period's sales.
pub async fn sales_by_category(
    db: &Db,
    tenant_id: &str,
    period: Period,
) -> anyhow::Result<Vec<CategorySales>> {
    db.fetch_all(SALES_BY_CATEGORY, (tenant_id, period.from, period.to))
        .await
}

/// See [`sales_by_category`].
pub const SALES_BY_CATEGORY: &str = "SELECT l.currency AS currency, c.id AS category_id,
        max(c.name) AS name,
        sum(l.quantity) AS quantity,
        sum(l.amount_cents) AS sales_cents,
        sum(CASE WHEN l.profit_cents IS NOT NULL THEN l.quantity ELSE 0 END) AS costed_quantity,
        sum(coalesce(l.profit_cents, 0)) AS profit_cents
     FROM orders_sale_lines l INDEXED BY orders_sale_lines_placed
     LEFT JOIN catalog_product_categories pc
        ON pc.tenant_id = l.tenant_id AND pc.product_id = l.product_id
     LEFT JOIN catalog_categories c ON c.tenant_id = pc.tenant_id AND c.id = pc.category_id
     WHERE l.tenant_id = ?1 AND l.placed_at >= ?2 AND l.placed_at < ?3 AND l.counted = 1
     GROUP BY l.currency, c.id
     ORDER BY sales_cents DESC, c.id";
