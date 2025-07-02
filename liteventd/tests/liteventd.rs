use liteventd::{Executor, SqlExecutor};

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
    let res = liteventd::save_aggregator(account::Account::default(), user1)
        .data(&account::AccountCreated {
            fullname: "john".to_owned(),
        })?
        .commit(executor)
        .await;

    assert_eq!(res, Err(liteventd::SaveError::InvalidOriginalVersion));

    Ok(())
}

#[tokio::test]
async fn sql_save() -> anyhow::Result<()> {
    let executor = SqlExecutor();

    save(&executor).await
}

#[tokio::test]
async fn sql_invalid_original_version() -> anyhow::Result<()> {
    let executor = SqlExecutor();

    invalid_original_version(&executor).await
}
