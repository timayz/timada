//! In-memory SQLite executor for integration tests (feature `test-support`).

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Sqlite, SqlitePool};
use sqlx_migrator::{Info, Migrate, Migration, Plan};

/// Opens a fresh `:memory:` database, applies the evento schema plus the given
/// context migrations, and returns the executor together with the pool for
/// SQL read models.
///
/// A single connection keeps the in-memory database alive and visible to every
/// query for the lifetime of the pool.
pub async fn memory_executor(
    migrations: Vec<Box<dyn Migration<Sqlite>>>,
) -> anyhow::Result<(evento::Sqlite, SqlitePool)> {
    let options = SqliteConnectOptions::new()
        .filename(":memory:")
        .foreign_keys(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await?;

    let mut migrator = evento::sql_migrator::new::<Sqlite>()?;
    migrator.add_migrations(migrations)?;
    let mut conn = pool.acquire().await?;
    migrator.run(&mut *conn, &Plan::apply_all()).await?;
    drop(conn);

    Ok((pool.clone().into(), pool))
}
