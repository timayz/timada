use std::{path::Path, sync::Arc};

use crate::{Aggregator, Event, Executor, SaveError, Snapshot};
use libsql::{Builder, params};
use libsql_migration::content::migrate;
use serde::{Deserialize, Serialize};
use ulid::Ulid;

#[derive(Clone)]
pub struct LibSqlExecutor {
    db: Arc<libsql::Database>,
}

impl LibSqlExecutor {
    pub async fn new(path: impl AsRef<Path>) -> Result<Self, libsql::Error> {
        let db = Builder::new_local(path).build().await?;
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

    async fn get_event_routing_key_base(
        &self,
        aggregate_type: String,
        aggregate_id: Ulid,
    ) -> Result<Option<String>, libsql::Error> {
        let conn = self.db.connect()?;
        let mut stmt = conn.prepare("SELECT routing_key FROM event WHERE aggregate_type = ?1 AND aggregate_id = ?2 LIMIT 1").await?;

        let mut rows = stmt
            .query([aggregate_type.to_string(), aggregate_id.to_string()])
            .await?;

        let Some(row) = rows.next().await? else {
            return Ok(None);
        };

        Ok(row.get_str(0).map(|v| v.to_string()).ok())
    }

    async fn bulk_insert_base(&self, events: Vec<Event>) -> Result<(), libsql::Error> {
        let conn = self.db.connect()?;
        let mut rows = conn.query("select * from event", ()).await.unwrap();
        println!("{:?}", rows.next().await);
        let tx = conn.transaction().await?;

        for event in events {
            tx.execute("INSERT INTO event (id, name, aggregate_type, aggregate_id, version, routing_key, data, metadata) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)", params![
                event.id.to_string(),
                event.name.as_str(),
                event.aggregate_type.as_str(),
                event.aggregate_id.to_string(),
                event.version,
                event.routing_key.as_str(),
                event.data.to_vec(),
                event.metadata.to_vec()
            ]).await?;
        }

        tx.commit().await
    }
}

#[async_trait::async_trait]
impl Executor for LibSqlExecutor {
    async fn get_event_routing_key(
        &self,
        aggregate_type: String,
        aggregate_id: Ulid,
    ) -> Result<Option<String>, SaveError> {
        self.get_event_routing_key_base(aggregate_type, aggregate_id)
            .await
            .map_err(|e| SaveError::ServerError(e.into()))
    }

    async fn bulk_insert(&self, events: Vec<Event>) -> Result<(), SaveError> {
        let Err(e) = self.bulk_insert_base(events).await else {
            return Ok(());
        };

        if e.to_string().contains("(code: 2067)") {
            Err(SaveError::InvalidOriginalVersion)
        } else {
            Err(SaveError::ServerError(e.into()))
        }
    }

    async fn save_snapshot(&self, snapshot: Snapshot) -> Result<(), SaveError> {
        let conn = self.db.connect().unwrap();
        Ok(())
    }
}
