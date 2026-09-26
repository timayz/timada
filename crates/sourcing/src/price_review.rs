//! « À valider »: the prices the guardrails would not let the shop set by
//! itself.
//!
//! A row's id derives from the product and the reason, so a product polled
//! forty times before anybody looks is **one** row with the latest figures,
//! not forty. Approving it sets the price; refusing it locks the price, so
//! the queue does not fill up again with the same item every few hours —
//! unlocking is a deliberate act.

use sqlx::SqlitePool;
use timada_core::Money;

use crate::price::ReviewReason;

/// What became of a review.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Settled {
    Approved,
    Rejected,
    /// Overtaken: the price moved, or the product stopped being sourced,
    /// before anybody looked.
    Stale,
}

impl Settled {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Approved => "approved",
            Self::Rejected => "rejected",
            Self::Stale => "stale",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "approved" => Some(Self::Approved),
            "rejected" => Some(Self::Rejected),
            "stale" => Some(Self::Stale),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Approved => "Appliqué",
            Self::Rejected => "Refusé",
            Self::Stale => "Caduc",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PriceReview {
    pub review_id: String,
    pub product_id: String,
    pub supplier_id: String,
    pub reason: ReviewReason,
    /// What the product sells for now; `None` when it has no price.
    pub current: Option<Money>,
    pub proposed: Money,
    /// One unit's cost, landed, as the rule read it.
    pub cost: Money,
    pub margin_bp: i64,
    pub raised_at: i64,
    pub settled_at: Option<i64>,
    pub settled_as: Option<Settled>,
}

impl PriceReview {
    pub fn is_open(&self) -> bool {
        self.settled_at.is_none()
    }

    /// What the change would do to the price, in basis points; `None` when
    /// there is no price to compare with.
    pub fn move_bp(&self) -> Option<i64> {
        let current = self.current.as_ref().filter(|money| money.minor > 0)?;
        let moved = self.proposed.minor - current.minor;
        Some(moved * 10_000 / current.minor)
    }
}

/// One open review per product and reason.
pub fn review_id(product_id: &str, reason: ReviewReason) -> String {
    timada_core::id::derived(&[product_id, reason.as_str()], "price-review")
}

/// Records — or refreshes — what an operator is being asked about.
#[allow(clippy::too_many_arguments)]
pub async fn raise_review(
    db: &SqlitePool,
    product_id: &str,
    supplier_id: &str,
    reason: ReviewReason,
    current: Option<&Money>,
    proposed: &Money,
    cost: &Money,
    margin_bp: i32,
    at: i64,
) -> sqlx::Result<String> {
    let id = review_id(product_id, reason);
    sqlx::query(
        "INSERT INTO sourcing_price_review
            (review_id, product_id, supplier_id, reason, current_minor, proposed_minor,
             currency, cost_minor, cost_currency, margin_bp, raised_at, settled_at, settled_as)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, NULL)
         ON CONFLICT (review_id) DO UPDATE SET
            supplier_id = excluded.supplier_id,
            current_minor = excluded.current_minor,
            proposed_minor = excluded.proposed_minor,
            currency = excluded.currency,
            cost_minor = excluded.cost_minor,
            cost_currency = excluded.cost_currency,
            margin_bp = excluded.margin_bp,
            raised_at = excluded.raised_at,
            settled_at = NULL,
            settled_as = NULL",
    )
    .bind(&id)
    .bind(product_id)
    .bind(supplier_id)
    .bind(reason.as_str())
    .bind(current.map(|money| money.minor))
    .bind(proposed.minor)
    .bind(&proposed.currency)
    .bind(cost.minor)
    .bind(&cost.currency)
    .bind(i64::from(margin_bp))
    .bind(at)
    .execute(db)
    .await?;
    Ok(id)
}

/// Marks a review settled. Answering one that somebody already answered
/// changes nothing, so two operators clicking at once is harmless.
pub async fn settle_review(
    db: &SqlitePool,
    review_id: &str,
    settled_as: Settled,
    at: i64,
) -> sqlx::Result<bool> {
    let done = sqlx::query(
        "UPDATE sourcing_price_review
         SET settled_at = ?2, settled_as = ?3
         WHERE review_id = ?1 AND settled_at IS NULL",
    )
    .bind(review_id)
    .bind(at)
    .bind(settled_as.as_str())
    .execute(db)
    .await?;
    Ok(done.rows_affected() > 0)
}

