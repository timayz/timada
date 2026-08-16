//! SQLite pool initialization and database migrations.
//!
//! One database file holds both the evento event store and the read-model
//! tables. Long-running servers use the pair [`create_read_pool`] +
//! [`create_write_pool`]; short-lived contexts (migrations, CLI, tests) use
//! the single [`create_pool`].

use anyhow::Result;
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous,
};
use sqlx_migrator::migrator::{Info as _, Migrate as _, Migrator};
use std::str::FromStr;
use std::time::Duration;

/// Base connect options shared by every pool.
///
/// Returns options with all per-connection pragmas configured. sqlx re-applies
/// these every time it opens a new connection, including replacement
/// connections after idle timeout — which is the behavior you want.
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
/// are write-side concerns and `PRAGMA journal_mode = WAL` would fail on a
/// read-only connection anyway. The DB file's journal mode is set by the
/// write pool.
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

/// Read-write pool, single connection to serialize writes and avoid
/// SQLITE_BUSY.
///
/// All write transactions go through this pool. Use `BEGIN IMMEDIATE` for any
/// transaction that will write, so it grabs the reserved lock up front instead
/// of upgrading mid-transaction (which is what causes most BUSY errors).
pub async fn create_write_pool(database_url: &str) -> Result<SqlitePool> {
    let options = base_options(database_url, Duration::from_millis(5000))?
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        // Disable SQLite's automatic WAL checkpoint at runtime. When Litestream is
        // replicating, it must be the sole owner of checkpointing — an app-initiated
        // checkpoint can discard WAL frames Litestream hasn't shipped yet and break the
        // replication chain. journal_size_limit still bounds -wal growth as a backstop.
        // (migrate's create_pool keeps autocheckpoint — it runs before the Litestream
        // sidecar starts.)
        .pragma("wal_autocheckpoint", "0");

    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await?;

    tracing::info!("Created read-write pool with 1 max connection");
    Ok(pool)
}

/// Standard pool for CLI contexts (migrate, import, tests).
///
/// Single-pool setup, so it owns the WAL/synchronous settings.
///
/// Uses a long `busy_timeout` because migrate/reset can open a database with a
/// large leftover `-wal` from a previous unclean shutdown; recovering it on
/// slow storage can take well over the 5s the serve pools use.
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

/// Apply all pending database migrations: the evento event-store schema
/// (owned by evento) followed by the timada read-model tables.
///
/// Run this at startup on a [`create_pool`] pool before opening the serve
/// pools.
pub async fn migrate(pool: &SqlitePool) -> Result<()> {
    let mut conn = pool.acquire().await?;

    evento::sql_migrator::new::<sqlx::Sqlite>()?
        .run(&mut *conn, &evento::migrator::Plan::apply_all())
        .await?;

    let migrations = crate::read_model::migrations();
    if !migrations.is_empty() {
        let mut migrator = Migrator::<sqlx::Sqlite>::default();
        migrator.add_migrations(migrations)?;
        migrator
            .run(&mut *conn, &sqlx_migrator::migrator::Plan::apply_all())
            .await?;
    }

    tracing::info!("Database migrations applied");
    Ok(())
}
