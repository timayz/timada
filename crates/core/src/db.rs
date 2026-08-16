//! SQLite connection pools.
//!
//! Long-running servers use the pair `create_read_pool` + `create_write_pool`;
//! short-lived CLI commands and tests use the single `create_pool`. All
//! pragmas are set via `SqliteConnectOptions` so sqlx re-applies them on
//! replacement connections.

use anyhow::Result;
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous,
};
use std::str::FromStr;
use std::time::Duration;

/// Base connect options shared by every pool.
fn base_options(database_url: &str, busy_timeout: Duration) -> Result<SqliteConnectOptions> {
    Ok(SqliteConnectOptions::from_str(database_url)?
        .busy_timeout(busy_timeout)
        .foreign_keys(true)
        .pragma("wal_autocheckpoint", "1000") // explicit; write pool overrides to 0 (Litestream owns checkpointing)
        .pragma("journal_size_limit", "67108864")
        .pragma("cache_size", "-20000")
        .pragma("mmap_size", "268435456") // 256 MiB memory-mapped I/O — cuts read syscalls
        .pragma("temp_store", "memory"))
}

/// Read-only pool, optimized for concurrent reads.
///
/// Sized to CPU cores. Does NOT set `journal_mode` or `synchronous` — those
/// are write-side concerns and would fail on a read-only connection. The DB
/// file's journal mode is owned by the write pool.
pub async fn create_read_pool(database_url: &str, max_connections: u32) -> Result<SqlitePool> {
    let options = base_options(database_url, Duration::from_millis(5000))?.read_only(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(max_connections)
        .connect_with(options)
        .await?;

    tracing::info!(
        "Created read-only pool with {} max connections",
        max_connections
    );
    Ok(pool)
}

/// Read-write pool, single connection to serialize writes and avoid SQLITE_BUSY.
///
/// All write transactions go through this pool with `BEGIN IMMEDIATE`, so the
/// reserved lock is taken up front instead of upgrading mid-transaction.
pub async fn create_write_pool(database_url: &str) -> Result<SqlitePool> {
    let options = base_options(database_url, Duration::from_millis(5000))?
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        // Disable SQLite's automatic WAL checkpoint at runtime: when Litestream
        // is replicating it must be the sole owner of checkpointing.
        // journal_size_limit still bounds -wal growth as a backstop.
        .pragma("wal_autocheckpoint", "0");

    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await?;

    tracing::info!("Created read-write pool with 1 max connection");
    Ok(pool)
}

/// Standard pool for CLI commands (migrate, seed, tests).
///
/// Single-pool setup, so it owns the WAL/synchronous settings. Long
/// `busy_timeout` because recovering a large leftover `-wal` on slow storage
/// can take well over the 5s the serve pools use.
pub async fn create_pool(database_url: &str, max_connections: u32) -> Result<SqlitePool> {
    let options = base_options(database_url, Duration::from_secs(60))?
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal);

    let pool = SqlitePoolOptions::new()
        .max_connections(max_connections)
        .connect_with(options)
        .await?;

    tracing::info!("Created pool with {} max connections", max_connections);
    Ok(pool)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn write_pool_sets_wal_and_disables_autocheckpoint() {
        let dir = std::env::temp_dir().join(format!("timada-core-db-{}", crate::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let url = format!("sqlite://{}?mode=rwc", dir.join("t.db").display());

        let write = create_write_pool(&url).await.unwrap();
        let (journal_mode,): (String,) = sqlx::query_as("PRAGMA journal_mode")
            .fetch_one(&write)
            .await
            .unwrap();
        assert_eq!(journal_mode.to_lowercase(), "wal");
        let (autocheckpoint,): (i64,) = sqlx::query_as("PRAGMA wal_autocheckpoint")
            .fetch_one(&write)
            .await
            .unwrap();
        assert_eq!(autocheckpoint, 0);

        let read = create_read_pool(&url, 2).await.unwrap();
        let err = sqlx::query("CREATE TABLE nope (id INTEGER)")
            .execute(&read)
            .await;
        assert!(err.is_err(), "read pool must be read-only");

        write.close().await;
        read.close().await;
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
