//! SQLite pools and the schema migrator.
//!
//! Two pools for `serve`, as in imkitchen: a read-only pool sized for
//! concurrent reads and a single-connection write pool, so every write is
//! serialized up front instead of failing with SQLITE_BUSY mid-transaction.
//! One file holds the events, the projections and the Accord consensus
//! journal, so the write side runs WAL with `synchronous = FULL`: an acked
//! write must survive power loss.

use std::ops::Deref;
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use anyhow::{Context, Result};
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous,
};
use sqlx_migrator::Migrator;

/// The read-only pool, as registered in topcoat's app context:
/// `let db: &ReadPool = app_context(cx);` then use it as a `SqlitePool`.
pub struct ReadPool(pub SqlitePool);
/// The single-connection write pool, as registered in topcoat's app context.
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

/// Read-write pool for `serve`: one connection, so writes never contend with
/// each other. The Accord journal shares it with the event store.
pub async fn create_write_pool(database_url: &str) -> Result<SqlitePool> {
    prepare_database_dir(database_url)?;
    let options = base_options(database_url, SERVE_BUSY_TIMEOUT)?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Full);

    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .with_context(|| format!("cannot open {database_url}"))?;

    tracing::info!("write pool ready (1 connection)");
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

/// evento's schema (events, snapshots, subscribers, Accord journal) followed by
/// the application's own migrations. Add those with `add_migrations` here.
pub fn migrator<DB: sqlx::Database>() -> Result<Migrator<DB>, sqlx_migrator::Error>
where
    evento_sql_migrator::InitMigration: sqlx_migrator::Migration<DB>,
    evento_sql_migrator::M0002: sqlx_migrator::Migration<DB>,
    evento_sql_migrator::M0003: sqlx_migrator::Migration<DB>,
    evento_sql_migrator::M0004: sqlx_migrator::Migration<DB>,
    evento_sql_migrator::M0005: sqlx_migrator::Migration<DB>,
    evento_sql_migrator::M0006: sqlx_migrator::Migration<DB>,
    evento_sql_migrator::M0007: sqlx_migrator::Migration<DB>,
    evento_sql_migrator::AccordMigration: sqlx_migrator::Migration<DB>,
{
    evento_sql_migrator::new::<DB>()
}
