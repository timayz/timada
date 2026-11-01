//! The database file, and the read-model pools of the tables not yet in it.
//!
//! The store is one file (`database.path`): every tenant's events, the
//! snapshots, the subscription cursors and the consensus journal, opened for
//! several writers at once ([`open_store`]): the node applies events on the
//! store's own connection, the consensus journal writes on its own, and the
//! read models that have moved into the file are written by their
//! subscriptions through [`evento::Turso::transaction`]. Every write is a
//! transaction that runs alongside the others and is run again should two of
//! them change the same rows.
//!
//! The read models still in their own file (`database.read_models`) keep
//! the two pools of before, as in imkitchen: a read-only pool sized for
//! concurrent reads and a single-connection write pool, so every write is
//! serialized up front instead of failing with SQLITE_BUSY mid-transaction.
//! The write side runs WAL with `synchronous = FULL`.

use std::ops::Deref;
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use anyhow::{Context, Result};
use evento::Turso;
use evento::turso::TursoJournal;
use evento::turso::migrate::Migrations;
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous,
};
use sqlx_migrator::{Info, Migrator};
pub use timada_core::db::Db;

/// The store: the one database file, as the node and the read models in it
/// write it.
pub type Store = Turso;

/// The read models' read-only pool, as registered in topcoat's app context:
/// `let db: &ReadPool = app_context(cx);` then use it as a `SqlitePool`.
pub struct ReadPool(pub SqlitePool);
/// The read models' single-connection write pool, as registered in topcoat's
/// app context.
pub struct WritePool(pub SqlitePool);

impl Deref for ReadPool {
    type Target = SqlitePool;
    fn deref(&self) -> &SqlitePool {
        &self.0
    }
}

impl Deref for WritePool {
    type Target = SqlitePool;
    fn deref(&self) -> &SqlitePool {
        &self.0
    }
}

/// `busy_timeout` for the `serve` pools.
const SERVE_BUSY_TIMEOUT: Duration = Duration::from_secs(5);
/// `busy_timeout` for `migrate` / `reset`: opening a database with a large
/// leftover `-wal` from an unclean shutdown can take well over 5s on slow
/// storage.
pub const MAINTENANCE_BUSY_TIMEOUT: Duration = Duration::from_secs(60);

/// Opens the store at `path`, creating the file and its directory if needed,
/// and applies the pending migrations of its schema (the events', the
/// consensus journal's, then the read models' that live in it). The file is
/// opened for several writers at once; one opened before that way stays so.
pub async fn open_store(path: &Path) -> Result<Store> {
    prepare_dir(path)?;
    let store = Turso::open_concurrent(path)
        .await
        .with_context(|| format!("cannot open {}", path.display()))?;
    // The journal's tables, so a `migrate` leaves nothing for `serve` to
    // create; the node builds its journal over the same database later.
    TursoJournal::new(store.database().clone())
        .await
        .with_context(|| format!("cannot migrate the consensus journal in {}", path.display()))?;
    migrate_store_read_models(&store)
        .await
        .with_context(|| format!("cannot migrate the read models in {}", path.display()))?;
    tracing::info!(path = %path.display(), "store ready");
    Ok(store)
}

/// The read models that live in the store's database, one crate each: its
/// migrations, and the tables they create with what an emptied table must
/// hold again. The crates still on `database.read_models` join this list as
/// they move.
struct StoreReadModel {
    migrations: &'static Migrations<'static>,
    tables: &'static [&'static str],
    reset_rows: &'static str,
}

const STORE_READ_MODELS: &[StoreReadModel] = &[
    StoreReadModel {
        migrations: &timada_identity::MIGRATIONS,
        tables: timada_identity::TABLES,
        reset_rows: timada_identity::RESET_ROWS,
    },
    StoreReadModel {
        migrations: &timada_tenancy::MIGRATIONS,
        tables: timada_tenancy::TABLES,
        reset_rows: timada_tenancy::RESET_ROWS,
    },
];

