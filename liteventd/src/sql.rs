use crate::{Aggregator, Event, Executor, WriteError};
use serde::{Deserialize, Serialize};
use sqlx::{Database, Pool};
use ulid::Ulid;

#[derive(Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub aggregate_id: Ulid,
    pub aggregate_type: String,
    pub version: u16,
    pub data: Vec<u8>,
}

pub struct SqlExecutor<D: Database>(pub Pool<D>);

impl<D> SqlExecutor<D>
where
    D: Database,
    for<'a> D::Arguments<'a>: sqlx::IntoArguments<'a, D>,
    for<'c> &'c mut D::Connection: sqlx::Executor<'c, Database = D>,
{
    fn get_bind_key() -> &'static str {
        match D::NAME {
            "SQLite" => "?",
            name => panic!("get_database_schema not supported for '{name}'"),
        }
    }

    pub async fn create_database_schema(&self) -> sqlx::Result<()> {
        let schema = match D::NAME {
            "SQLite" => Self::get_sqlite_schema(),
            name => panic!("get_database_schema not supported for '{name}'"),
        };

        sqlx::query(schema).execute(&self.0).await?;

        Ok(())
    }

    fn get_sqlite_schema() -> &'static str {
        r#"
CREATE TABLE IF NOT EXISTS event (
    id  TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    aggregate_type TEXT NOT NULL,
    aggregate_id TEXT NOT NULL,
    version INTEGER NOT NULL,
    data BLOB NOT NULL,
    metadata BLOB NULL,
    routing_key TEXT NOT NULL,
    timestamp INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
);

CREATE INDEX IF NOT EXISTS idx_event_5sQ1BZoDjCO ON event(aggregate_type, aggregate_id);
CREATE INDEX IF NOT EXISTS idx_event_CISJrZpQwSz ON event(aggregate_type);
CREATE INDEX IF NOT EXISTS idx_event_sX8G8WjC8WQ ON event(routing_key, aggregate_type);
CREATE UNIQUE INDEX IF NOT EXISTS idx_event_BVjOvQoJPQj ON event(aggregate_type,aggregate_id,version);

CREATE TABLE IF NOT EXISTS snapshot (
    aggregate_type TEXT NOT NULL,
    aggregate_id TEXT NOT NULL,
    version INTEGER NOT NULL,
    data BLOB NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
    updated_at INTEGER NULL,
    PRIMARY KEY (aggregate_type,aggregate_id)
);
        "#
    }
}

impl<D: Database> Clone for SqlExecutor<D> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<D: Database> From<Pool<D>> for SqlExecutor<D> {
    fn from(value: Pool<D>) -> Self {
        Self(value)
    }
}

#[async_trait::async_trait]
impl<D> Executor for SqlExecutor<D>
where
    D: Database,
    for<'a> D::Arguments<'a>: sqlx::IntoArguments<'a, D>,
    for<'c> &'c mut D::Connection: sqlx::Executor<'c, Database = D>,
    for<'t> String: sqlx::Encode<'t, D> + sqlx::Decode<'t, D> + sqlx::Type<D>,
    for<'t> u16: sqlx::Encode<'t, D> + sqlx::Decode<'t, D> + sqlx::Type<D>,
    for<'t> Vec<u8>: sqlx::Encode<'t, D> + sqlx::Decode<'t, D> + sqlx::Type<D>,
{
    async fn write<A: Aggregator>(
        &self,
        aggregator: &A,
        events: Vec<Event>,
    ) -> Result<(), WriteError> {
        let Some(info) = events.first() else {
            return Err(WriteError::NoData);
        };

        let values = events
            .iter()
            .map(|_| format!("({0},{0},{0},{0},{0},{0},{0},{0})", Self::get_bind_key()))
            .collect::<Vec<_>>()
            .join(",");

        let sql = format!(
            "INSERT INTO event (id,name,data,metadata,aggregate_type,aggregate_id,version,routing_key) VALUES {}",
            values
        );

        let mut query = sqlx::query(&sql);

        for event in events.iter() {
            query = query
                .bind(event.id.to_string())
                .bind(&event.name)
                .bind(&event.data)
                .bind(&event.metadata)
                .bind(&event.aggregate_type)
                .bind(event.aggregate_id.to_string())
                .bind(event.version)
                .bind(&event.routing_key);
        }

        query.execute(&self.0).await.map_err(|err| {
            if err.to_string().contains("(code: 2067)") {
                WriteError::InvalidOriginalVersion
            } else {
                err.into()
            }
        })?;

        let mut data = vec![];
        ciborium::into_writer(aggregator, &mut data)?;

        sqlx::query(&format!(
            r#"
        INSERT INTO snapshot (aggregate_type,aggregate_id,version,data)
        VALUES ({0},{0},{0},{0})
            ON CONFLICT(aggregate_type,aggregate_id) DO UPDATE SET 
                data=excluded.data,
                version=excluded.version,
                updated_at=(strftime('%s', 'now'))
        "#,
            Self::get_bind_key()
        ))
        .bind(&info.aggregate_type)
        .bind(info.aggregate_id.to_string())
        .bind(info.version)
        .bind(&data)
        .execute(&self.0)
        .await?;

        Ok(())
    }
}
