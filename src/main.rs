use liteventd::{sql::Sql, AggregatorName, Context, EventData, Executor};
use serde::{Deserialize, Serialize};
use sqlx::{any::install_default_drivers, migrate::MigrateDatabase, Any, Sqlite, SqlitePool};
use std::{str::FromStr, time::Duration};
use tracing_subscriber::{
    prelude::__tracing_subscriber_SubscriberExt, util::SubscriberInitExt, EnvFilter,
};

type CalculEvent<D> = EventData<D, bool>;

#[derive(AggregatorName, Serialize, Deserialize)]
struct Added {
    pub value: i32,
}

#[derive(AggregatorName, Serialize, Deserialize)]
struct Subtracted {
    pub value: i32,
}

#[derive(Default, Serialize, Deserialize, Clone)]
struct CalculOne {
    pub value: i32,
}

#[liteventd::aggregator]
impl CalculOne {
    async fn added(&mut self, event: CalculEvent<Added>) -> anyhow::Result<()> {
        self.value += event.data.value;

        Ok(())
    }

    async fn subtracted(&mut self, event: CalculEvent<Subtracted>) -> anyhow::Result<()> {
        self.value -= event.data.value;

        Ok(())
    }
}

#[derive(AggregatorName, Serialize, Deserialize)]
struct Multiplied {
    pub value: i32,
}

#[derive(AggregatorName, Serialize, Deserialize)]
struct Divided {
    pub value: i32,
}

#[derive(Default, Serialize, Deserialize, Clone)]
struct CalculTwo {
    pub value: i32,
}

#[liteventd::aggregator]
impl CalculTwo {
    async fn multiplied(&mut self, event: CalculEvent<Multiplied>) -> anyhow::Result<()> {
        self.value *= event.data.value;

        Ok(())
    }

    async fn divided(&mut self, event: CalculEvent<Divided>) -> anyhow::Result<()> {
        self.value /= event.data.value;

        Ok(())
    }
}

#[liteventd::handler(CalculOne)]
async fn handler_calcul_one_added<E: Executor>(
    _context: &Context<'_, E>,
    data: Added,
    _metadata: bool,
) -> anyhow::Result<()> {
    println!("add >> {}", data.value);

    Ok(())
}

#[liteventd::handler(CalculOne)]
async fn handler_calcul_one_subtracted<E: Executor>(
    context: &Context<'_, E>,
    data: Subtracted,
    _metadata: bool,
) -> anyhow::Result<()> {
    let v: bool = context.extract();
    println!("sub >> {} {}", data.value, v);

    Ok(())
}

#[liteventd::handler(CalculTwo)]
async fn handler_calcul_two_multiplied<E: Executor>(
    context: &Context<'_, E>,
    data: Multiplied,
    _metadata: bool,
) -> anyhow::Result<()> {
    let v: bool = context.extract();
    println!("multi >> {} {}", data.value, v);

    Ok(())
}

#[tokio::main()]
async fn main() -> anyhow::Result<()> {
    let env_filter = EnvFilter::from_str("error,liteventd=debug")?;
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(env_filter)
        .init();

    install_default_drivers();

    let url = "sqlite:./target/tmp/timada.db";

    Any::create_database(url).await?;

    let pool = SqlitePool::connect(url).await?;
    let schema = Sql::<Sqlite>::get_schema();

    sqlx::query(&schema).execute(&pool).await?;

    let executor: Sql<Sqlite> = pool.into();

    liteventd::subscribe::<Sql<Sqlite>>("eu-west-3")
        .delay(Duration::from_secs(5))
        .data(true)
        .aggregator::<CalculOne>()
        .aggregator::<CalculTwo>()
        .handler(handler_calcul_one_added())
        .handler(handler_calcul_one_subtracted())
        .handler(handler_calcul_two_multiplied())
        .run(&executor)
        .await?;

    liteventd::create::<CalculOne>()
        .data(&Added { value: 0 })?
        .metadata(&true)?
        .commit(&executor)
        .await?;

    liteventd::create::<CalculOne>()
        .data(&Subtracted { value: 10 })?
        .metadata(&true)?
        .commit(&executor)
        .await?;

    liteventd::create::<CalculTwo>()
        .data(&Multiplied { value: 3 })?
        .metadata(&true)?
        .commit(&executor)
        .await?;

    tokio::time::sleep(tokio::time::Duration::from_secs(8)).await;

    Ok(())
}
