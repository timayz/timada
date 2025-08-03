use liteventd::{sql::Sql, AggregatorName, Context, EventData, Executor};
use serde::{Deserialize, Serialize};
use sqlx::{any::install_default_drivers, migrate::MigrateDatabase, Any, Sqlite, SqlitePool};

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

#[liteventd::handler(CalculOne, Added)]
async fn handler_calcul_one_added<E: Executor>(
    _context: &Context<'_, E>,
    event: CalculEvent<Added>,
) -> anyhow::Result<()> {
    println!("{}", event.data.value);

    Ok(())
}

#[tokio::main()]
async fn main() -> anyhow::Result<()> {
    install_default_drivers();

    let url = "sqlite:./target/tmp/timada.db";

    Any::create_database(url).await?;

    let pool = SqlitePool::connect(url).await?;
    let schema = Sql::<Sqlite>::get_schema();

    sqlx::query(&schema).execute(&pool).await?;

    let _executor: Sql<Sqlite> = pool.into();

    liteventd::subscribe::<Sql<Sqlite>>("eu-west-3")
        .aggregator::<CalculOne>()
        .handler(handler_calcul_one_added());

    Ok(())
}
