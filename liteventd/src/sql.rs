use crate::Executor;
use sqlx::{Database, Pool};

pub struct SqlExecutor<D: Database>(pub Pool<D>);

impl<D: Database> SqlExecutor<D> {
    pub fn get_database_schema() -> &'static str {
        match D::NAME {
            "SQLite" => Self::get_sqlite_schema(),
            name => panic!("get_database_schema not supported for '{name}'"),
        }
    }

    fn get_sqlite_schema() -> &'static str {
        r#"
CREATE TABLE event (
    id  TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    aggregate_id TEXT NOT NULL,
    aggregate_type TEXT NOT NULL,
    version INTEGER NOT NULL,
    data BLOB NOT NULL,
    metadata BLOB NULL,
    routing_key TEXT NOT NULL,
    timestamp INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
);

CREATE INDEX idx_event_5sQ1BZoDjCO ON event(aggregate_id);
CREATE INDEX idx_event_CISJrZpQwSz ON event(aggregate_type);
CREATE INDEX idx_event_sX8G8WjC8WQ ON event(routing_key, aggregate_type);
CREATE UNIQUE INDEX idx_event_BVjOvQoJPQj ON event(aggregate_id,aggregate_type,version);
        "#
    }
}

impl<D: Database> Clone for SqlExecutor<D> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

#[async_trait::async_trait]
impl<D: Database> Executor for SqlExecutor<D> {
    async fn test(&self) {
        todo!()
    }
}
