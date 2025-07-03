use liteventd::{Executor, sql::SqlExecutor};
use sqlx::{Database, IntoArguments, Pool, Sqlite, SqlitePool};

mod account;

async fn save<E: Executor>(executor: &E) -> anyhow::Result<()> {
    let user1 = account::create_account(executor, "user1").await?;
    let user2 = account::create_account(executor, "user2").await?;
    account::change_fullname(executor, user1, "john doe").await?;
    account::transfer_money(executor, user1, user2, 19.00).await?;

    let user1_account = liteventd::load::<account::Account, _>(executor, user1).await?;
    assert_eq!(user1_account.item.fullname, "john doe");
    assert_eq!(user1_account.item.balance, 100.00 - 19.00);

    let user2_account = liteventd::load::<account::Account, _>(executor, user2).await?;
    assert_eq!(user2_account.item.fullname, "user2");
    assert_eq!(user2_account.item.balance, 100.00 + 19.00);

    Ok(())
}

async fn invalid_original_version<E: Executor>(executor: &E) -> anyhow::Result<()> {
    let user1 = account::create_account(executor, "user1").await?;
    let res = liteventd::create(account::Account::default(), user1)
        .data(&account::AccountCreated {
            fullname: "john".to_owned(),
        })?
        .commit(executor)
        .await;

    assert_eq!(res, Err(liteventd::SaveError::InvalidOriginalVersion));

    Ok(())
}

#[tokio::test]
async fn sqlite_save() -> anyhow::Result<()> {
    let executor = create_sqlite_executor().await?;

    save(&executor).await
}

#[tokio::test]
async fn sqlite_invalid_original_version() -> anyhow::Result<()> {
    let executor = create_sqlite_executor().await?;

    invalid_original_version(&executor).await
}

async fn create_sqlite_executor() -> anyhow::Result<SqlExecutor<Sqlite>> {
    let pool = SqlitePool::connect("sqlite::memory:").await?;

    create_sql_executor(pool).await
}

async fn create_sql_executor<'a, D>(pool: Pool<D>) -> anyhow::Result<SqlExecutor<D>>
where
    D: Database,
    D::Arguments<'a>: IntoArguments<'a, D>,
    for<'c> &'c mut D::Connection: sqlx::Executor<'c, Database = D>,
{
    sqlx::query(SqlExecutor::<D>::get_database_schema())
        .execute(&pool)
        .await?;

    Ok(SqlExecutor(pool))
}
