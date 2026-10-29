#![allow(dead_code)]

use evento::migrator::Migrate;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx_migrator::Info;
use timada_pages::{Command, CreatePage};

pub type Pages = Command<evento::Sqlite>;

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
    migrator.add_migrations(timada_pages::migrations()).unwrap();
    migrator
        .run(&mut *conn, &evento::migrator::Plan::apply_all())
        .await
        .unwrap();
    drop(conn);
    (pool.clone().into(), pool)
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
