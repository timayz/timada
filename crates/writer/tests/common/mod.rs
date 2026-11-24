#![allow(dead_code)]

use timada_core::db::Db;
use timada_writer::{Command, KIND_PRODUCT, RequestDraft};

pub type Writer = Command<evento::Turso>;

/// A fresh in-memory event store with the read-model schema applied.
pub async fn executor_with_pool() -> (evento::Turso, Db) {
    let store = evento::Turso::in_memory_concurrent().await.unwrap();
    let mut conn = store.connect().unwrap();
    timada_writer::MIGRATIONS
        .to_latest(&mut conn)
        .await
        .map_err(|(_, err)| err)
        .unwrap();
    (store.clone(), Db::from(store))
}

pub async fn command() -> Writer {
    Command::new(executor_with_pool().await.0, TENANT)
}

/// The tenant the tests act in.
pub const TENANT: &str = "acme";
pub const ALICE: &str = "user-alice";
pub const NOW: u64 = 1_700_000_000;

/// A draft of the product `p1` for a keyword.
pub fn gloves() -> RequestDraft {
    RequestDraft {
        kind: KIND_PRODUCT.to_owned(),
        subject_id: "p1".to_owned(),
        subject_name: "Gants".to_owned(),
        keyword: " gants moto été ".to_owned(),
    }
}
