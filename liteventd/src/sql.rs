use std::sync::Arc;

use crate::{Aggregator, Event, Executor, SaveError, Snapshot};
use libsql::Builder;
use libsql_migration::content::migrate;
use serde::{Deserialize, Serialize};
use ulid::Ulid;

#[derive(Clone)]
pub struct LibSqlExecutor {
    db: Arc<libsql::Database>,
}

impl LibSqlExecutor {
    pub async fn new(path: impl Into<String>) -> Result<Self, libsql::Error> {
        let db = Builder::new_local(path.into()).build().await?;
        Ok(Self { db: Arc::new(db) })
    }

    pub async fn new_remote(
        url: impl Into<String>,
        token: impl Into<String>,
    ) -> Result<Self, libsql::Error> {
        let db = Builder::new_remote(url.into(), token.into())
            .build()
            .await?;
        Ok(Self { db: Arc::new(db) })
    }

    pub async fn migrate(
        &self,
    ) -> Result<(), libsql_migration::errors::LibsqlContentMigratorError> {
        let conn = self.db.connect()?;

        let migration_id = "20250704_init".to_string();
        let migration_script = r#"
CREATE TABLE event (
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

CREATE INDEX idx_event_5sQ1BZoDjCO ON event(aggregate_type, aggregate_id);
CREATE INDEX idx_event_CISJrZpQwSz ON event(aggregate_type);
CREATE INDEX idx_event_sX8G8WjC8WQ ON event(routing_key, aggregate_type);
CREATE UNIQUE INDEX idx_event_BVjOvQoJPQj ON event(aggregate_type,aggregate_id,version);

CREATE TABLE snapshot (
    aggregate_type TEXT NOT NULL,
    aggregate_id TEXT NOT NULL,
    version INTEGER NOT NULL,
    data BLOB NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
    updated_at INTEGER NULL,
    PRIMARY KEY (aggregate_type,aggregate_id)
);
        "#
        .to_string();

        migrate(&conn, migration_id, migration_script).await?;

        Ok(())
    }
}

#[async_trait::async_trait]
impl Executor for LibSqlExecutor {
    async fn get_event_routing_key(
        &self,
        aggregate_type: String,
        aggregate_id: Ulid,
    ) -> Result<Option<String>, SaveError> {
        // let mut qb = QueryBuilder::new("SELECT routing_key FROM event WHERE aggregate_type = ");
        // qb.push_bind(aggregate_type.to_string());
        // qb.push(" AND aggregate_id = ");
        // qb.push_bind(aggregate_id.to_string());
        // qb.push(" LIMIT 1");
        //
        // let query = qb.build();
        // println!("{}", query.sql());
        // query.execute(&self.0).await;

        // Ok(qb
        //     .build_query_as()
        //     .fetch_optional(&self.0)
        //     .await
        //     .map_err(|e| SaveError::ServerError(e.into()))?
        //     .map(|v: (String,)| v.0))
        todo!()
    }

    async fn bulk_insert(&self, events: Vec<Event>) -> Result<(), SaveError> {
        todo!()
    }

    async fn save_snapshot(&self, snapshot: Snapshot) -> Result<(), SaveError> {
        todo!()
    }
}
