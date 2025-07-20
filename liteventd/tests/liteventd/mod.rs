mod account;

use liteventd::Executor;
use ulid::Ulid;

pub async fn save<E: Executor>(executor: &E) -> anyhow::Result<()> {
    let user1 = account::create_account(executor, "user1").await?;
    let user2 = Ulid::new();
    liteventd::create(account::Account::default(), user2)
        .data(&account::AccountCreated {
            fullname: "user2".to_owned(),
        })?
        .data(&account::AccountCreated {
            fullname: "albert dupont".to_owned(),
        })?
        .commit(executor)
        .await?;
    account::change_fullname(executor, user1, "john doe").await?;
    account::transfer_money(executor, user1, user2, 19.00).await?;

    let user1_account = liteventd::load::<account::Account, _>(executor, user1).await?;
    assert_eq!(user1_account.item.fullname, "john doe");
    assert_eq!(user1_account.item.balance, 100.00 - 19.00);

    let user2_account = liteventd::load::<account::Account, _>(executor, user2).await?;
    assert_eq!(user2_account.item.fullname, "albert dupont");
    assert_eq!(user2_account.item.balance, 100.00 + 19.00);

    Ok(())
}

pub async fn invalid_original_version<E: Executor>(executor: &E) -> anyhow::Result<()> {
    let user1 = account::create_account(executor, "user1").await?;
    let res = liteventd::create(account::Account::default(), user1)
        .data(&account::AccountCreated {
            fullname: "john".to_owned(),
        })?
        .commit(executor)
        .await;

    assert_eq!(
        res.map_err(|e| e.to_string()),
        Err(liteventd::WriteError::InvalidOriginalVersion.to_string())
    );

    Ok(())
}
