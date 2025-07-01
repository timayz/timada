use liteventd::{Aggregator, Event, EventDetail, Executor, SqlExecutor, SubscribeHandler};
use liteventd_macros::handle;
use std::{collections::HashMap, sync::Arc};
use ulid::Ulid;
use validator::Validate;

use crate::account::{
    Account, AccountCreated, AccountCredited, AccountDebited, FullNameChanged,
    MoneyTransferCancelled, MoneyTransferSucceeded, MoneyTransferred, Reason,
};

type AccountEventDetail<D> = EventDetail<D, HashMap<String, String>>;

#[derive(Debug, Validate)]
pub struct CreateAccountInput {
    #[validate(length(min = 3, max = 20))]
    pub fullname: String,
}

pub async fn create_account<E: Executor>(
    executor: &E,
    input: CreateAccountInput,
) -> anyhow::Result<Ulid> {
    input.validate()?;

    let id = Ulid::new();

    liteventd::save_aggregator(Account::default(), id)
        .data(&AccountCreated {
            fullname: input.fullname,
        })?
        .commit(executor)
        .await?;

    Ok(id)
}

#[derive(Debug, Validate)]
pub struct ChangeFullNameInput {
    pub id: Ulid,

    #[validate(length(min = 3, max = 20))]
    pub fullname: String,
}

pub async fn change_fullname<E: Executor>(
    executor: &E,
    input: ChangeFullNameInput,
) -> anyhow::Result<()> {
    input.validate()?;

    let account = liteventd::load::<Account, _>(executor, input.id).await?;

    liteventd::save(account, input.id)
        .data(&FullNameChanged {
            fullname: input.fullname,
        })?
        .commit(executor)
        .await?;

    Ok(())
}

#[derive(Debug, Validate)]
pub struct TransferMoneyInput {
    pub from: Ulid,
    pub to: Ulid,
    #[validate(range(exclusive_min = 0.0))]
    pub value: f32,
}

pub async fn transfer_money<E: Executor>(
    executor: &E,
    input: TransferMoneyInput,
) -> anyhow::Result<()> {
    input.validate()?;

    if input.from == input.to {
        return Err(anyhow::anyhow!(
            "not possible to transfer money to the same account"
        ));
    }

    let from_account = liteventd::load::<Account, _>(executor, input.from).await?;
    let to_account = liteventd::load::<Account, _>(executor, input.to).await?;

    liteventd::save(from_account, input.from)
        .data(&MoneyTransferred {
            from_id: input.from,
            to_id: to_account.item.id,
            transaction_id: Ulid::new(),
            value: input.value,
        })?
        .commit(executor)
        .await?;

    Ok(())
}

#[handle(money_transferred, account_debited, account_credited)]
struct Command;

impl Command {
    async fn money_transferred<E: Executor>(
        &self,
        executor: &E,
        event: AccountEventDetail<MoneyTransferred>,
    ) -> anyhow::Result<()> {
        let account = liteventd::load::<Account, _>(executor, event.detail.aggregate_id).await?;
        liteventd::save(account, event.detail.aggregate_id)
            .data(&AccountDebited {
                from_id: event.data.from_id,
                to_id: event.data.to_id,
                transaction_id: event.data.transaction_id,
                value: event.data.value,
            })?
            .commit(executor)
            .await?;
        Ok(())
    }

    async fn account_debited<E: Executor>(
        &self,
        executor: &E,
        event: AccountEventDetail<AccountDebited>,
    ) -> anyhow::Result<()> {
        let from = liteventd::load::<Account, _>(executor, event.data.from_id).await?;
        if from.item.balance - event.data.value < 0.0 {
            liteventd::save(from, event.data.from_id)
                .data(&MoneyTransferCancelled {
                    from_id: event.data.from_id,
                    transaction_id: event.data.transaction_id,
                    to_id: event.data.to_id,
                    value: event.data.value,
                    reason: Reason::BalanceTooLow,
                })?
                .commit(executor)
                .await?;
            return Ok(());
        }

        let to = liteventd::load::<Account, _>(executor, event.data.to_id).await?;

        liteventd::save(to, event.data.to_id)
            .data(&AccountCredited {
                from_id: event.data.from_id,
                transaction_id: event.data.transaction_id,
                to_id: event.data.to_id,
                value: event.data.value,
            })?
            .commit(executor)
            .await?;

        Ok(())
    }

    async fn account_credited<E: Executor>(
        &self,
        executor: &E,
        event: AccountEventDetail<AccountCredited>,
    ) -> anyhow::Result<()> {
        let from = liteventd::load::<Account, _>(executor, event.data.from_id).await?;

        liteventd::save(from, event.data.from_id)
            .data(&MoneyTransferSucceeded {
                from_id: event.data.from_id,
                transaction_id: event.data.transaction_id,
                to_id: event.data.to_id,
                value: event.data.value,
            })?
            .commit(executor)
            .await?;

        let to = liteventd::load::<Account, _>(executor, event.data.to_id).await?;

        liteventd::save(to, event.data.to_id)
            .data(&MoneyTransferSucceeded {
                from_id: event.data.from_id,
                transaction_id: event.data.transaction_id,
                to_id: event.data.to_id,
                value: event.data.value,
            })?
            .commit(executor)
            .await?;

        Ok(())
    }
}

pub async fn subscribe<E: Executor + 'static>(executor: E) -> anyhow::Result<()> {
    let key = "account-command";
    let recv = liteventd::subscribe(key)
        .aggregator::<Account>()
        .start(&executor)
        .await?;

    tokio::spawn(async move {
        while let Ok(event) = recv.recv() {
            let res = if event.aggregate_type == Account::name() {
                Command.handle(&executor, &event).await
            } else {
                Ok(())
            };

            if let Err(err) = res {
                panic!("subscribe # account-command > Command.handle => '{err}'");
            }

            if let Err(err) = liteventd::acknowledge(&executor, key, &event).await {
                panic!("subscribe # account-command > acknowledge => '{err}'");
            }
        }
    });

    Ok(())
}
