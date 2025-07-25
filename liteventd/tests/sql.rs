#[path = "liteventd.rs"]
mod liteventd_test;

#[path = "cursor.rs"]
mod cursor_test;

use liteventd::{
    Event, Executor,
    cursor::{Args, ReadResult},
    sql::{Reader, Sql},
};
use sea_query::{Query, SqliteQueryBuilder};
use sea_query_binder::SqlxBinder;
use sqlx::{
    Any, Database, Pool, SqlitePool, any::install_default_drivers, migrate::MigrateDatabase,
};

use crate::cursor_test::assert_read_result;

// #[tokio::test]
// async fn sqlite_save() -> anyhow::Result<()> {
//     let executor = create_sqlite_executor("save").await?;
//
//     liteventd_test::save(&executor).await
// }
//
// #[tokio::test]
// async fn sqlite_invalid_original_version() -> anyhow::Result<()> {
//     let executor = create_sqlite_executor("invalid_original_version").await?;
//
//     liteventd_test::invalid_original_version(&executor).await
// }
//
//
#[tokio::test]
async fn forward_asc() -> anyhow::Result<()> {
    let pool = create_sqlite_pool("forward_asc").await?;
    let data = get_data(&pool).await?;
    let args = Args::forward(10, None);

    let result = read(&pool, args.clone()).await?;

    assert_read_result(args, data, result)?;

    Ok(())
}

async fn read<DB>(pool: &Pool<DB>, args: Args) -> anyhow::Result<ReadResult<Event>>
where
    DB: Database,
    for<'c> &'c mut DB::Connection: sqlx::Executor<'c, Database = DB>,
    sea_query_binder::SqlxValues: for<'q> sqlx::IntoArguments<'q, DB>,
    usize: sqlx::ColumnIndex<DB::Row>,
    Event: for<'r> sqlx::FromRow<'r, DB::Row>,
{
    let statement = Query::select()
        .columns([
            liteventd::sql::Event::Id,
            liteventd::sql::Event::Name,
            liteventd::sql::Event::AggregateType,
            liteventd::sql::Event::AggregateId,
            liteventd::sql::Event::Version,
            liteventd::sql::Event::Data,
            liteventd::sql::Event::Metadata,
            liteventd::sql::Event::RoutingKey,
            liteventd::sql::Event::Timestamp,
        ])
        .from(liteventd::sql::Event::Table)
        .to_owned();

    Ok(Reader::new(statement)
        .args(args)
        .execute::<_, crate::Event, _>(pool)
        .await?)
}

async fn get_data<DB>(pool: &Pool<DB>) -> anyhow::Result<Vec<Event>>
where
    DB: Database,
    for<'c> &'c mut DB::Connection: sqlx::Executor<'c, Database = DB>,
    sea_query_binder::SqlxValues: for<'q> sqlx::IntoArguments<'q, DB>,
{
    let data = cursor_test::get_data();
    let mut query = Query::insert()
        .into_table(liteventd::sql::Event::Table)
        .columns([
            liteventd::sql::Event::Id,
            liteventd::sql::Event::Name,
            liteventd::sql::Event::Data,
            liteventd::sql::Event::Metadata,
            liteventd::sql::Event::AggregateType,
            liteventd::sql::Event::AggregateId,
            liteventd::sql::Event::Version,
            liteventd::sql::Event::RoutingKey,
            liteventd::sql::Event::Timestamp,
        ])
        .to_owned();

    for event in data.clone() {
        query.values_panic([
            event.id.to_string().into(),
            event.name.into(),
            event.data.into(),
            event.metadata.into(),
            event.aggregate_type.into(),
            event.aggregate_id.to_string().into(),
            event.version.into(),
            event.routing_key.into(),
            event.timestamp.into(),
        ]);
    }

    let (sql, values) = match DB::NAME {
        "SQLite" => query.build_sqlx(SqliteQueryBuilder),
        name => panic!("'{name}' not supported, consider using SQLite"),
    };

    sqlx::query_with::<DB, _>(&sql, values)
        .execute(pool)
        .await?;

    Ok(data)
}

async fn create_sqlite_executor(key: impl Into<String>) -> anyhow::Result<Sql<sqlx::Sqlite>> {
    Ok(create_sqlite_pool(key).await?.into())
}

async fn create_sqlite_pool(key: impl Into<String>) -> anyhow::Result<SqlitePool> {
    let key = key.into();
    let url = format!("sqlite:../target/tmp/test_sql_{key}.db");

    create_pool(url).await
}

async fn create_pool<DB: Database>(url: impl Into<String>) -> anyhow::Result<Pool<DB>>
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

    Ok(pool)
}