/// Closes whatever was open about a product: its price moved by itself, or
/// it is no longer bought anywhere, so the question no longer stands.
pub async fn close_reviews_of(db: &SqlitePool, product_id: &str, at: i64) -> sqlx::Result<u64> {
    let done = sqlx::query(
        "UPDATE sourcing_price_review
         SET settled_at = ?2, settled_as = 'stale'
         WHERE product_id = ?1 AND settled_at IS NULL",
    )
    .bind(product_id)
    .bind(at)
    .execute(db)
    .await?;
    Ok(done.rows_affected())
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ListReviews {
    /// `None` keeps the open ones, which is what the queue shows.
    pub settled: Option<bool>,
    pub supplier_id: Option<String>,
    pub limit: u32,
    pub offset: u32,
}

impl ListReviews {
    pub fn open(limit: u32, offset: u32) -> Self {
        Self {
            settled: Some(false),
            limit,
            offset,
            ..Self::default()
        }
    }
}

pub async fn list_reviews(db: &SqlitePool, filter: &ListReviews) -> sqlx::Result<Vec<PriceReview>> {
    let rows: Vec<RawReview> = sqlx::query_as(
        "SELECT review_id, product_id, supplier_id, reason, current_minor, proposed_minor,
                currency, cost_minor, cost_currency, margin_bp, raised_at, settled_at, settled_as
         FROM sourcing_price_review
         WHERE (?1 IS NULL
                OR (?1 = 0 AND settled_at IS NULL)
                OR (?1 = 1 AND settled_at IS NOT NULL))
           AND (?2 IS NULL OR supplier_id = ?2)
         ORDER BY settled_at IS NOT NULL, raised_at DESC, review_id
         LIMIT ?3 OFFSET ?4",
    )
    .bind(filter.settled)
    .bind(filter.supplier_id.as_deref())
    .bind(filter.limit.max(1))
    .bind(filter.offset)
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().map(Into::into).collect())
}

pub async fn count_reviews(db: &SqlitePool, filter: &ListReviews) -> sqlx::Result<i64> {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM sourcing_price_review
         WHERE (?1 IS NULL
                OR (?1 = 0 AND settled_at IS NULL)
                OR (?1 = 1 AND settled_at IS NOT NULL))
           AND (?2 IS NULL OR supplier_id = ?2)",
    )
    .bind(filter.settled)
    .bind(filter.supplier_id.as_deref())
    .fetch_one(db)
    .await
}

pub async fn review_by_id(db: &SqlitePool, review_id: &str) -> sqlx::Result<Option<PriceReview>> {
    let row: Option<RawReview> = sqlx::query_as(
        "SELECT review_id, product_id, supplier_id, reason, current_minor, proposed_minor,
                currency, cost_minor, cost_currency, margin_bp, raised_at, settled_at, settled_as
         FROM sourcing_price_review WHERE review_id = ?",
    )
    .bind(review_id)
    .fetch_optional(db)
    .await?;
    Ok(row.map(Into::into))
}

#[derive(sqlx::FromRow)]
struct RawReview {
    review_id: String,
    product_id: String,
    supplier_id: String,
    reason: String,
    current_minor: Option<i64>,
    proposed_minor: i64,
    currency: String,
    cost_minor: i64,
    cost_currency: String,
    margin_bp: i64,
    raised_at: i64,
    settled_at: Option<i64>,
    settled_as: Option<String>,
}

impl From<RawReview> for PriceReview {
    fn from(row: RawReview) -> Self {
        Self {
            reason: ReviewReason::parse(&row.reason).unwrap_or(ReviewReason::Jump),
            current: row
                .current_minor
                .map(|minor| Money::new(minor, row.currency.clone())),
            proposed: Money::new(row.proposed_minor, row.currency),
            cost: Money::new(row.cost_minor, row.cost_currency),
            settled_as: row.settled_as.as_deref().and_then(Settled::parse),
            review_id: row.review_id,
            product_id: row.product_id,
            supplier_id: row.supplier_id,
            margin_bp: row.margin_bp,
            raised_at: row.raised_at,
            settled_at: row.settled_at,
        }
    }
}
