//! Sequential document numbers.
//!
//! Invoice numbers cannot be ULIDs: tax authorities want a sequence, and a
//! sequence is exactly the thing an event store does not hand you. So it lives
//! in one SQL counter row per document kind, incremented inside a
//! `BEGIN IMMEDIATE` transaction on the single-connection write pool — the
//! reserved lock is taken up front, so two concurrent allocations queue instead
//! of colliding on the upgrade.
//!
//! **The crash window.** A number is allocated in its own committed
//! transaction, before the event that uses it is appended. If that append then
//! fails, the number is burned and the sequence has a gap. That is the safe
//! direction to fail: uniqueness and monotonicity hold, which is what makes a
//! number usable as a reference, and only gaplessness is lost. Closing the gap
//! needs a reconciliation pass that walks the issued events against the
//! counter — a follow-up, not something to fake by rolling the counter back
//! (which would re-issue a number that may already be on a printed document).

use sqlx::SqlitePool;

/// Allocate the next number for `kind` and format it as `{prefix}-{000001}`.
///
/// `kind` is the counter's identity — `"invoice"` and `"credit_note"` count
/// independently — and `prefix` is only how the result is spelled.
pub(crate) async fn next_number(
    write_pool: &SqlitePool,
    kind: &str,
    prefix: &str,
) -> anyhow::Result<String> {
    let mut tx = write_pool.begin_with("BEGIN IMMEDIATE").await?;

    sqlx::query(
        "INSERT INTO invoice_sequences (kind, next_value) VALUES (?, 1) \
         ON CONFLICT(kind) DO NOTHING",
    )
    .bind(kind)
    .execute(&mut *tx)
    .await?;

    let (next,): (i64,) = sqlx::query_as("SELECT next_value FROM invoice_sequences WHERE kind = ?")
        .bind(kind)
        .fetch_one(&mut *tx)
        .await?;

    sqlx::query("UPDATE invoice_sequences SET next_value = next_value + 1 WHERE kind = ?")
        .bind(kind)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;

    Ok(format!("{prefix}-{next:06}"))
}

#[cfg(test)]
mod tests {
    use sqlx_migrator::migrator::{Info as _, Migrate as _, Migrator, Plan};
    use timada_core::new_id;

    use super::*;

    /// A temp-file database with this crate's read-side tables applied.
    async fn test_pool() -> anyhow::Result<(std::path::PathBuf, SqlitePool)> {
        let dir = std::env::temp_dir().join(format!("timada-invoice-numbering-{}", new_id()));
        std::fs::create_dir_all(&dir)?;
        let url = format!("sqlite://{}?mode=rwc", dir.join("test.db").display());
        let pool = timada_core::db::create_pool(&url, 1).await?;

        let mut migrator = Migrator::<sqlx::Sqlite>::default();
        migrator.add_migrations(crate::migrations())?;
        let mut conn = pool.acquire().await?;
        migrator.run(&mut *conn, &Plan::apply_all()).await?;
        drop(conn);

        Ok((dir, pool))
    }

    #[tokio::test]
    async fn numbers_run_in_sequence_and_each_kind_counts_on_its_own() -> anyhow::Result<()> {
        let (dir, pool) = test_pool().await?;

        assert_eq!(next_number(&pool, "invoice", "INV").await?, "INV-000001");
        assert_eq!(next_number(&pool, "invoice", "INV").await?, "INV-000002");

        // A different kind starts its own count, unaffected by the two above.
        assert_eq!(next_number(&pool, "credit_note", "CN").await?, "CN-000001");
        assert_eq!(next_number(&pool, "invoice", "INV").await?, "INV-000003");
        assert_eq!(next_number(&pool, "credit_note", "CN").await?, "CN-000002");

        pool.close().await;
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }
}
