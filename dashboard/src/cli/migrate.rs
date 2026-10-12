use anyhow::Result;
use sqlx::migrate::MigrateDatabase;
use sqlx_migrator::{Migrate, Plan};

use crate::config::Config;
use crate::db;

pub async fn migrate(config: Config) -> Result<()> {
    let url = &config.database.url;
    tracing::info!("running database migrations");

    db::prepare_database_dir(url)?;
    if !sqlx::Sqlite::database_exists(url).await? {
        tracing::info!(url, "database does not exist, creating it");
        sqlx::Sqlite::create_database(url).await?;
    }

    let pool = db::create_pool(url, 1, db::MAINTENANCE_BUSY_TIMEOUT).await?;
    let mut conn = pool.acquire().await?;

    // Collapse any leftover `-wal` from a previous unclean shutdown up front, so
    // the migration transaction does not hit SQLITE_BUSY while WAL recovery runs
    // lazily. The long busy_timeout on this pool rides out slow storage.
    sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
        .execute(&mut *conn)
        .await?;

    db::migrator()?.run(&mut *conn, &Plan::apply_all()).await?;
    drop(conn);
    pool.close().await;

    tracing::info!("migrations completed");
    Ok(())
}

/// Drops the database file and the tenant databases, then migrates a fresh one. Development only: the
/// subcommand does not exist in release builds.
#[cfg(debug_assertions)]
pub async fn reset(config: Config) -> Result<()> {
    let url = &config.database.url;

    if sqlx::Sqlite::database_exists(url).await? {
        tracing::warn!(url, "dropping existing database");
        sqlx::Sqlite::drop_database(url).await?;
    } else {
        tracing::info!(url, "database does not exist, nothing to drop");
    }
    let tenants = &config.tenants.dir;
    if tenants.is_dir() {
        tracing::warn!(dir = %tenants.display(), "removing the tenant databases");
        std::fs::remove_dir_all(tenants)?;
    }

    migrate(config).await?;

    tracing::info!("database reset completed");
    Ok(())
}
