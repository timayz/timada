//! SQLite pools and the schema migrator.
//!
//! One file holds the events, the projections and the Accord consensus
//! journal, so every connection runs WAL with `synchronous = FULL`: an acked
//! write must survive power loss. The remaining pragmas follow imkitchen.

use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use anyhow::{Context, Result};
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous,
};
use sqlx_migrator::Migrator;

/// `busy_timeout` for the `serve` pool.
pub const SERVE_BUSY_TIMEOUT: Duration = Duration::from_secs(5);
/// `busy_timeout` for `migrate` / `reset`: opening a database with a large
/// leftover `-wal` from an unclean shutdown can take well over 5s on slow
/// storage.
pub const MAINTENANCE_BUSY_TIMEOUT: Duration = Duration::from_secs(60);

/// Per-connection options. sqlx re-applies these pragmas every time it opens a
/// connection, including replacements after the idle timeout.
fn connect_options(database_url: &str, busy_timeout: Duration) -> Result<SqliteConnectOptions> {
    Ok(SqliteConnectOptions::from_str(database_url)?
        .create_if_missing(true)
        .busy_timeout(busy_timeout)
        .foreign_keys(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Full)
        .pragma("wal_autocheckpoint", "1000")
        .pragma("journal_size_limit", "67108864") // 64 MiB cap on -wal growth
        .pragma("cache_size", "-20000") // 20 MiB page cache
        .pragma("mmap_size", "268435456") // 256 MiB memory-mapped I/O
        .pragma("temp_store", "memory"))
}

/// Opens a pool on `database_url`, creating the file and its parent directory
/// when missing. Idle connections close after 30s so a quiet node holds no
/// SQLite threads.
pub async fn create_pool(
    database_url: &str,
    max_connections: u32,
    busy_timeout: Duration,
) -> Result<SqlitePool> {
    prepare_database_dir(database_url)?;
    let options = connect_options(database_url, busy_timeout)?;

    let pool = SqlitePoolOptions::new()
        .max_connections(max_connections)
        .min_connections(0)
        .idle_timeout(Some(Duration::from_secs(30)))
        .connect_with(options)
        .await
        .with_context(|| format!("cannot open {database_url}"))?;

    tracing::info!(max_connections, "sqlite pool ready");
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
