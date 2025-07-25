use liteventd::Executor;
use ulid::Ulid;

pub async fn save<E: Executor>(executor: &E) -> anyhow::Result<()> {
    // let user1 = account::create_account(executor, "user1").await?;
    // let user2 = Ulid::new();
    // liteventd::create_with_id(account::Account::default(), user2)
    //     .metadata(&Metadata { request_id: 1 })?
    //     .data(&account::AccountCreated {
    //         fullname: "user2".to_owned(),
    //     })?
    //     .data(&account::AccountCreated {
    //         fullname: "albert dupont".to_owned(),
    //     })?
    //     .commit(executor)
    //     .await?;
    // account::change_fullname(executor, user1, "john doe").await?;
    //
    // let user1_account = liteventd::load::<account::Account, _>(executor, user1).await?;
    // assert_eq!(user1_account.item.fullname, "john doe");
    //
    // let user2_account = liteventd::load::<account::Account, _>(executor, user2).await?;
    // assert_eq!(user2_account.item.fullname, "albert dupont");

    Ok(())
}

pub async fn invalid_original_version<E: Executor>(executor: &E) -> anyhow::Result<()> {
    // let user1 = account::create_account(executor, "user1").await?;
    // let res = liteventd::create_with_id(account::Account::default(), user1)
    //     .metadata(&Metadata { request_id: 1 })?
    //     .data(&account::AccountCreated {
    //         fullname: "john".to_owned(),
    //     })?
    //     .commit(executor)
    //     .await;
    //
    // assert_eq!(
    //     res.map_err(|e| e.to_string()),
    //     Err(liteventd::WriteError::InvalidOriginalVersion.to_string())
    // );

    Ok(())
}
