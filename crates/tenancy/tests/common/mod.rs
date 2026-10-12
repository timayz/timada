#![allow(dead_code)]

use evento::migrator::Migrate;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx_migrator::Info;
use timada_tenancy::{Command, CreateTenant, TenantState};

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
        .add_migrations(timada_tenancy::migrations())
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

pub fn create(slug: &str, owner: &str) -> CreateTenant {
    CreateTenant {
        slug: slug.to_owned(),
        name: format!("{slug} shop"),
        owner_user_id: owner.to_owned(),
    }
}

pub async fn create_tenant(cmd: &Command<evento::Sqlite>, slug: &str, owner: &str) -> TenantState {
    cmd.create_tenant(create(slug, owner)).await.unwrap()
}

pub fn new_id() -> String {
    ulid::Ulid::generate().to_string()
}
