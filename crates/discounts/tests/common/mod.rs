#![allow(dead_code)]

use timada_core::db::Db;
use timada_discounts::{Command, CreateDiscount, DiscountState, DiscountValue};

/// A fresh in-memory event store with the read-model schema applied.
pub async fn executor_with_pool() -> (evento::Turso, Db) {
    let store = evento::Turso::in_memory_concurrent().await.unwrap();
    let mut conn = store.connect().unwrap();
    timada_discounts::MIGRATIONS
        .to_latest(&mut conn)
        .await
        .map_err(|(_, err)| err)
        .unwrap();
    (store.clone(), Db::from(store))
}

pub async fn command() -> Command<evento::Turso> {
    Command::new(executor_with_pool().await.0, TENANT)
}

/// The tenant the tests act in.
pub const TENANT: &str = "acme";
pub const ALICE: &str = "user-alice";
pub const SHOP: &str = "timada-dashboard";

pub fn percent(code: &str, bps: u32) -> CreateDiscount {
    CreateDiscount {
        code: code.to_owned(),
        value: DiscountValue::Percent(bps),
        ..Default::default()
    }
}

pub async fn create(cmd: &Command<evento::Turso>, discount: CreateDiscount) -> DiscountState {
    cmd.create_discount(ALICE, discount).await.unwrap()
}
