use evento::{Executor, ProjectionAggregate};
use timada_core::Money;

use crate::{aggregator::DiscountRedeemed, error::PromotionError};

use super::discount_id;

impl<E: Executor> super::Command<'_, E> {
    /// Uses a code on an order and returns what it takes off `subtotal`
    /// (never more than `max`). The redemption cap is enforced by inserting
    /// into `promotion_redemption` under `BEGIN IMMEDIATE`, so concurrent
    /// checkouts cannot over-redeem; the event is appended afterwards. A
    /// repeat for the same order (a retry, or a replayed checkout) claims
    /// nothing and returns the same amount.
    pub async fn redeem_discount(
        &self,
        code: &str,
        order_id: impl Into<String>,
        subtotal: &Money,
        max: &Money,
    ) -> Result<Money, PromotionError> {
        let order_id = order_id.into();
        if order_id.trim().is_empty() {
            return Err(PromotionError::Required("order_id"));
        }
        let Some(discount) = self.load_discount(discount_id(code)).await? else {
            return Err(PromotionError::UnknownCode);
        };
        if !discount.active {
            return Err(PromotionError::Inactive);
        }
        if let Some(until) = discount.valid_until
            && until < timada_core::time::now_unix_secs()?
        {
            return Err(PromotionError::Expired);
        }
        // Before the slot is claimed: a code worth nothing here keeps its slot.
        let amount = discount.kind.amount_off(subtotal, max)?;
        if !amount.is_positive() {
            return Err(PromotionError::NotApplicable);
        }

        let mut conn = self.db.acquire().await?;
        sqlx::query("BEGIN IMMEDIATE").execute(&mut *conn).await?;
        let outcome = claim_slot(
            &mut conn,
            &discount.code,
            &order_id,
            discount.max_redemptions,
        )
        .await;
        match outcome {
            Ok(Claim::AlreadyClaimed) => {
                sqlx::query("ROLLBACK").execute(&mut *conn).await?;
                return Ok(amount);
            }
            Ok(Claim::Claimed) => {
                sqlx::query("COMMIT").execute(&mut *conn).await?;
            }
            Err(err) => {
                // Best effort: the connection is dropped either way.
                let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
                return Err(err);
            }
        }
        drop(conn);

        discount
            .write()?
            .event(&DiscountRedeemed {
                order_id: order_id.clone(),
            })
            .commit(self.executor)
            .await?;
        tracing::info!(discount_id = %discount.id, %order_id, "discount redeemed");
        Ok(amount)
    }
}

enum Claim {
    Claimed,
    AlreadyClaimed,
}

async fn claim_slot(
    conn: &mut sqlx::SqliteConnection,
    code: &str,
    order_id: &str,
    max_redemptions: Option<u32>,
) -> Result<Claim, PromotionError> {
    let existing: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM promotion_redemption WHERE code = ? AND order_id = ?")
            .bind(code)
            .bind(order_id)
            .fetch_optional(&mut *conn)
            .await?;
    if existing.is_some() {
        return Ok(Claim::AlreadyClaimed);
    }
    if let Some(max) = max_redemptions {
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM promotion_redemption WHERE code = ?")
                .bind(code)
                .fetch_one(&mut *conn)
                .await?;
        if count >= i64::from(max) {
            return Err(PromotionError::LimitReached);
        }
    }
    sqlx::query("INSERT INTO promotion_redemption (code, order_id) VALUES (?, ?)")
        .bind(code)
        .bind(order_id)
        .execute(&mut *conn)
        .await?;
    Ok(Claim::Claimed)
}
