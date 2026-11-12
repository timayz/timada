//! The subscription that keeps `reviews_reviews`, `reviews_product_ratings`,
//! `reviews_requests` and `reviews_request_policies` current for every
//! tenant.
//!
//! The handler reloads the strict write model of the review and rewrites its
//! row, then counts the product's published reviews again, in one
//! transaction, so delivery may repeat (the subscription is at-least-once)
//! without the numbers drifting. The row is written from the review alone:
//! what it says of the order (its reference, the product's name) the review
//! copied when it was submitted. One subscription follows every tenant's
//! streams: each row carries the tenant of the event that made it, and the
//! rows live in the store's own database ([`Db`]).
//!
//! It also follows the orders as they leave, for the review request each
//! one owes its buyer ([`review_requests_due`]): an order's row falls due
//! the shop's delay after the order left, under the shop's policy as this
//! subscription last read it from the policy's stream, and stops being due
//! once the request is mailed or the shop stops asking. A change of policy
//! moves the orders that may still be asked about; an order that left
//! earlier than the longest delay and the window could not fall due again
//! whatever the delay, so it is left as it is.

use evento::{
    Executor,
    metadata::{Event, RawEvent},
    subscription::{Context, SubscriptionBuilder},
};
use timada_core::db::{Db, FromRow, execute};
use timada_orders::aggregator::Fulfilled;

use crate::aggregator::{Review, ReviewRequest, ReviewRequests};
use crate::command::{
    ReviewState, review_projection, review_request_projection, review_requests_projection,
};
use crate::value_object::{
    DAY_SECS, DEFAULT_REQUEST_DELAY_DAYS, MAX_REQUEST_DELAY_DAYS, REQUEST_WINDOW_SECS,
};

/// The cursor identity of the read-model subscription, in the store.
pub const READ_MODEL_SUBSCRIPTION: &str = "timada-reviews/read-models";

/// Builds the subscription over every tenant and routing key. Start it with
/// `.start(&store)` and keep the handle for shutdown.
pub fn read_model_subscription<E: Executor>(db: Db) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(READ_MODEL_SUBSCRIPTION)
        .data(db)
        .handler(on_review_event())
        .handler(on_review_request_event())
        .handler(on_review_requests_event())
        .handler(on_order_fulfilled())
        .any_routing_key()
}

/// An order left: the request it owes falls due the shop's delay later, as
/// the shop's policy stands, unless it was mailed already.
#[evento::subscription]
async fn on_order_fulfilled<E: Executor>(
    context: &Context<'_, E>,
    event: Event<Fulfilled>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let due_at = delay_of(&db, &event.tenant)
        .await?
        .map(|delay| event.timestamp + delay);
    db.execute(
        "INSERT INTO reviews_requests (tenant_id, order_id, fulfilled_at, due_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (tenant_id, order_id) DO UPDATE SET fulfilled_at = excluded.fulfilled_at,
            due_at = CASE WHEN mailed_at IS NULL THEN excluded.due_at END",
        (
            event.tenant.as_str(),
            event.aggregate_id.as_str(),
            event.timestamp as i64,
            due_at.map(|at| at as i64),
        ),
    )
    .await?;
    Ok(())
}

/// How long after an order left the shop asks its buyer, in seconds, as this
/// subscription last read its policy: `None` when it never asks, the
/// default when the merchant never said.
async fn delay_of(db: &Db, tenant_id: &str) -> anyhow::Result<Option<u64>> {
    let row: Option<(Option<i64>,)> = db
        .fetch_optional(
            "SELECT delay_days FROM reviews_request_policies WHERE tenant_id = ?1",
            (tenant_id,),
        )
        .await?;
    let days = match row {
        None => Some(u64::from(DEFAULT_REQUEST_DELAY_DAYS)),
        Some((days,)) => days.map(|days| days.max(0) as u64),
    };
    Ok(days.map(|days| days * DAY_SECS))
}

