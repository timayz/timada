//! Temp-database plumbing shared by this crate's tests.

use std::path::PathBuf;

use sqlx::Sqlite;
use sqlx_migrator::migrator::{Info as _, Migrate as _, Migrator, Plan};
use timada_core::ServiceContext;

pub struct TempDb {
    pub ctx: ServiceContext,
    dir: PathBuf,
}

impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A throwaway on-disk SQLite database with the event store and this crate's
/// read-side tables migrated, wired the way the CLI wires them (one pool).
pub async fn temp_db() -> TempDb {
    let dir = std::env::temp_dir().join(format!("timada-payment-{}", timada_core::new_id()));
    std::fs::create_dir_all(&dir).unwrap();
    let url = format!("sqlite://{}?mode=rwc", dir.join("test.db").display());

    let pool = timada_core::db::create_pool(&url, 4).await.unwrap();

    let mut migrator = Migrator::<Sqlite>::default();
    migrator.add_migrations(crate::migrations()).unwrap();
    let mut conn = pool.acquire().await.unwrap();
    migrator.run(&mut *conn, &Plan::apply_all()).await.unwrap();
    drop(conn);

    let ctx = ServiceContext::new(pool.clone(), pool).await.unwrap();
    TempDb { ctx, dir }
}
