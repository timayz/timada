mod command;
mod event;
mod query;

pub use command::*;
pub use event::*;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use ulid::Ulid;

use liteventd::{Aggregator, Event, EventData};
use liteventd_macros::aggregate;

type AccountEventData<D> = EventData<D, HashMap<String, String>>;

#[derive(Debug, Serialize, Deserialize)]
pub enum MoneyTransactionState {
    New,
    Pending,
    Succeeded,
    Cancelled,
}

impl Default for MoneyTransactionState {
    fn default() -> Self {
        Self::New
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub enum MoneyTransactionType {
    Incoming,
    Outgoing,
}

impl Default for MoneyTransactionType {
    fn default() -> Self {
        Self::Incoming
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct MoneyTransaction {
    pub transaction_id: Ulid,
    pub from_id: Ulid,
    pub to_id: Ulid,
    pub value: f32,
    pub state: MoneyTransactionState,
    pub transaction_type: MoneyTransactionType,
    pub created_at: u32,
    pub updated_at: Option<u32>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[aggregate(
    aggregate_account_created,
    aggregate_account_credited,
    aggregate_account_debited,
    aggregate_full_name_changed,
    aggregate_money_transferred,
    aggregate_money_transfer_cancelled,
    aggregate_money_transfer_succeeded
)]
pub struct Account {
    pub id: Ulid,
    pub fullname: String,
    pub balance: f32,
    pub transaction_to_reserved_balance: HashMap<Ulid, f32>,
    pub transactions: HashMap<Ulid, MoneyTransaction>,
    pub created_at: u32,
    pub updated_at: Option<u32>,
}

impl Account {
    async fn aggregate_account_created(
        &mut self,
        event: AccountEventData<AccountCreated>,
    ) -> anyhow::Result<()> {
        self.id = event.details.aggregate_id;
        self.fullname = event.data.fullname;
        self.created_at = event.details.timestamp;

        Ok(())
    }

    async fn aggregate_account_credited(
        &mut self,
        event: AccountEventData<AccountCredited>,
    ) -> anyhow::Result<()> {
        self.updated_at = Some(event.details.timestamp);
        self.transaction_to_reserved_balance
            .insert(event.data.transaction_id, event.data.value);

        if let Some(transaction) = self.transactions.get_mut(&event.data.transaction_id) {
            transaction.state = MoneyTransactionState::Pending;
            transaction.updated_at = Some(event.details.timestamp);
        }
        Ok(())
    }

    async fn aggregate_account_debited(
        &mut self,
        event: AccountEventData<AccountDebited>,
    ) -> anyhow::Result<()> {
        self.updated_at = Some(event.details.timestamp);
        self.balance -= event.data.value;
        self.transaction_to_reserved_balance
            .insert(event.data.transaction_id, event.data.value * -1.0);

        if let Some(transaction) = self.transactions.get_mut(&event.data.transaction_id) {
            transaction.state = MoneyTransactionState::Pending;
            transaction.updated_at = Some(event.details.timestamp);
        }
        Ok(())
    }

    async fn aggregate_full_name_changed(
        &mut self,
        event: AccountEventData<FullNameChanged>,
    ) -> anyhow::Result<()> {
        self.fullname = event.data.fullname;
        self.updated_at = Some(event.details.timestamp);
        Ok(())
    }

    async fn aggregate_money_transferred(
        &mut self,
        event: AccountEventData<MoneyTransferred>,
    ) -> anyhow::Result<()> {
        self.updated_at = Some(event.details.timestamp);

        let transaction_type = if self.id == event.data.from_id {
            MoneyTransactionType::Outgoing
        } else {
            MoneyTransactionType::Incoming
        };

        let value = if self.id == event.data.from_id {
            event.data.value * -1.0
        } else {
            event.data.value
        };

        self.transactions.insert(
            event.data.transaction_id,
            MoneyTransaction {
                transaction_id: event.data.transaction_id,
                from_id: event.data.from_id,
                to_id: event.data.to_id,
                value,
                state: MoneyTransactionState::New,
                transaction_type,
                created_at: event.details.timestamp,
                updated_at: None,
            },
        );
        Ok(())
    }

    async fn aggregate_money_transfer_cancelled(
        &mut self,
        event: AccountEventData<MoneyTransferCancelled>,
    ) -> anyhow::Result<()> {
        self.updated_at = Some(event.details.timestamp);

        if event.data.to_id == self.id {
            self.transaction_to_reserved_balance
                .remove(&event.data.transaction_id);
        } else if let Some(reserved_balance) = self
            .transaction_to_reserved_balance
            .get(&event.data.transaction_id)
        {
            self.balance += reserved_balance * -1.0;
            self.transaction_to_reserved_balance
                .remove(&event.data.transaction_id);
        }

        if let Some(transaction) = self.transactions.get_mut(&event.data.transaction_id) {
            transaction.state = MoneyTransactionState::Cancelled;
            transaction.updated_at = Some(event.details.timestamp);
        }
        Ok(())
    }

    async fn aggregate_money_transfer_succeeded(
        &mut self,
        event: AccountEventData<MoneyTransferSucceeded>,
    ) -> anyhow::Result<()> {
        self.updated_at = Some(event.details.timestamp);

        if let Some(transaction) = self.transactions.get_mut(&event.data.transaction_id) {
            transaction.state = MoneyTransactionState::Succeeded;
            transaction.updated_at = Some(event.details.timestamp);
        }

        if let (Some(reserved_balance), true) = (
            self.transaction_to_reserved_balance
                .remove(&event.data.transaction_id),
            event.data.to_id == self.id,
        ) {
            self.balance += reserved_balance;
        }
        Ok(())
    }
}
