#![allow(dead_code)]

use timada_core::db::Db;
use timada_proposals::aggregator::Change;
use timada_proposals::{Command, FIELD_DESCRIPTION, FIELD_NAME, KIND_PRODUCT, Prepare};

pub type Proposals = Command<evento::Turso>;

/// A fresh in-memory event store with the read-model schema applied.
pub async fn executor_with_pool() -> (evento::Turso, Db) {
    let store = evento::Turso::in_memory_concurrent().await.unwrap();
    let mut conn = store.connect().unwrap();
    timada_proposals::MIGRATIONS
        .to_latest(&mut conn)
        .await
        .map_err(|(_, err)| err)
        .unwrap();
    (store.clone(), Db::from(store))
}

pub async fn command() -> Proposals {
    Command::new(executor_with_pool().await.0, TENANT)
}

/// The tenant the tests act in.
pub const TENANT: &str = "acme";

pub const ALICE: &str = "user-alice";
pub const BOB: &str = "user-bob";
pub const NOW: u64 = 1_700_000_000;

pub fn change(field: &str, before: &str, after: &str) -> Change {
    Change {
        field: field.to_owned(),
        before: before.to_owned(),
        after: after.to_owned(),
    }
}

/// A new name and a new description for the product `p1`.
pub fn texts() -> Prepare {
    Prepare {
        kind: KIND_PRODUCT.to_owned(),
        subject_id: "p1".to_owned(),
        subject_name: "Gants".to_owned(),
        changes: vec![
            change(FIELD_NAME, "Gants", "Gants moto été"),
            change(FIELD_DESCRIPTION, "", "Des gants légers."),
        ],
        reason: "Wording for the keyword \"gants moto été\".".to_owned(),
        source: "writer".to_owned(),
        valid_until: None,
    }
}

pub async fn prepared(proposals: &Proposals) -> String {
    proposals.prepare(BOB, texts(), NOW).await.unwrap()
}
