use liteventd::AggregatorEvent;
use serde::{Deserialize, Serialize};
use timada_macros::AggregatorEvent;
use ulid::Ulid;

#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub enum Reason {
    BalanceTooLow,
    InternalServerError,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, AggregatorEvent)]
pub struct AccountCreated {
    pub fullname: String,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, AggregatorEvent)]
pub struct AccountCredited {
    pub transaction_id: Ulid,
    pub from_id: Ulid,
    pub to_id: Ulid,
    pub value: f32,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, AggregatorEvent)]
pub struct AccountDebited {
    pub transaction_id: Ulid,
    pub from_id: Ulid,
    pub to_id: Ulid,
    pub value: f32,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, AggregatorEvent)]
pub struct FullNameChanged {
    pub fullname: String,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, AggregatorEvent)]
pub struct MoneyTransferCancelled {
    pub transaction_id: Ulid,
    pub from_id: Ulid,
    pub to_id: Ulid,
    pub value: f32,
    pub reason: Reason,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, AggregatorEvent)]
pub struct MoneyTransferSucceeded {
    pub transaction_id: Ulid,
    pub from_id: Ulid,
    pub to_id: Ulid,
    pub value: f32,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, AggregatorEvent)]
pub struct MoneyTransferred {
    pub transaction_id: Ulid,
    pub from_id: Ulid,
    pub to_id: Ulid,
    pub value: f32,
}
