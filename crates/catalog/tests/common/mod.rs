#![allow(dead_code)]

use evento::migrator::Migrate;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx_migrator::Info;
use timada_catalog::{CategoryState, Command, CreateCategory, CreateProduct, ProductState};

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
        .add_migrations(timada_catalog::migrations())
        .unwrap();
    migrator
        .run(&mut *conn, &evento::migrator::Plan::apply_all())
        .await
        .unwrap();
    drop(conn);
    (pool.clone().into(), pool)
}

pub async fn command() -> Command<evento::Sqlite> {
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

pub async fn create_product(cmd: &Command<evento::Sqlite>, name: &str) -> ProductState {
    cmd.create_product(ALICE, create(name)).await.unwrap()
}

pub fn new_category(name: &str) -> CreateCategory {
    CreateCategory {
        name: name.to_owned(),
        ..Default::default()
    }
}

pub async fn create_category(cmd: &Command<evento::Sqlite>, name: &str) -> CategoryState {
    cmd.create_category(ALICE, new_category(name))
        .await
        .unwrap()
}

/// A category under `parent_id`.
pub async fn create_child(
    cmd: &Command<evento::Sqlite>,
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
