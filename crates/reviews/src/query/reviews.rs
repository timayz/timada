//! Queries over `reviews_reviews` and `reviews_product_ratings`, for the
//! review pages of a tenant.

use timada_core::db::{Db, FromRow};

use crate::value_object::ReviewStatus;

/// One row of `reviews_reviews`.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct ReviewRow {
    pub tenant_id: String,
    pub id: String,
    pub order_id: String,
    pub order_reference: String,
    pub product_id: String,
    /// The product's name when it was sold.
    pub product_name: String,
    pub variant_label: Option<String>,
    pub rating: i64,
    pub title: Option<String>,
    pub body: String,
    pub author_name: String,
    pub photo_count: i64,
    pub status: String,
    /// Unix seconds.
    pub submitted_at: i64,
    /// Unix seconds.
    pub updated_at: i64,
}

/// Which reviews a list shows; `None` is any.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReviewFilter {
    pub status: Option<ReviewStatus>,
    pub rating: Option<u8>,
    pub product_id: Option<String>,
}

const COLUMNS: &str = "tenant_id, id, order_id, order_reference, product_id, product_name,
    variant_label, rating, title, body, author_name, photo_count, status, submitted_at,
    updated_at";

/// The tenant's reviews matching `filter`, newest first.
pub async fn reviews_of_tenant(
    db: &Db,
    tenant_id: &str,
    filter: &ReviewFilter,
    limit: u32,
    offset: u32,
) -> anyhow::Result<Vec<ReviewRow>> {
    db.fetch_all(
        &format!(
            "SELECT {COLUMNS} FROM reviews_reviews
             WHERE tenant_id = ?1 AND (?2 IS NULL OR status = ?2)
                AND (?3 IS NULL OR rating = ?3) AND (?4 IS NULL OR product_id = ?4)
             ORDER BY submitted_at DESC, id DESC
             LIMIT ?5 OFFSET ?6"
        ),
        (
            tenant_id,
            filter.status.map(ReviewStatus::as_str),
            filter.rating.map(i64::from),
            filter.product_id.as_deref(),
            i64::from(limit),
            i64::from(offset),
        ),
    )
    .await
}

/// How many of the tenant's reviews match `filter`.
pub async fn count_reviews(db: &Db, tenant_id: &str, filter: &ReviewFilter) -> anyhow::Result<u64> {
    let count: i64 = db
        .fetch_scalar(
            "SELECT COUNT(*) FROM reviews_reviews
             WHERE tenant_id = ?1 AND (?2 IS NULL OR status = ?2)
                AND (?3 IS NULL OR rating = ?3) AND (?4 IS NULL OR product_id = ?4)",
            (
                tenant_id,
                filter.status.map(ReviewStatus::as_str),
                filter.rating.map(i64::from),
                filter.product_id.as_deref(),
            ),
        )
        .await?;
    Ok(count as u64)
}

/// A product's published reviews, counted.
#[derive(Debug, Clone, Copy, Default, FromRow, PartialEq, Eq)]
pub struct ProductRating {
    pub review_count: i64,
    pub rating_sum: i64,
}

impl ProductRating {
    /// The average, to one decimal (`4.3`); `None` without a review.
    pub fn average(&self) -> Option<f64> {
        (self.review_count > 0)
            .then(|| (self.rating_sum as f64 / self.review_count as f64 * 10.0).round() / 10.0)
    }
}

/// The rating of a product: zero reviews when it has none published.
pub async fn product_rating(
    db: &Db,
    tenant_id: &str,
    product_id: &str,
) -> anyhow::Result<ProductRating> {
    Ok(db
        .fetch_optional(
            "SELECT review_count, rating_sum FROM reviews_product_ratings
             WHERE tenant_id = ?1 AND product_id = ?2",
            (tenant_id, product_id),
        )
        .await?
        .unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_average_has_one_decimal() {
        let rating = ProductRating {
            review_count: 3,
            rating_sum: 13,
        };
        assert_eq!(rating.average(), Some(4.3));
        assert_eq!(ProductRating::default().average(), None);
    }
}
