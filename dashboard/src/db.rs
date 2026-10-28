//! SQLite pools and the schema migrators.
//!
//! Two files: the store (every tenant's events, the snapshots, the
//! subscription cursors and the Accord consensus journal) and the read models
//! (the tables the pages read). Each gets two pools for `serve`, as in
//! imkitchen: a read-only pool sized for concurrent reads and a
//! single-connection write pool, so every write is serialized up front
//! instead of failing with SQLITE_BUSY mid-transaction. The write side runs
//! WAL with `synchronous = FULL`: an acked write must survive power loss.

use std::ops::Deref;
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use anyhow::{Context, Result};
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous,
};
use sqlx_migrator::{Info, Migrator};

/// The read models' read-only pool, as registered in topcoat's app context:
/// `let db: &ReadPool = app_context(cx);` then use it as a `SqlitePool`.
pub struct ReadPool(pub SqlitePool);
/// The read models' single-connection write pool, as registered in topcoat's
/// app context.
pub struct WritePool(pub SqlitePool);

impl Deref for ReadPool {
    type Target = SqlitePool;
    fn deref(&self) -> &SqlitePool {
        &self.0
    }
}

impl Deref for WritePool {
    type Target = SqlitePool;
    fn deref(&self) -> &SqlitePool {
        &self.0
    }
}

/// `busy_timeout` for the `serve` pools.
const SERVE_BUSY_TIMEOUT: Duration = Duration::from_secs(5);
/// `busy_timeout` for `migrate` / `reset`: opening a database with a large
/// leftover `-wal` from an unclean shutdown can take well over 5s on slow
/// storage.
pub const MAINTENANCE_BUSY_TIMEOUT: Duration = Duration::from_secs(60);

/// Per-connection options shared by every pool. sqlx re-applies these pragmas
/// every time it opens a connection, including replacements after the idle
/// timeout. Journal mode and synchronous are write-side concerns, set by the
/// pools that own the file (`PRAGMA journal_mode = WAL` fails on a read-only
/// connection).
fn base_options(database_url: &str, busy_timeout: Duration) -> Result<SqliteConnectOptions> {
    Ok(SqliteConnectOptions::from_str(database_url)?
        .busy_timeout(busy_timeout)
        .foreign_keys(true)
        .pragma("wal_autocheckpoint", "1000")
        .pragma("journal_size_limit", "67108864") // 64 MiB cap on -wal growth
        .pragma("cache_size", "-20000") // 20 MiB page cache
        .pragma("mmap_size", "268435456") // 256 MiB memory-mapped I/O
        .pragma("temp_store", "memory"))
}

/// Read-only pool for `serve`, sized by `database.max_connections`. Open the
/// write pool first: a read-only connection cannot create the file.
pub async fn create_read_pool(database_url: &str, max_connections: u32) -> Result<SqlitePool> {
    let options = base_options(database_url, SERVE_BUSY_TIMEOUT)?.read_only(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(max_connections)
        .min_connections(0)
        .idle_timeout(Some(Duration::from_secs(30)))
        .connect_with(options)
        .await
        .with_context(|| format!("cannot open {database_url} read-only"))?;

    tracing::info!(max_connections, "read pool ready");
    Ok(pool)
}

/// What a write pool promises once a commit returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Durability {
    /// On disk (`synchronous = FULL`): the events, the consensus journal
    /// and the read models.
    Durable,
    /// In the operating system's hands (`synchronous = NORMAL`): a power
    /// cut may lose the last commits, never corrupt the file. For the
    /// subscription cursors and the snapshots, which are redone or rebuilt.
    Relaxed,
}

/// Read-write pool for `serve`: one connection, so writes never contend
/// with each other and queue in order instead of backing off on SQLite's
/// lock. On the store, the Accord journal shares it with the events.
pub async fn create_write_pool(database_url: &str, durability: Durability) -> Result<SqlitePool> {
    prepare_database_dir(database_url)?;
    let options = base_options(database_url, SERVE_BUSY_TIMEOUT)?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(match durability {
            Durability::Durable => SqliteSynchronous::Full,
            Durability::Relaxed => SqliteSynchronous::Normal,
        });

    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .with_context(|| format!("cannot open {database_url}"))?;

    tracing::info!(?durability, "write pool ready (1 connection)");
    Ok(pool)
}