/// The merchant changed when the shop asks, or stopped it asking: the policy
/// is kept, and the orders not asked about yet that left recently enough to
/// fall due under some delay are due again under this one.
#[evento::subscription_all]
async fn on_review_requests_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<ReviewRequests>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let days = review_requests_projection()
        .load(&event.aggregate_id)
        .tenant(&event.tenant)
        .execute(context.executor)
        .await?
        .unwrap_or_default()
        .delay_days();
    // Left before this, an order's request fell due and out of the window
    // already, whatever the delay.
    let reach = event
        .timestamp
        .saturating_sub(u64::from(MAX_REQUEST_DELAY_DAYS) * DAY_SECS + REQUEST_WINDOW_SECS);
    let env = (
        event.tenant.as_str(),
        days.map(i64::from),
        days.map(|days| (u64::from(days) * DAY_SECS) as i64),
        reach as i64,
    );
    db.write(&env, |tx, (tenant_id, days, delay, reach)| {
        Box::pin(async move {
            execute(
                tx,
                "INSERT INTO reviews_request_policies (tenant_id, delay_days) VALUES (?1, ?2)
                 ON CONFLICT (tenant_id) DO UPDATE SET delay_days = excluded.delay_days",
                (*tenant_id, *days),
            )
            .await?;
            execute(tx, MOVE_DUE, (*tenant_id, *delay, *reach)).await?;
            Ok(())
        })
    })
    .await
}

/// What a change of policy writes: the requests not mailed of the orders
/// that left since `?3` fall due `?2` seconds after, or never (`NULL`).
const MOVE_DUE: &str = "UPDATE reviews_requests INDEXED BY reviews_requests_fulfilled
    SET due_at = fulfilled_at + ?2
    WHERE tenant_id = ?1 AND fulfilled_at >= ?3 AND mailed_at IS NULL";

/// Keeps `reviews_requests` current: when the review request of an order
/// was mailed, for the order's page; a request mailed is no longer due.
#[evento::subscription_all]
async fn on_review_request_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<ReviewRequest>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let Some(request) = review_request_projection()
        .load(&event.aggregate_id)
        .tenant(&event.tenant)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    db.execute(
        "INSERT INTO reviews_requests (tenant_id, order_id, mailed_at) VALUES (?1, ?2, ?3)
         ON CONFLICT (tenant_id, order_id) DO UPDATE SET mailed_at = excluded.mailed_at,
            due_at = NULL",
        (
            event.tenant.as_str(),
            request.order_id.as_str(),
            request.mailed_at as i64,
        ),
    )
    .await?;
    Ok(())
}

/// Unix seconds the review request of an order was mailed at, if it was.
pub async fn review_request_mailed_at(
    db: &Db,
    tenant_id: &str,
    order_id: &str,
) -> anyhow::Result<Option<i64>> {
    let row: Option<(Option<i64>,)> = db
        .fetch_optional(
            "SELECT mailed_at FROM reviews_requests WHERE tenant_id = ?1 AND order_id = ?2",
            (tenant_id, order_id),
        )
        .await?;
    Ok(row.and_then(|(at,)| at))
}

/// A review request due and not mailed yet, as [`review_requests_due`]
/// lists it: where a walk of them stands once it read it.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct DueReviewRequest {
    pub tenant_id: String,
    pub order_id: String,
    /// Unix seconds: when the order left.
    pub fulfilled_at: i64,
    /// Unix seconds: when the request fell due, under the policy the read
    /// model knew.
    pub due_at: i64,
}

/// What [`review_requests_due`] reads: off the index of the requests due, in
/// its order, from where the walk stands (the bound on `due_at` alone lets
/// it start there rather than walk up to it).
pub const REQUESTS_DUE: &str = "SELECT tenant_id, order_id, fulfilled_at, due_at
    FROM reviews_requests INDEXED BY reviews_requests_due
    WHERE due_at IS NOT NULL AND due_at > ?1 AND due_at <= ?2
      AND due_at >= ?3 AND (due_at > ?3 OR tenant_id > ?4 OR (tenant_id = ?4 AND order_id > ?5))
    ORDER BY due_at, tenant_id, order_id
    LIMIT ?6";

/// The review requests of every tenant due at `now` (unix seconds) and not
/// mailed yet: fallen due within [`REQUEST_WINDOW_SECS`], the longest
/// overdue first, after `after` (the last one the walk read) when given.
/// The read model is a moment behind: a request mailed a moment ago may
/// still be listed, and its claim then finds it taken.
pub async fn review_requests_due(
    db: &Db,
    now: u64,
    after: Option<&DueReviewRequest>,
    limit: u32,
) -> anyhow::Result<Vec<DueReviewRequest>> {
    let (from, from_tenant, from_order) = after.map_or((i64::MIN, "", ""), |request| {
        (
            request.due_at,
            request.tenant_id.as_str(),
            request.order_id.as_str(),
        )
    });
    db.fetch_all(
        REQUESTS_DUE,
        (
            now.saturating_sub(REQUEST_WINDOW_SECS) as i64,
            now as i64,
            from,
            from_tenant,
            from_order,
            i64::from(limit),
        ),
    )
    .await
}

