//! The atomic redemption counter.

use sqlx::SqlitePool;

use crate::view::DiscountView;

/// Consume one redemption slot; `false` means the limit is spent.
///
/// The guarded `UPDATE` under `BEGIN IMMEDIATE` is what makes two checkouts
/// racing the last slot serialize: exactly one sees `rows_affected == 1`. An
/// uncapped discount still counts — the admin list shows how often a code was
/// used either way.
pub async fn redeem(write_pool: &SqlitePool, discount: &DiscountView) -> anyhow::Result<bool> {
    let mut tx = write_pool.begin_with("BEGIN IMMEDIATE").await?;

    sqlx::query(
        "INSERT INTO discount_redemptions (discount_id, redeemed)
         VALUES (?, 0)
         ON CONFLICT (discount_id) DO NOTHING",
    )
    .bind(&discount.id)
    .execute(&mut *tx)
    .await?;

    let limit = discount.usage_limit.map(i64::from);
    let updated = sqlx::query(
        "UPDATE discount_redemptions
         SET redeemed = redeemed + 1
         WHERE discount_id = ? AND (? IS NULL OR redeemed < ?)",
    )
    .bind(&discount.id)
    .bind(limit)
    .bind(limit)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(updated.rows_affected() == 1)
}
