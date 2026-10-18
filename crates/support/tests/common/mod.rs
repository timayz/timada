#![allow(dead_code)]

use evento::migrator::Migrate;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx_migrator::Info;
use timada_support::{Command, MessageState, ReceiveMessage};

pub type Support = Command<evento::Sqlite>;

/// A fresh in-memory event store with the read-model schema applied.
pub async fn executor_with_pool() -> (evento::Sqlite, sqlx::SqlitePool) {
    let options = SqliteConnectOptions::new()
        .filename(":memory:")
        .create_if_missing(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let mut migrator = evento::sql_migrator::new::<sqlx::Sqlite>().unwrap();
    migrator
        .add_migrations(timada_support::migrations())
        .unwrap();
    migrator
        .run(&mut *conn, &evento::migrator::Plan::apply_all())
        .await
        .unwrap();
    drop(conn);
    (pool.clone().into(), pool)
}

pub async fn command() -> Support {
    Command(executor_with_pool().await.0)
}

pub const ALICE: &str = "user-alice";
pub const VISITOR: &str = "storefront";

pub fn draft(body: &str) -> ReceiveMessage {
    ReceiveMessage {
        first_name: "  Ada ".to_owned(),
        last_name: " Lovelace ".to_owned(),
        email: " Ada@Example.com ".to_owned(),
        body: body.to_owned(),
        language: Some("fr-FR,fr;q=0.9,en;q=0.8".to_owned()),
    }
}

pub async fn receive(support: &Support, body: &str) -> MessageState {
    support.receive_message(VISITOR, draft(body)).await.unwrap()
}