/// Applies the pending migrations of the read models in the store (what
/// [`open_store`] does; for a store a test built in memory).
pub async fn migrate_store_read_models(store: &Store) -> Result<()> {
    let mut conn = store.connect()?;
    for read_model in STORE_READ_MODELS {
        read_model
            .migrations
            .to_latest(&mut conn)
            .await
            .map_err(|(applied, err)| {
                anyhow::anyhow!(
                    "{}: migration failed after {applied} applied: {err}",
                    read_model.migrations.app()
                )
            })?;
    }
    Ok(())
}

/// Empties the read models in the store: they are derived data, which the
/// subscriptions fill again from the store once it is copied from a peer.
pub async fn reset_store_read_models(store: &Store) -> Result<()> {
    let db = Db::from(store.clone());
    for read_model in STORE_READ_MODELS {
        for table in read_model.tables {
            db.execute(&format!("DELETE FROM {table}"), ()).await?;
        }
        db.execute(read_model.reset_rows, ()).await?;
    }
    Ok(())
}

/// The files that make up the store at `path`: the file itself, its
/// write-ahead log and its commit log. They belong together: copied, moved
/// or deleted as one.
pub fn store_files(path: &Path) -> [std::path::PathBuf; 3] {
    [
        path.to_path_buf(),
        with_suffix(path, "-wal"),
        with_suffix(path, "-log"),
    ]
}

/// Deletes the store at `path`, with its logs and the lock file its
/// connections may have left; nothing when there is none.
pub fn remove_store(path: &Path) -> Result<()> {
    for file in store_files(path)
        .iter()
        .chain([with_suffix(path, "-shm")].iter())
    {
        match std::fs::remove_file(file) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(err).with_context(|| format!("cannot remove {}", file.display()));
            }
        }
    }
    Ok(())
}

/// `path` with `suffix` appended to its file name.
pub fn with_suffix(path: &Path, suffix: &str) -> std::path::PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    std::path::PathBuf::from(name)
}

/// Creates the directory `path` lives in.
pub fn prepare_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
    }
    Ok(())
}

/// Per-connection options shared by every pool. sqlx re-applies these pragmas
/// every time it opens a connection, including replacements after the idle
/// timeout. Journal mode and synchronous are write-side concerns, set by the
/// pools that own the file (`PRAGMA journal_mode = WAL` fails on a read-only
/// connection).
fn base_options(database_url: &str, busy_timeout: Duration) -> Result<SqliteConnectOptions> {
    Ok(SqliteConnectOptions::from_str(database_url)?
        .busy_timeout(busy_timeout)
        .foreign_keys(true)
        .pragma("wal_autocheckpoint", "1000")
        .pragma("journal_size_limit", "67108864") // 64 MiB cap on -wal growth
        .pragma("cache_size", "-20000") // 20 MiB page cache
        .pragma("mmap_size", "268435456") // 256 MiB memory-mapped I/O
        .pragma("temp_store", "memory"))
}

/// Read-only pool for `serve`, sized by `database.max_connections`. Open the
/// write pool first: a read-only connection cannot create the file.
pub async fn create_read_pool(database_url: &str, max_connections: u32) -> Result<SqlitePool> {
    let options = base_options(database_url, SERVE_BUSY_TIMEOUT)?.read_only(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(max_connections)
        .min_connections(0)
        .idle_timeout(Some(Duration::from_secs(30)))
        .connect_with(options)
        .await
        .with_context(|| format!("cannot open {database_url} read-only"))?;

    tracing::info!(max_connections, "read pool ready");
    Ok(pool)
}

/// Read-write pool for `serve`: one connection, so writes never contend
/// with each other and queue in order instead of backing off on SQLite's
/// lock. `synchronous = FULL`: an acked write must survive power loss.
pub async fn create_write_pool(database_url: &str) -> Result<SqlitePool> {
    prepare_database_dir(database_url)?;
    let options = base_options(database_url, SERVE_BUSY_TIMEOUT)?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Full);

    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .with_context(|| format!("cannot open {database_url}"))?;

    tracing::info!("write pool ready (1 connection)");
    Ok(pool)
}

