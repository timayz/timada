#![allow(dead_code)]

use timada_core::db::Db;
use timada_tenancy::{Command, CreateTenant, TenantState};

/// A fresh in-memory event store with the read-model schema applied, and
/// the read-model database in it.
pub async fn executor_with_pool() -> (evento::Turso, Db) {
    let store = evento::Turso::in_memory_concurrent().await.unwrap();
    let mut conn = store.connect().unwrap();
    timada_tenancy::MIGRATIONS
        .to_latest(&mut conn)
        .await
        .map_err(|(_, err)| err)
        .unwrap();
    (store.clone(), Db::from(store))
}

pub async fn command() -> Command<evento::Turso> {
    Command(executor_with_pool().await.0)
}

pub fn create(name: &str, owner: &str) -> CreateTenant {
    CreateTenant {
        name: name.to_owned(),
        owner_user_id: owner.to_owned(),
    }
}

pub async fn create_tenant(cmd: &Command<evento::Turso>, name: &str, owner: &str) -> TenantState {
    cmd.create_tenant(create(name, owner)).await.unwrap()
}

pub fn new_id() -> String {
    ulid::Ulid::generate().to_string()
}

/// The gateway and the read-model database behind it.
pub async fn command_with_pool() -> (Command<evento::Turso>, Db) {
    let (executor, pool) = executor_with_pool().await;
    (Command(executor), pool)
}
