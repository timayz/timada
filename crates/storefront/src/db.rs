//! The app's local database: one Turso file (SQLite's format in a pure-Rust,
//! in-process engine), opened for several writers at once the way the
//! dashboard opens its own. The catalog copy the subscription keeps, its
//! cursor and the projection snapshots are written in transactions that run
//! alongside the pages' reads and one another, and are run again should two
//! of them change the same rows; nothing queues behind a lock.
//!
//! The copy is derived from the store and rebuilt from it when lost: a lost
//! file costs a resynchronisation, never a sale.

use std::path::Path;

use anyhow::Context as _;
pub use timada_core::db::Db;
use timada_core::db::Turso;

/// The local database: [`StoreDb::open`] for an app, or a store a test built
/// in memory (a `Turso` or a [`Db`] converts).
#[derive(Clone)]
pub struct StoreDb {
    db: Db,
}

impl StoreDb {
    /// Opens (creating it if missing) the database at `path`, for several
    /// writers at once. [`migrate`](crate::migrate) adds the library's tables;
    /// [`Store::connect`](crate::Store::connect) runs it.
    pub async fn open(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref();
        let store = Turso::open_concurrent(path)
            .await
            .with_context(|| format!("cannot open {}", path.display()))?;
        tracing::info!(path = %path.display(), "local database ready");
        Ok(Self {
            db: Db::from(store),
        })
    }

    /// What pages read through, and the subscription writes through.
    pub fn db(&self) -> &Db {
        &self.db
    }

    /// The database as an event store: where the subscription's cursor and
    /// the projection snapshots live.
    pub fn store(&self) -> &Turso {
        self.db.store()
    }
}

impl From<Turso> for StoreDb {
    fn from(store: Turso) -> Self {
        Self {
            db: Db::from(store),
        }
    }
}

impl From<Db> for StoreDb {
    fn from(db: Db) -> Self {
        Self { db }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn pages_read_what_the_subscription_wrote() {
        let dir = std::env::temp_dir().join(format!("storefront-db-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = StoreDb::open(dir.join("local.db")).await.unwrap();
        crate::migrate(db.db()).await.unwrap();
        db.db()
            .execute(
                "INSERT INTO storefront_pages (id, slug, title, body, text, meta_title,
                    meta_description, updated_at)
                 VALUES ('p1', 'legal', 'Legal', '', '', '', '', 1)",
                (),
            )
            .await
            .unwrap();
        let title: String = db
            .db()
            .fetch_scalar(
                "SELECT title FROM storefront_pages WHERE slug = 'legal'",
                (),
            )
            .await
            .unwrap();
        assert_eq!(title, "Legal");
        // The same file opened again sees the row: it was committed.
        drop(db);
        let again = StoreDb::open(dir.join("local.db")).await.unwrap();
        let n: i64 = again
            .db()
            .fetch_scalar("SELECT count(*) FROM storefront_pages", ())
            .await
            .unwrap();
        assert_eq!(n, 1);
        drop(again);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
