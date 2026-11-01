//! The database file: the store and every read model, one Turso file.
//!
//! The store (`database.path`) holds every tenant's events, the snapshots,
//! the subscription cursors, the consensus journal and the read-model tables
//! of every context, and is opened for several writers at once
//! ([`open_store`]): the node applies events on the store's own connection,
//! the consensus journal writes on its own, and each read model is written by
//! its subscription through [`evento::Turso::transaction`]. Every write is a
//! transaction that runs alongside the others and is run again should two of
//! them change the same rows.

use std::path::Path;

use anyhow::{Context, Result};
use evento::Turso;
use evento::turso::TursoJournal;
use evento::turso::migrate::Migrations;
pub use timada_core::db::Db;

/// The store: the one database file, as the node and the read models in it
/// write it.
pub type Store = Turso;

/// Opens the store at `path`, creating the file and its directory if needed,
/// and applies the pending migrations of its schema (the events', the
/// consensus journal's, then the read models'). The file is opened for
/// several writers at once; one opened before that way stays so.
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

/// The read models, one crate each: its migrations, and the tables they
/// create with what an emptied table must hold again.
struct StoreReadModel {
    migrations: &'static Migrations<'static>,
    tables: &'static [&'static str],
    /// A statement that puts back what an emptied table must hold (a totals
    /// row); `""` when nothing.
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
    StoreReadModel {
        migrations: &timada_catalog::MIGRATIONS,
        tables: timada_catalog::TABLES,
        reset_rows: "",
    },
    StoreReadModel {
        migrations: &timada_pages::MIGRATIONS,
        tables: timada_pages::TABLES,
        reset_rows: "",
    },
    StoreReadModel {
        migrations: &timada_reviews::MIGRATIONS,
        tables: timada_reviews::TABLES,
        reset_rows: "",
    },
    StoreReadModel {
        migrations: &timada_discounts::MIGRATIONS,
        tables: timada_discounts::TABLES,
        reset_rows: "",
    },
    StoreReadModel {
        migrations: &timada_support::MIGRATIONS,
        tables: timada_support::TABLES,
        reset_rows: "",
    },
    StoreReadModel {
        migrations: &timada_orders::MIGRATIONS,
        tables: timada_orders::TABLES,
        reset_rows: "",
    },
    StoreReadModel {
        migrations: &timada_shoppers::MIGRATIONS,
        tables: timada_shoppers::TABLES,
        reset_rows: "",
    },
    StoreReadModel {
        migrations: &timada_invoicing::MIGRATIONS,
        tables: timada_invoicing::TABLES,
        reset_rows: "",
    },
    StoreReadModel {
        migrations: &timada_newsletter::MIGRATIONS,
        tables: timada_newsletter::TABLES,
        reset_rows: "",
    },
    StoreReadModel {
        migrations: &timada_automations::MIGRATIONS,
        tables: timada_automations::TABLES,
        reset_rows: "",
    },
    StoreReadModel {
        migrations: &timada_aliexpress::MIGRATIONS,
        tables: timada_aliexpress::TABLES,
        reset_rows: "",
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
        if !read_model.reset_rows.is_empty() {
            db.execute(read_model.reset_rows, ()).await?;
        }
    }
    Ok(())
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
