use liteventd::{Context, Executor, SubscribeHandler};
use std::collections::HashMap;
use ulid::Ulid;
use validator::Validate;

use crate::account::{
    Account, AccountCreated, AccountCredited, AccountDebited, FullNameChanged,
    MoneyTransferCancelled, MoneyTransferSucceeded, MoneyTransferred, Reason,
};

type AccountContext<'a, E, D> = Context<'a, E, D, HashMap<String, String>>;

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

struct Command;

#[liteventd_macros::handler]
impl Command {
    async fn money_transferred<E: Executor>(
        &self,
        context: AccountContext<'_, E, MoneyTransferred>,
    ) -> anyhow::Result<()> {
        let account =
            liteventd::load::<Account, _>(context.executor, context.event.aggregate_id).await?;
        liteventd::save(account, context.event.aggregate_id)
            .data(&AccountDebited {
                from_id: context.data.from_id,
                to_id: context.data.to_id,
                transaction_id: context.data.transaction_id,
                value: context.data.value,
            })?
            .commit(context.executor)
            .await?;
        Ok(())
    }

    async fn account_debited<E: Executor>(
        &self,
        context: AccountContext<'_, E, AccountDebited>,
    ) -> anyhow::Result<()> {
        let from = liteventd::load::<Account, _>(context.executor, context.data.from_id).await?;
        if from.item.balance - context.data.value < 0.0 {
            liteventd::save(from, context.data.from_id)
                .data(&MoneyTransferCancelled {
                    from_id: context.data.from_id,
                    transaction_id: context.data.transaction_id,
                    to_id: context.data.to_id,
                    value: context.data.value,
                    reason: Reason::BalanceTooLow,
                })?
                .commit(context.executor)
                .await?;
            return Ok(());
        }

        let to = liteventd::load::<Account, _>(context.executor, context.data.to_id).await?;

        liteventd::save(to, context.data.to_id)
            .data(&AccountCredited {
                from_id: context.data.from_id,
                transaction_id: context.data.transaction_id,
                to_id: context.data.to_id,
                value: context.data.value,
            })?
            .commit(context.executor)
            .await?;

        Ok(())
    }

    async fn account_credited<E: Executor>(
        &self,
        context: AccountContext<'_, E, AccountCredited>,
    ) -> anyhow::Result<()> {
        let from = liteventd::load::<Account, _>(context.executor, context.data.from_id).await?;

        liteventd::save(from, context.data.from_id)
            .data(&MoneyTransferSucceeded {
                from_id: context.data.from_id,
                transaction_id: context.data.transaction_id,
                to_id: context.data.to_id,
                value: context.data.value,
            })?
            .commit(context.executor)
            .await?;

        let to = liteventd::load::<Account, _>(context.executor, context.data.to_id).await?;

        liteventd::save(to, context.data.to_id)
            .data(&MoneyTransferSucceeded {
                from_id: context.data.from_id,
                transaction_id: context.data.transaction_id,
                to_id: context.data.to_id,
                value: context.data.value,
            })?
            .commit(context.executor)
            .await?;

        Ok(())
    }
}

pub async fn subscribe<E: Executor + 'static>(executor: E) -> anyhow::Result<()> {
    let key = "account-command";
    let _ = liteventd::subscribe::<E>(key)
        .handler::<Account, _>(Command)
        .start(&executor)
        .await?;

    Ok(())
}
