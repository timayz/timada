//! evento executor setup and the per-service context.

use anyhow::Result;
use evento::migrator::{Migrate as _, Plan};
use sqlx::SqlitePool;

/// The framework-wide evento executor: reads go to the read pool, writes and
/// subscription cursors go to the single-connection write pool.
pub type Executor = evento::sql::RwSqlite;

/// Everything a service crate needs to talk to storage.
///
/// - `read_pool`: concurrent SQL reads of projection tables
/// - `write_pool`: serialized SQL writes (projection updates, `BEGIN IMMEDIATE`)
/// - `executor`: evento event store on top of the same two pools
#[derive(Clone)]
pub struct ServiceContext {
    pub read_pool: SqlitePool,
    pub write_pool: SqlitePool,
    pub executor: Executor,
}

impl ServiceContext {
    /// Build the context and run evento's own event-store migrations.
    pub async fn new(read_pool: SqlitePool, write_pool: SqlitePool) -> Result<Self> {
        let mut conn = write_pool.acquire().await?;
        evento::sql_migrator::new::<sqlx::Sqlite>()?
            .run(&mut *conn, &Plan::apply_all())
            .await?;
        drop(conn);

        let executor: Executor = (read_pool.clone().into(), write_pool.clone().into()).into();
        Ok(Self {
            read_pool,
            write_pool,
            executor,
        })
    }
}