/// Pool for `migrate` / `reset`: the only pool open, so it owns the WAL and
/// synchronous settings too.
pub async fn create_pool(
    database_url: &str,
    max_connections: u32,
    busy_timeout: Duration,
) -> Result<SqlitePool> {
    prepare_database_dir(database_url)?;
    let options = base_options(database_url, busy_timeout)?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Full);

    let pool = SqlitePoolOptions::new()
        .max_connections(max_connections)
        .connect_with(options)
        .await
        .with_context(|| format!("cannot open {database_url}"))?;

    tracing::info!(max_connections, "maintenance pool ready");
    Ok(pool)
}

/// Creates the directory the database file lives in. `sqlx::Sqlite::create_database`
/// does not, so `migrate` calls this before it.
pub fn prepare_database_dir(database_url: &str) -> Result<()> {
    let options = SqliteConnectOptions::from_str(database_url)?;
    let filename: &Path = options.get_filename();
    if let Some(parent) = filename.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
    }
    Ok(())
}

/// The store's schema: events, snapshots, subscribers and the Accord journal.
pub fn store_migrator() -> Result<Migrator<sqlx::Sqlite>, sqlx_migrator::Error> {
    evento_sql_migrator::new::<sqlx::Sqlite>()
}

/// The bounded contexts' read models.
pub fn read_model_migrator() -> Result<Migrator<sqlx::Sqlite>, sqlx_migrator::Error> {
    let mut migrator = Migrator::default();
    migrator.add_migrations(timada_identity::migrations())?;
    migrator.add_migrations(timada_tenancy::migrations())?;
    migrator.add_migrations(timada_catalog::migrations())?;
    migrator.add_migrations(timada_orders::migrations())?;
    migrator.add_migrations(timada_shoppers::migrations())?;
    migrator.add_migrations(timada_discounts::migrations())?;
    migrator.add_migrations(timada_reviews::migrations())?;
    migrator.add_migrations(timada_support::migrations())?;
    migrator.add_migrations(timada_newsletter::migrations())?;
    migrator.add_migrations(timada_automations::migrations())?;
    migrator.add_migrations(timada_invoicing::migrations())?;
    migrator.add_migrations(timada_aliexpress::migrations())?;
    Ok(migrator)
}

/// Creates the file if needed and applies the pending migrations of
/// `migrator` to it, on a pool of its own.
pub async fn migrate(url: &str, migrator: Migrator<sqlx::Sqlite>) -> Result<()> {
    use sqlx::migrate::MigrateDatabase;
    use sqlx_migrator::{Migrate, Plan};

    prepare_database_dir(url)?;
    if !sqlx::Sqlite::database_exists(url).await? {
        tracing::info!(url, "database does not exist, creating it");
        sqlx::Sqlite::create_database(url).await?;
    }
    let pool = create_pool(url, 1, MAINTENANCE_BUSY_TIMEOUT).await?;
    let mut conn = pool.acquire().await?;
    // Collapse any leftover `-wal` from a previous unclean shutdown up front, so
    // the migration transaction does not hit SQLITE_BUSY while WAL recovery runs
    // lazily. The long busy_timeout on this pool rides out slow storage.
    sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
        .execute(&mut *conn)
        .await?;
    migrator.run(&mut *conn, &Plan::apply_all()).await?;
    drop(conn);
    pool.close().await;
    Ok(())
}

/// Drops the file, if there, then creates and migrates it again.
pub async fn recreate(url: &str, migrator: Migrator<sqlx::Sqlite>) -> Result<()> {
    use sqlx::migrate::MigrateDatabase;

    if sqlx::Sqlite::database_exists(url).await? {
        tracing::warn!(url, "dropping existing database");
        sqlx::Sqlite::drop_database(url).await?;
    }
    migrate(url, migrator).await
}

/// Recreates the read models from nothing: they are derived data, which the
/// subscriptions fill again from the store.
pub async fn recreate_read_models(url: &str) -> Result<()> {
    recreate(url, read_model_migrator()?).await
}

/// Empties the store of its events, snapshots and subscription cursors and
/// keeps the consensus journal (the `accord_*` tables): what a node does
/// before copying a peer's store (see `evento-accord`'s operations guide,
/// "Opening a node by joining").
pub async fn empty_store(write_pool: &SqlitePool) -> Result<()> {
    let mut tx = write_pool.begin().await?;
    sqlx::query("DELETE FROM event")
        .execute(&mut *tx)
        .await
        .context("cannot empty the events")?;
    sqlx::query("DELETE FROM snapshot")
        .execute(&mut *tx)
        .await
        .context("cannot empty the snapshots")?;
    sqlx::query("DELETE FROM subscriber")
        .execute(&mut *tx)
        .await
        .context("cannot empty the subscription cursors")?;
    tx.commit().await?;
    Ok(())
}
