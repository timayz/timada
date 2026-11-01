#![allow(dead_code)]

use timada_catalog::{CategoryState, Command, CreateCategory, CreateProduct, ProductState};
use timada_core::db::Db;

/// A fresh in-memory event store with the read-model schema applied.
pub async fn executor_with_pool() -> (evento::Turso, Db) {
    let store = evento::Turso::in_memory_concurrent().await.unwrap();
    let mut conn = store.connect().unwrap();
    timada_catalog::MIGRATIONS
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

pub fn create(name: &str) -> CreateProduct {
    CreateProduct {
        name: name.to_owned(),
        slug: None,
    }
}

pub async fn create_product(cmd: &Command<evento::Turso>, name: &str) -> ProductState {
    cmd.create_product(ALICE, create(name)).await.unwrap()
}

pub fn new_category(name: &str) -> CreateCategory {
    CreateCategory {
        name: name.to_owned(),
        ..Default::default()
    }
}

pub async fn create_category(cmd: &Command<evento::Turso>, name: &str) -> CategoryState {
    cmd.create_category(ALICE, new_category(name))
        .await
        .unwrap()
}

/// A category under `parent_id`.
pub async fn create_child(
    cmd: &Command<evento::Turso>,
    name: &str,
    parent_id: &str,
) -> CategoryState {
    cmd.create_category(
        ALICE,
        CreateCategory {
            name: name.to_owned(),
            slug: None,
            parent_id: Some(parent_id.to_owned()),
        },
    )
    .await
    .unwrap()
}
