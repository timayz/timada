//! Human-readable order numbers ("C2026-000042"). The counter is contended,
//! so it lives in a write-side SQL table rather than being counted from
//! events; the number itself is recorded on the order by `OrderNumberAssigned`.

use sqlx::SqlitePool;

/// The number of an order, allocating the next one of the sequence the first
/// time. Keyed by order id: a retry of the same checkout gets the same
/// number. `BEGIN IMMEDIATE` serialises allocators. Numbers are unique, not
/// gapless — an order that fails to be placed leaves a hole.
pub async fn allocate_order_number(db: &SqlitePool, order_id: &str) -> anyhow::Result<String> {
    let this_year = timada_core::time::year_of(timada_core::time::now_unix_secs()?);
    let mut conn = db.acquire().await?;
    sqlx::query("BEGIN IMMEDIATE").execute(&mut *conn).await?;

    let allocated: Result<(i64, i64), sqlx::Error> = async {
        sqlx::query(
            "INSERT OR IGNORE INTO order_number (order_id, number, year)
             SELECT ?, COALESCE(MAX(number), 0) + 1, ? FROM order_number",
        )
        .bind(order_id)
        .bind(this_year)
        .execute(&mut *conn)
        .await?;
        sqlx::query_as::<_, (i64, i64)>("SELECT number, year FROM order_number WHERE order_id = ?")
            .bind(order_id)
            .fetch_one(&mut *conn)
            .await
    }
    .await;

    match allocated {
        Ok((number, year)) => {
            sqlx::query("COMMIT").execute(&mut *conn).await?;
            Ok(format!("C{year}-{number:06}"))
        }
        Err(err) => {
            if let Err(rollback) = sqlx::query("ROLLBACK").execute(&mut *conn).await {
                tracing::warn!(error = %rollback, "order number rollback failed");
            }
            Err(err.into())
        }
    }
}
