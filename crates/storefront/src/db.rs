//! The app's local database, opened the way the dashboard opens its own:
//! pages read through a pool of read-only connections the host sizes, and
//! every write (the catalog copy the subscription keeps, its cursor, the
//! projection snapshots) goes through one connection, so writes queue in
//! order instead of contending for SQLite's lock while holding a connection
//! the pages are waiting for. sqlx runs each connection on a thread of its
//! own: the read pool's size is how many pages read at once.
//!
//! The copy is derived from the store and rebuilt from it when lost, so the
//! writer commits with `synchronous = NORMAL`: a power cut may drop the last
//! commits, in order, never corrupt the file, and the subscription then
//! takes again the events after the cursor that survived.

use std::path::Path;
use std::time::Duration;

use anyhow::Context as _;
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};

/// How long a connection waits on SQLite's lock before failing.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// The read pool's size when the host does not choose one.
pub const DEFAULT_READ_CONNECTIONS: u32 = 8;

/// The local database: [`StoreDb::open`] for an app, or a single pool
/// (`SqlitePool` converts) for a test's in-memory database.
#[derive(Clone)]
pub struct StoreDb {
    read: SqlitePool,
    write: SqlitePool,
}

impl StoreDb {
    /// Opens (creating it if missing) the database at `path`: the writer
    /// first, which sets the journal mode, then `read_connections`
    /// read-only connections, opened as pages need them.
    pub async fn open(path: impl AsRef<Path>, read_connections: u32) -> anyhow::Result<Self> {
        let path = path.as_ref();
        let write = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                options(path)
                    .create_if_missing(true)
                    .journal_mode(SqliteJournalMode::Wal)
                    .synchronous(SqliteSynchronous::Normal),
            )
            .await
            .with_context(|| format!("cannot open {}", path.display()))?;
        let read = SqlitePoolOptions::new()
            .max_connections(read_connections.max(1))
            .min_connections(0)
            .idle_timeout(Some(Duration::from_secs(30)))
            .connect_with(options(path).read_only(true))
            .await
            .with_context(|| format!("cannot open {} read-only", path.display()))?;
        tracing::info!(read_connections, path = %path.display(), "local database ready");
        Ok(Self { read, write })
    }

    /// What pages read through.
    pub fn read(&self) -> &SqlitePool {
        &self.read
    }

    /// The one writer: the catalog copy, the cursor, the snapshots, the
    /// migrations.
    pub fn write(&self) -> &SqlitePool {
        &self.write
    }

    pub async fn close(&self) {
        self.read.close().await;
        self.write.close().await;
    }
}

/// One pool for both: a test's in-memory database, which a second pool
/// would not see.
impl From<SqlitePool> for StoreDb {
    fn from(pool: SqlitePool) -> Self {
        Self {
            read: pool.clone(),
            write: pool,
        }
    }
}

/// The settings every connection gets, as the dashboard's (`dashboard/src/db.rs`).
fn options(path: &Path) -> SqliteConnectOptions {
    SqliteConnectOptions::new()
        .filename(path)
        .busy_timeout(BUSY_TIMEOUT)
        .pragma("wal_autocheckpoint", "1000")
        .pragma("journal_size_limit", "67108864") // 64 MiB cap on -wal growth
        .pragma("cache_size", "-20000") // 20 MiB page cache
        .pragma("mmap_size", "268435456") // 256 MiB memory-mapped I/O
        .pragma("temp_store", "memory")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn pages_read_what_the_writer_wrote_and_cannot_write() {
        let dir = std::env::temp_dir().join(format!("storefront-db-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = StoreDb::open(dir.join("local.db"), 2).await.unwrap();
        sqlx::query("CREATE TABLE t (v INTEGER)")
            .execute(db.write())
            .await
            .unwrap();
        sqlx::query("INSERT INTO t VALUES (1)")
            .execute(db.write())
            .await
            .unwrap();
        let (v,): (i64,) = sqlx::query_as("SELECT v FROM t")
            .fetch_one(db.read())
            .await
            .unwrap();
        assert_eq!(v, 1);
        assert!(
            sqlx::query("INSERT INTO t VALUES (2)")
                .execute(db.read())
                .await
                .is_err()
        );
        db.close().await;
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
