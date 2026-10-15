#![allow(dead_code)]

use evento::migrator::Migrate;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx_migrator::Info;
use timada_discounts::{Command, CreateDiscount, DiscountState, DiscountValue};

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
        .add_migrations(timada_discounts::migrations())
        .unwrap();
    migrator
        .run(&mut *conn, &evento::migrator::Plan::apply_all())
        .await
        .unwrap();
    drop(conn);
    (pool.clone().into(), pool)
}

pub async fn command() -> Command<evento::Sqlite> {
    Command(executor_with_pool().await.0)
}

pub const ALICE: &str = "user-alice";
pub const SHOP: &str = "timada-dashboard";

pub fn percent(code: &str, bps: u32) -> CreateDiscount {
    CreateDiscount {
        code: code.to_owned(),
        value: DiscountValue::Percent(bps),
        ..Default::default()
    }
}

pub async fn create(cmd: &Command<evento::Sqlite>, discount: CreateDiscount) -> DiscountState {
    cmd.create_discount(ALICE, discount).await.unwrap()
}
