use anyhow::Result;
use liteventd::Executor;
use ulid::Ulid;
use validator::Validate;

use crate::account::{Account, AccountCreated, FullNameChanged, MoneyTransferred};

#[derive(Debug, Validate)]
pub struct CreateAccountInput {
    #[validate(length(min = 3, max = 20))]
    pub fullname: String,
}

pub fn create_account<E: Executor>(executor: &E, input: CreateAccountInput) -> Result<Ulid> {
    input.validate()?;

    let id = Ulid::new();

    liteventd::save(Account::default(), id)
        .data(&AccountCreated {
            fullname: input.fullname,
        })?
        .commit(executor)?;

    Ok(id)
}

#[derive(Debug, Validate)]
pub struct ChangeFullNameInput {
    pub id: Ulid,

    #[validate(length(min = 3, max = 20))]
    pub fullname: String,
}

pub fn change_fullname<E: Executor>(executor: &E, input: ChangeFullNameInput) -> Result<()> {
    input.validate()?;

    let account = liteventd::load(executor, input.id)?;

    liteventd::save::<Account>(account.item, input.id)
        .original_version(account.version)
        .data(&FullNameChanged {
            fullname: input.fullname,
        })?
        .commit(executor)?;

    Ok(())
}

#[derive(Debug, Validate)]
pub struct TransferMoneyInput {
    pub from: Ulid,
    pub to: Ulid,
    #[validate(range(exclusive_min = 0.0))]
    pub value: f32,
}

pub fn transfer_money<E: Executor>(executor: &E, input: TransferMoneyInput) -> Result<()> {
    input.validate()?;

    if input.from == input.to {
        return Err(anyhow::anyhow!(
            "not possible to transfer money to the same account"
        ));
    }

    let from_account = liteventd::load::<_, Account>(executor, input.from)?;

    if from_account.item.balance - input.value < 0.0 {
        return Err(anyhow::anyhow!("balance too low"));
    }

    let to_account = liteventd::load::<_, Account>(executor, input.to)?;

    liteventd::save(from_account.item, input.from)
        .original_version(from_account.version)
        .data(&MoneyTransferred {
            from_id: input.from,
            to_id: to_account.item.id,
            transaction_id: Ulid::new(),
            value: input.value,
        })?
        .commit(executor)?;

    Ok(())
}
