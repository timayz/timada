mod command;
mod event;
mod view;

pub use command::*;
pub use event::*;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use ulid::Ulid;

use liteventd::{Aggregator, Event};
use timada_macros::aggregate;

#[derive(Debug, Serialize, Deserialize)]
enum MoneyTransactionState {
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
enum MoneyTransactionType {
    Incoming,
    Outgoing,
}

impl Default for MoneyTransactionType {
    fn default() -> Self {
        Self::Incoming
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct MoneyTransaction {
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
struct Account {
    pub id: Ulid,
    pub fullname: String,
    pub balance: f32,
    pub transaction_to_reserved_balance: HashMap<Ulid, f32>,
    pub transactions: HashMap<Ulid, MoneyTransaction>,
    pub created_at: u32,
    pub updated_at: Option<u32>,
}

impl Account {
    fn aggregate_account_created(
        &mut self,
        event: &'_ Event,
        data: AccountCreated,
        _metadata: HashMap<String, String>,
    ) -> anyhow::Result<()> {
        self.id = event.aggregate_id;
        self.fullname = data.fullname;
        self.created_at = event.timestamp;

        Ok(())
    }

    fn aggregate_account_credited(
        &mut self,
        event: &'_ Event,
        data: AccountCredited,
        _metadata: HashMap<String, String>,
    ) -> anyhow::Result<()> {
        self.updated_at = Some(event.timestamp);
        self.transaction_to_reserved_balance
            .insert(data.transaction_id, data.value);

        if let Some(transaction) = self.transactions.get_mut(&data.transaction_id) {
            transaction.state = MoneyTransactionState::Pending;
            transaction.updated_at = Some(event.timestamp);
        }
        Ok(())
    }

    fn aggregate_account_debited(
        &mut self,
        event: &'_ Event,
        data: AccountDebited,
        _metadata: HashMap<String, String>,
    ) -> anyhow::Result<()> {
        self.updated_at = Some(event.timestamp);
        self.balance -= data.value;
        self.transaction_to_reserved_balance
            .insert(data.transaction_id, data.value * -1.0);

        if let Some(transaction) = self.transactions.get_mut(&data.transaction_id) {
            transaction.state = MoneyTransactionState::Pending;
            transaction.updated_at = Some(event.timestamp);
        }
        Ok(())
    }

    fn aggregate_full_name_changed(
        &mut self,
        event: &'_ Event,
        data: FullNameChanged,
        _metadata: HashMap<String, String>,
    ) -> anyhow::Result<()> {
        self.fullname = data.fullname;
        self.updated_at = Some(event.timestamp);
        Ok(())
    }

    fn aggregate_money_transferred(
        &mut self,
        event: &'_ Event,
        data: MoneyTransferred,
        _metadata: HashMap<String, String>,
    ) -> anyhow::Result<()> {
        self.updated_at = Some(event.timestamp);

        let transaction_type = if self.id == data.from_id {
            MoneyTransactionType::Outgoing
        } else {
            MoneyTransactionType::Incoming
        };

        let value = if self.id == data.from_id {
            data.value * -1.0
        } else {
            data.value
        };

        self.transactions.insert(
            data.transaction_id,
            MoneyTransaction {
                transaction_id: data.transaction_id,
                from_id: data.from_id,
                to_id: data.to_id,
                value,
                state: MoneyTransactionState::New,
                transaction_type,
                created_at: event.timestamp,
                updated_at: None,
            },
        );
        Ok(())
    }

    fn aggregate_money_transfer_cancelled(
        &mut self,
        event: &'_ Event,
        data: MoneyTransferCancelled,
        _metadata: HashMap<String, String>,
    ) -> anyhow::Result<()> {
        self.updated_at = Some(event.timestamp);

        if data.to_id == self.id {
            self.transaction_to_reserved_balance
                .remove(&data.transaction_id);
        } else if let Some(reserved_balance) = self
            .transaction_to_reserved_balance
            .get(&data.transaction_id)
        {
            self.balance += reserved_balance * -1.0;
            self.transaction_to_reserved_balance
                .remove(&data.transaction_id);
        }

        if let Some(transaction) = self.transactions.get_mut(&data.transaction_id) {
            transaction.state = MoneyTransactionState::Cancelled;
            transaction.updated_at = Some(event.timestamp);
        }
        Ok(())
    }

    fn aggregate_money_transfer_succeeded(
        &mut self,
        event: &'_ Event,
        data: MoneyTransferSucceeded,
        _metadata: HashMap<String, String>,
    ) -> anyhow::Result<()> {
        self.updated_at = Some(event.timestamp);

        if let Some(transaction) = self.transactions.get_mut(&data.transaction_id) {
            transaction.state = MoneyTransactionState::Succeeded;
            transaction.updated_at = Some(event.timestamp);
        }

        if let (Some(reserved_balance), true) = (
            self.transaction_to_reserved_balance
                .remove(&data.transaction_id),
            data.to_id == self.id,
        ) {
            self.balance += reserved_balance;
        }
        Ok(())
    }
}
