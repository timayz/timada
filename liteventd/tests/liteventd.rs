use liteventd::{AggregatorEvent, Executor, SqlExecutor};
use ulid::Ulid;

use crate::account::{AccountCreated, CreateAccountInput, FullNameChanged};

mod account;

async fn load_fixtures<E: Executor>(executor: &E) -> anyhow::Result<(Ulid, Ulid)> {
    let john = account::create_account(
        executor,
        CreateAccountInput {
            fullname: "John doe".to_owned(),
        },
    )
    .await?;

    let albert = account::create_account(
        executor,
        CreateAccountInput {
            fullname: "Albert Dupon".to_owned(),
        },
    )
    .await?;

    account::change_fullname(
        executor,
        account::ChangeFullNameInput {
            id: albert,
            fullname: "Albert Dupont".to_owned(),
        },
    )
    .await?;

    account::transfer_money(
        executor,
        account::TransferMoneyInput {
            from: john,
            to: albert,
            value: 13.99,
        },
    )
    .await?;

    account::change_fullname(
        executor,
        account::ChangeFullNameInput {
            id: john,
            fullname: "John wick".to_owned(),
        },
    )
    .await?;

    Ok((john, albert))
}

#[tokio::test]
async fn subscribe_persistant() -> anyhow::Result<()> {
    let executor = SqlExecutor();
    let (john, albert) = load_fixtures(&executor).await?;
    let recv1 = liteventd::subscribe("sub1")
        .aggregator::<account::Account>()
        .start(&executor)
        .await?;

    let event = recv1.recv()?;
    assert_eq!(event.name, AccountCreated::name());
    assert_eq!(event.aggregate_id, john);

    liteventd::acknowledge(&executor, "sub1", &event).await?;

    let event = event.to_detail::<AccountCreated, ()>()?.unwrap();
    assert_eq!(event.data.fullname, "John doe");

    let event = recv1.recv()?;
    assert_eq!(event.name, AccountCreated::name());
    assert_eq!(event.aggregate_id, albert);

    liteventd::acknowledge(&executor, "sub1", &event).await?;

    let event = event.to_detail::<AccountCreated, ()>()?.unwrap();
    assert_eq!(event.data.fullname, "Albert Dupon");

    let recv2 = liteventd::subscribe("sub1")
        .aggregator::<account::Account>()
        .start(&executor)
        .await?;

    let event = recv2.recv()?;
    assert_eq!(event.name, AccountCreated::name());
    assert_eq!(event.aggregate_id, albert);

    liteventd::acknowledge(&executor, "sub1", &event).await?;

    let event = event.to_detail::<FullNameChanged, ()>()?.unwrap();
    assert_eq!(event.data.fullname, "Albert Dupont");

    let event = recv2.recv()?;
    assert_eq!(event.name, AccountCreated::name());
    assert_eq!(event.aggregate_id, john);

    liteventd::acknowledge(&executor, "sub1", &event).await?;

    let event = event.to_detail::<FullNameChanged, ()>()?.unwrap();
    assert_eq!(event.data.fullname, "John wick");

    Ok(())
}

#[tokio::test]
async fn subscribe_normal() -> anyhow::Result<()> {
    let executor = SqlExecutor();
    let (john, albert) = load_fixtures(&executor).await?;
    let recv1 = liteventd::subscribe("sub1")
        .mode(liteventd::SubscribeMode::Normal)
        .aggregator::<account::Account>()
        .start(&executor)
        .await?;

    let event = recv1.recv()?;
    assert_eq!(event.name, AccountCreated::name());
    assert_eq!(event.aggregate_id, john);

    let event = event.to_detail::<AccountCreated, ()>()?.unwrap();
    assert_eq!(event.data.fullname, "John doe");

    let event = recv1.recv()?;
    assert_eq!(event.name, AccountCreated::name());
    assert_eq!(event.aggregate_id, albert);

    let event = event.to_detail::<AccountCreated, ()>()?.unwrap();
    assert_eq!(event.data.fullname, "Albert Dupon");

    let recv2 = liteventd::subscribe("sub1")
        .aggregator::<account::Account>()
        .start(&executor)
        .await?;

    let event = recv2.recv()?;
    assert_eq!(event.name, AccountCreated::name());
    assert_eq!(event.aggregate_id, john);

    let event = event.to_detail::<AccountCreated, ()>()?.unwrap();
    assert_eq!(event.data.fullname, "John doe");

    Ok(())
}

#[tokio::test]
async fn subscribe_live() -> anyhow::Result<()> {
    let executor = SqlExecutor();
    let (john, albert) = load_fixtures(&executor).await?;
    let recv1 = liteventd::subscribe("sub1")
        .mode(liteventd::SubscribeMode::Live)
        .aggregator::<account::Account>()
        .start(&executor)
        .await?;

    let event = recv1.recv()?;
    assert_eq!(event.name, AccountCreated::name());
    assert_eq!(event.aggregate_id, john);

    let event = event.to_detail::<AccountCreated, ()>()?.unwrap();
    assert_eq!(event.data.fullname, "John doe");

    let event = recv1.recv()?;
    assert_eq!(event.name, AccountCreated::name());
    assert_eq!(event.aggregate_id, albert);

    let event = event.to_detail::<AccountCreated, ()>()?.unwrap();
    assert_eq!(event.data.fullname, "Albert Dupon");

    let recv2 = liteventd::subscribe("sub1")
        .aggregator::<account::Account>()
        .start(&executor)
        .await?;

    let event = recv2.recv()?;
    assert_eq!(event.name, AccountCreated::name());
    assert_eq!(event.aggregate_id, john);

    let event = event.to_detail::<FullNameChanged, ()>()?.unwrap();
    assert_eq!(event.data.fullname, "John wick");

    Ok(())
}

#[tokio::test]
async fn invalid_original_version() {
    let executor = SqlExecutor();

    let id = Ulid::new();
    let _ = liteventd::save_aggregator(account::Account::default(), id)
        .data(&AccountCreated {
            fullname: "John doe".to_owned(),
        })
        .unwrap()
        .commit(&executor)
        .await;

    let res = liteventd::save_aggregator(account::Account::default(), id)
        .data(&AccountCreated {
            fullname: "John doe".to_owned(),
        })
        .unwrap()
        .commit(&executor)
        .await;

    assert_eq!(res, Err(liteventd::SaveError::InvalidOriginalVersion));
}
