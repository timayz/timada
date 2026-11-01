#![allow(dead_code)]

use timada_core::db::Db;
use timada_pages::{Command, CreatePage};

pub type Pages = Command<evento::Turso>;

/// A fresh in-memory event store with the read-model schema applied.
pub async fn executor_with_pool() -> (evento::Turso, Db) {
    let store = evento::Turso::in_memory_concurrent().await.unwrap();
    let mut conn = store.connect().unwrap();
    timada_pages::MIGRATIONS
        .to_latest(&mut conn)
        .await
        .map_err(|(_, err)| err)
        .unwrap();
    (store.clone(), Db::from(store))
}

pub async fn command() -> Pages {
    Command::new(executor_with_pool().await.0, TENANT)
}

/// The tenant the tests act in.
pub const TENANT: &str = "acme";

pub const ALICE: &str = "user-alice";

pub fn draft(title: &str) -> CreatePage {
    CreatePage {
        slug: String::new(),
        title: title.to_owned(),
        body: "## Article 1\r\n\r\nLes ventes sont fermes.\r\n\r\n".to_owned(),
    }
}