#[evento::subscription_all]
async fn on_review_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<Review>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let Some(review) = review_projection()
        .load(&event.aggregate_id)
        .tenant(&event.tenant)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    upsert_review(&db, &event.tenant, &review).await
}

/// Writes the row of a review as the write model sees it, and the rating of
/// its product, in one transaction.
pub(crate) async fn upsert_review(
    db: &Db,
    tenant_id: &str,
    review: &ReviewState,
) -> anyhow::Result<()> {
    let env = (tenant_id, review);
    db.write(&env, |tx, (tenant_id, review)| {
        Box::pin(async move {
            execute(
                tx,
                "INSERT INTO reviews_reviews (tenant_id, id, order_id, order_reference, product_id,
                    product_name, variant_label, rating, title, body, author_name, photo_count,
                    status, submitted_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
                 ON CONFLICT (tenant_id, id) DO UPDATE SET
                    rating = excluded.rating, title = excluded.title, body = excluded.body,
                    photo_count = excluded.photo_count, status = excluded.status,
                    updated_at = excluded.updated_at",
                (
                    *tenant_id,
                    review.id.as_str(),
                    review.order_id.as_str(),
                    review.order_reference.as_str(),
                    review.product_id.as_str(),
                    review.product_name.as_str(),
                    review.variant_label.as_deref(),
                    i64::from(review.rating),
                    review.title.as_deref(),
                    review.body.as_str(),
                    review.author_name.as_str(),
                    review.photos.len() as i64,
                    review.status.as_str(),
                    review.submitted_at as i64,
                    review.updated_at as i64,
                ),
            )
            .await?;
            execute(
                tx,
                "INSERT INTO reviews_product_ratings (tenant_id, product_id, review_count,
                    rating_sum)
                 SELECT ?1, ?2, COUNT(*), COALESCE(SUM(rating), 0) FROM reviews_reviews
                 WHERE tenant_id = ?1 AND product_id = ?2 AND status = 'published'
                 ON CONFLICT (tenant_id, product_id) DO UPDATE SET
                    review_count = excluded.review_count, rating_sum = excluded.rating_sum",
                (*tenant_id, review.product_id.as_str()),
            )
            .await?;
            Ok(())
        })
    })
    .await
}

#[cfg(test)]
mod tests {
    use timada_core::db::IntoParams;

    use super::*;

    async fn db() -> Db {
        let store = evento::Turso::in_memory_concurrent().await.unwrap();
        let mut conn = store.connect().unwrap();
        crate::MIGRATIONS
            .to_latest(&mut conn)
            .await
            .map_err(|(_, err)| err)
            .unwrap();
        Db::from(store)
    }

    /// The `detail` of each step of `sql`'s plan.
    async fn plan_of(db: &Db, sql: &str, params: impl IntoParams) -> Vec<String> {
        db.fetch_all::<(i64, i64, i64, String)>(&format!("EXPLAIN QUERY PLAN {sql}"), params)
            .await
            .unwrap()
            .into_iter()
            .map(|(_, _, _, detail)| detail)
            .collect()
    }

    /// Fails unless a step of `plan` searches `index` by `key` (an index read
    /// whole names it too), and when any step sorts.
    fn searches(plan: &[String], index: &str, key: &str) {
        assert!(
            plan.iter().any(|step| step.starts_with("SEARCH")
                && step.contains(index)
                && step.contains(key)),
            "no step searches {index} by {key}:\n{}",
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

    #[tokio::test]
    async fn the_statements_read_the_indexes_they_are_written_for() {
        let db = db().await;
        let due = plan_of(&db, REQUESTS_DUE, (0i64, 10i64, i64::MIN, "", "", 200i64)).await;
        searches(&due, "reviews_requests_due", "due_at>");
        let moved = plan_of(&db, MOVE_DUE, ("t1", Some(86400i64), 0i64)).await;
        searches(&moved, "reviews_requests_fulfilled", "tenant_id=?");
    }
}