/// Pool for `migrate` / `reset`: the only pool open, so it owns the WAL and
/// synchronous settings too.
pub async fn create_pool(
    database_url: &str,
    max_connections: u32,
    busy_timeout: Duration,
) -> Result<SqlitePool> {
    prepare_database_dir(database_url)?;
    let options = base_options(database_url, busy_timeout)?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Full);

    let pool = SqlitePoolOptions::new()
        .max_connections(max_connections)
        .connect_with(options)
        .await
        .with_context(|| format!("cannot open {database_url}"))?;

    tracing::info!(max_connections, "maintenance pool ready");
    Ok(pool)
}

/// Creates the directory the database file lives in. `sqlx::Sqlite::create_database`
/// does not, so `migrate` calls this before it.
pub fn prepare_database_dir(database_url: &str) -> Result<()> {
    let options = SqliteConnectOptions::from_str(database_url)?;
    prepare_dir(options.get_filename())
}

/// The bounded contexts' read models still in their own file.
pub fn read_model_migrator() -> Result<Migrator<sqlx::Sqlite>, sqlx_migrator::Error> {
    let mut migrator = Migrator::default();
    migrator.add_migrations(timada_catalog::migrations())?;
    migrator.add_migrations(timada_orders::migrations())?;
    migrator.add_migrations(timada_shoppers::migrations())?;
    migrator.add_migrations(timada_discounts::migrations())?;
    migrator.add_migrations(timada_reviews::migrations())?;
    migrator.add_migrations(timada_support::migrations())?;
    migrator.add_migrations(timada_newsletter::migrations())?;
    migrator.add_migrations(timada_pages::migrations())?;
    migrator.add_migrations(timada_automations::migrations())?;
    migrator.add_migrations(timada_invoicing::migrations())?;
    migrator.add_migrations(timada_aliexpress::migrations())?;
    Ok(migrator)
}

/// Creates the file if needed and applies the pending migrations of
/// `migrator` to it, on a pool of its own.
pub async fn migrate(url: &str, migrator: Migrator<sqlx::Sqlite>) -> Result<()> {
    use sqlx::migrate::MigrateDatabase;
    use sqlx_migrator::{Migrate, Plan};

    prepare_database_dir(url)?;
    if !sqlx::Sqlite::database_exists(url).await? {
        tracing::info!(url, "database does not exist, creating it");
        sqlx::Sqlite::create_database(url).await?;
    }
    let pool = create_pool(url, 1, MAINTENANCE_BUSY_TIMEOUT).await?;
    let mut conn = pool.acquire().await?;
    // Collapse any leftover `-wal` from a previous unclean shutdown up front, so
    // the migration transaction does not hit SQLITE_BUSY while WAL recovery runs
    // lazily. The long busy_timeout on this pool rides out slow storage.
    sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
        .execute(&mut *conn)
        .await?;
    migrator.run(&mut *conn, &Plan::apply_all()).await?;
    drop(conn);
    pool.close().await;
    Ok(())
}

/// Drops the file, if there, then creates and migrates it again.
pub async fn recreate(url: &str, migrator: Migrator<sqlx::Sqlite>) -> Result<()> {
    use sqlx::migrate::MigrateDatabase;

    if sqlx::Sqlite::database_exists(url).await? {
        tracing::warn!(url, "dropping existing database");
        sqlx::Sqlite::drop_database(url).await?;
    }
    migrate(url, migrator).await
}

/// Recreates the read models from nothing: they are derived data, which the
/// subscriptions fill again from the store.
pub async fn recreate_read_models(url: &str) -> Result<()> {
    recreate(url, read_model_migrator()?).await
}

/// Empties the store of its events, snapshots and subscription cursors and
/// keeps the consensus journal (the `accord_*` tables): what a node does
/// before copying a peer's store (see `evento-accord`'s operations guide,
/// "Opening a node by joining").
pub async fn empty_store(store: &Store) -> Result<()> {
    store
        .transaction(&(), |tx, ()| {
            Box::pin(async move {
                tx.execute("DELETE FROM event", ())
                    .await
                    .context("cannot empty the events")?;
                tx.execute("DELETE FROM snapshot", ())
                    .await
                    .context("cannot empty the snapshots")?;
                tx.execute("DELETE FROM subscriber", ())
                    .await
                    .context("cannot empty the subscription cursors")?;
                Ok(())
            })
        })
        .await
}
