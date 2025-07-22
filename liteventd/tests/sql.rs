#[path = "liteventd/mod.rs"]
mod liteventd_test;

use liteventd::sql::Sql;
use sqlx::{Any, Database, Pool, any::install_default_drivers, migrate::MigrateDatabase};

#[tokio::test]
async fn sqlite_save() -> anyhow::Result<()> {
    let executor = create_sqlite_executor("save").await?;

    liteventd_test::save(&executor).await
}

#[tokio::test]
async fn sqlite_invalid_original_version() -> anyhow::Result<()> {
    let executor = create_sqlite_executor("invalid_original_version").await?;

    liteventd_test::invalid_original_version(&executor).await
}

async fn create_sqlite_executor(key: impl Into<String>) -> anyhow::Result<Sql<sqlx::Sqlite>> {
    let key = key.into();
    let url = format!("sqlite:../target/liteventd_{key}.db");

    create_executor(url).await
}

async fn create_executor<DB: Database>(url: impl Into<String>) -> anyhow::Result<Sql<DB>>
where
    for<'q> DB::Arguments<'q>: sqlx::IntoArguments<'q, DB>,
    for<'c> &'c mut DB::Connection: sqlx::Executor<'c, Database = DB>,
{
    install_default_drivers();

    let url = url.into();

    let _ = Any::drop_database(&url).await;
    Any::create_database(&url).await?;

    let pool = Pool::<DB>::connect(&url).await?;
    let schema = Sql::<DB>::get_schema();

    sqlx::query(&schema).execute(&pool).await?;

    Ok(pool.into())
}
