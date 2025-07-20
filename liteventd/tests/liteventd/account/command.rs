use liteventd::{Context, Executor, SubscribeHandler};
use std::collections::HashMap;
use ulid::Ulid;
use validator::Validate;

use crate::liteventd_test::account::{
    Account, AccountCreated, AccountCredited, AccountDebited, FullNameChanged,
    MoneyTransferCancelled, MoneyTransferSucceeded, MoneyTransferred, Reason,
};

type AccountContext<'a, E, D> = Context<'a, E, D, HashMap<String, String>>;

#[derive(Debug, Validate)]
struct CreateAccountInput {
    #[validate(length(min = 3, max = 20))]
    pub fullname: String,
}

pub async fn create_account<E: Executor>(
    executor: &E,
    fullname: impl Into<String>,
) -> anyhow::Result<Ulid> {
    let fullname = fullname.into();
    let input = CreateAccountInput { fullname };
    input.validate()?;

    let id = Ulid::default();

    liteventd::create(Account::default(), id)
        .data(&AccountCreated {
            fullname: input.fullname,
        })?
        .commit(executor)
        .await?;

    Ok(id)
}

#[derive(Debug, Validate)]
struct ChangeFullNameInput {
    pub id: Ulid,

    #[validate(length(min = 3, max = 20))]
    pub fullname: String,
}

pub async fn change_fullname<E: Executor>(
    executor: &E,
    id: Ulid,
    fullname: impl Into<String>,
) -> anyhow::Result<()> {
    let fullname = fullname.into();
    let input = ChangeFullNameInput { id, fullname };

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
struct TransferMoneyInput {
    pub from_id: Ulid,
    pub to_id: Ulid,
    #[validate(range(exclusive_min = 0.0))]
    pub value: f32,
}

pub async fn transfer_money<E: Executor>(
    executor: &E,
    from_id: Ulid,
    to_id: Ulid,
    value: f32,
) -> anyhow::Result<()> {
    let input = TransferMoneyInput {
        from_id,
        to_id,
        value,
    };
    input.validate()?;

    if input.from_id == input.to_id {
        return Err(anyhow::anyhow!(
            "not possible to transfer money to the same account"
        ));
    }

    let account_from = liteventd::load::<Account, _>(executor, input.from_id).await?;
    let account_to = liteventd::load::<Account, _>(executor, input.to_id).await?;

    liteventd::save(account_from, input.from_id)
        .data(&MoneyTransferred {
            from_id: input.from_id,
            to_id: account_to.item.id,
            transaction_id: Ulid::default(),
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
