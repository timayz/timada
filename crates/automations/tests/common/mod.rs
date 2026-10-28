#![allow(dead_code)]

use evento::migrator::Migrate;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx_migrator::Info;
use timada_automations::{
    Command, Definition, Lane, Slot,
    catalog::{CONDITION, NOTIFY, ORDER_PLACED, WAIT},
};

pub type Automations = Command<evento::Sqlite>;

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
        .add_migrations(timada_automations::migrations())
        .unwrap();
    migrator
        .run(&mut *conn, &evento::migrator::Plan::apply_all())
        .await
        .unwrap();
    drop(conn);
    (pool.clone().into(), pool)
}

pub async fn command() -> Automations {
    Command::new(executor_with_pool().await.0, TENANT)
}

/// The tenant the tests act in.
pub const TENANT: &str = "acme";

pub const ALICE: &str = "user-alice";
pub const BOB: &str = "user-bob";

pub fn root(index: usize) -> Slot {
    Slot::new(Lane::Root, index)
}

pub fn branch(step: &str, branch: usize, index: usize) -> Slot {
    Slot::new(
        Lane::Branch {
            step: step.to_owned(),
            branch,
        },
        index,
    )
}

/// "When an order is placed, if its total is 50 or more, notify the team",
/// ready to publish.
pub fn big_orders() -> Definition {
    let mut definition = Definition::default();
    definition.set_trigger(ORDER_PLACED).unwrap();
    let condition = definition.insert(&root(0), CONDITION).unwrap();
    let mut settings = definition.find(&condition).unwrap().settings.clone();
    settings.insert("rules.0.op".into(), "ge".into());
    settings.insert("rules.0.value".into(), "50".into());
    definition.configure(&condition, settings).unwrap();
    let notify = definition
        .insert(&branch(&condition, 0, 0), NOTIFY)
        .unwrap();
    definition
        .configure(
            &notify,
            [("subject".to_owned(), "A big order".to_owned())]
                .into_iter()
                .collect(),
        )
        .unwrap();
    definition.insert(&branch(&condition, 1, 0), WAIT).unwrap();
    definition
}
