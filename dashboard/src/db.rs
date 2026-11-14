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
use evento::turso::migrate::{MIGRATIONS_TABLE, Migrations};
pub use timada_core::db::{Connection, Db};

/// The store: the one database file, as the node and the read models in it
/// write it.
pub type Store = Turso;

/// Opens the store at `path`, creating the file and its directory if needed,
/// and applies the pending migrations of its schema (the events', the
/// consensus journal's, then the read models'). The file is opened for
/// several writers at once; one opened before that way stays so.
///
/// The system's temporary directory must exist and be writable first
/// ([`check_temp_dir`]): the database spills the larger statements through
/// it, and the first one that does would otherwise fail long after the open,
/// with an error that looks like a store problem (#338).
pub async fn open_store(path: &Path) -> Result<Store> {
    prepare_dir(path)?;
    check_temp_dir(&std::env::temp_dir())?;
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

/// Checks that `dir`, the system's temporary directory (`TMPDIR`, or `/tmp`
/// without it), exists and can be written to, by creating and removing a
/// file in it: the database spills its larger statements (sorts, upserts,
/// long `IN` lists) through files there and has no fallback. A shell whose
/// `TMPDIR` names a directory that is gone is the usual way to fail this.
pub fn check_temp_dir(dir: &Path) -> Result<()> {
    match std::fs::metadata(dir) {
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => anyhow::bail!(
            "the temporary directory {} is not a directory; \
             set TMPDIR to an existing, writable directory",
            dir.display()
        ),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => anyhow::bail!(
            "the temporary directory {} does not exist; \
             set TMPDIR to an existing, writable directory",
            dir.display()
        ),
        Err(err) => {
            return Err(err).with_context(|| {
                format!(
                    "cannot read the temporary directory {}; \
                     set TMPDIR to an existing, writable directory",
                    dir.display()
                )
            });
        }
    }
    // A random name (a ULID), as two probes of one process can read the same
    // time: the cluster tests open many stores at once (#574).
    let probe = dir.join(format!(".timada-probe-{}", ulid::Ulid::generate()));
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .with_context(|| {
            format!(
                "cannot write to the temporary directory {}; \
                 set TMPDIR to a writable directory",
                dir.display()
            )
        })?;
    if let Err(err) = std::fs::remove_file(&probe) {
        tracing::warn!(probe = %probe.display(), %err, "cannot remove the temporary-directory probe");
    }
    Ok(())
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

/// A read model in the store's database, one crate each: its migrations,
/// the tables they create, and the subscriptions that fill them.
pub struct ReadModel {
    /// What the `rebuild` command calls it; `None` for tables that are
    /// emptied with a copy of the store but are not rebuilt alone (the
    /// sagas' own bookkeeping).
    pub name: Option<&'static str>,
    /// The cursor keys of the subscriptions that fill its tables, as the
    /// crates name them (a node with a region prefixes them with it).
    pub subscriptions: &'static [&'static str],
    /// The cursor keys of the sagas that keep tables of their own among its
    /// tables: forgotten with the others when it is rebuilt, but no measure
    /// of how far behind it is, as a saga follows its region alone.
    pub sagas: &'static [&'static str],
    migrations: &'static Migrations<'static>,
    tables: &'static [&'static str],
}

/// The read models the `rebuild` command knows, by name.
pub fn read_models() -> impl Iterator<Item = &'static ReadModel> {
    STORE_READ_MODELS
        .iter()
        .filter(|read_model| read_model.name.is_some())
}

/// The read model called `name`, if any.
pub fn read_model(name: &str) -> Option<&'static ReadModel> {
    read_models().find(|read_model| read_model.name == Some(name))
}

const STORE_READ_MODELS: &[ReadModel] = &[
    ReadModel {
        name: Some("identity"),
        subscriptions: &[timada_identity::READ_MODEL_SUBSCRIPTION],
        sagas: &[],
        migrations: &timada_identity::MIGRATIONS,
        tables: timada_identity::TABLES,
    },
    ReadModel {
        name: Some("tenancy"),
        subscriptions: &[
            timada_tenancy::READ_MODEL_SUBSCRIPTION,
            timada_tenancy::MEMBER_AUDIT_SUBSCRIPTION,
            timada_tenancy::ALIEXPRESS_ACCOUNTS_SUBSCRIPTION,
        ],
        sagas: &[],
        migrations: &timada_tenancy::MIGRATIONS,
        tables: timada_tenancy::TABLES,
    },
    ReadModel {
        name: Some("catalog"),
        subscriptions: &[timada_catalog::READ_MODEL_SUBSCRIPTION],
        sagas: &[],
        migrations: &timada_catalog::MIGRATIONS,
        tables: timada_catalog::TABLES,
    },
    ReadModel {
        name: Some("pages"),
        subscriptions: &[timada_pages::READ_MODEL_SUBSCRIPTION],
        sagas: &[],
        migrations: &timada_pages::MIGRATIONS,
        tables: timada_pages::TABLES,
    },
    ReadModel {
        name: Some("reviews"),
        subscriptions: &[timada_reviews::READ_MODEL_SUBSCRIPTION],
        sagas: &[],
        migrations: &timada_reviews::MIGRATIONS,
        tables: timada_reviews::TABLES,
    },
    ReadModel {
        name: Some("discounts"),
        subscriptions: &[timada_discounts::READ_MODEL_SUBSCRIPTION],
        sagas: &[],
        migrations: &timada_discounts::MIGRATIONS,
        tables: timada_discounts::TABLES,
    },
    ReadModel {
        name: Some("support"),
        subscriptions: &[timada_support::READ_MODEL_SUBSCRIPTION],
        sagas: &[],
        migrations: &timada_support::MIGRATIONS,
        tables: timada_support::TABLES,
    },
    ReadModel {
        name: Some("orders"),
        subscriptions: &[timada_orders::READ_MODEL_SUBSCRIPTION],
        sagas: &[],
        migrations: &timada_orders::MIGRATIONS,
        tables: timada_orders::TABLES,
    },
    // The dashboard's own: the mails its sagas owe (`mailroom/schema.rs`).
    ReadModel {
        name: None,
        subscriptions: &[],
        sagas: &[],
        migrations: &crate::mailroom::MIGRATIONS,
        tables: crate::mailroom::TABLES,
    },
    // The pictures' files, as the store knows them (`media/schema.rs`),
    // filled from the catalog's, the pages', the reviews' and the tenants'
    // streams.
    ReadModel {
        name: Some("media"),
        subscriptions: &[crate::media::SUBSCRIPTION, crate::media::LOGO_SUBSCRIPTION],
        sagas: &[],
        migrations: &crate::media::MIGRATIONS,
        tables: crate::media::TABLES,
    },
    ReadModel {
        name: Some("shoppers"),
        subscriptions: &[timada_shoppers::READ_MODEL_SUBSCRIPTION],
        sagas: &[],
        migrations: &timada_shoppers::MIGRATIONS,
        tables: timada_shoppers::TABLES,
    },
    ReadModel {
        name: Some("invoicing"),
        subscriptions: &[timada_invoicing::READ_MODEL_SUBSCRIPTION],
        sagas: &[],
        migrations: &timada_invoicing::MIGRATIONS,
        tables: timada_invoicing::TABLES,
    },
    ReadModel {
        name: Some("newsletter"),
        subscriptions: &[timada_newsletter::READ_MODEL_SUBSCRIPTION],
        sagas: &[],
        migrations: &timada_newsletter::MIGRATIONS,
        tables: timada_newsletter::TABLES,
    },
    ReadModel {
        name: Some("automations"),
        subscriptions: &[timada_automations::READ_MODEL_SUBSCRIPTION],
        // The subscription that starts runs keeps the listeners, the
        // checkouts and the starts left to other nodes among these tables.
        sagas: &[timada_automations::TRIGGER_SUBSCRIPTION],
        migrations: &timada_automations::MIGRATIONS,
        tables: timada_automations::TABLES,
    },
    ReadModel {
        name: Some("aliexpress"),
        subscriptions: &[timada_aliexpress::READ_MODEL_SUBSCRIPTION],
        sagas: &[],
        migrations: &timada_aliexpress::MIGRATIONS,
        tables: timada_aliexpress::TABLES,
    },
];

/// The tables that are this node's own in the store's database, no peer's
/// and no read model's: a copy of the store from a peer leaves them alone.
const NODE_TABLES: &[&Migrations<'static>] = &[
    &timada_aliexpress::NODE_MIGRATIONS,
    &crate::media::NODE_MIGRATIONS,
];

/// Applies the pending migrations of the read models in the store, and of
/// this node's own tables (what [`open_store`] does; for a store a test
/// built in memory).
pub async fn migrate_store_read_models(store: &Store) -> Result<()> {
    let mut conn = store.connect()?;
    let read_models = STORE_READ_MODELS
        .iter()
        .map(|read_model| read_model.migrations);
    for migrations in read_models.chain(NODE_TABLES.iter().copied()) {
        migrations
            .to_latest(&mut conn)
            .await
            .map_err(|(applied, err)| {
                anyhow::anyhow!(
                    "{}: migration failed after {applied} applied: {err}",
                    migrations.app()
                )
            })?;
    }
    Ok(())
}

/// Empties the read models in the store: they are derived data, which the
/// subscriptions fill again from the store once it is copied from a peer.
///
/// Each table is dropped and created again by its crate's migrations rather
/// than emptied row by row: the database keeps a version of every row a
/// transaction deletes until it checkpoints, so deleting a large read model
/// would write it once more to the log, while a drop is one schema change.
/// Nothing else may be using the tables: this runs before the subscriptions
/// start. The migrations' records are forgotten with the tables, so a
/// migration that filled a table (a totals row) fills it again.
pub async fn reset_store_read_models(store: &Store) -> Result<()> {
    let mut conn = store.connect()?;
    for read_model in STORE_READ_MODELS {
        reset_read_model(&mut conn, read_model).await?;
    }
    Ok(())
}

/// Drops `read_model`'s tables and creates them again (see
/// [`reset_store_read_models`]).
async fn reset_read_model(conn: &mut Connection, read_model: &ReadModel) -> Result<()> {
    let app = read_model.migrations.app();
    for table in read_model.tables {
        conn.execute(&format!("DROP TABLE IF EXISTS {table}"), ())
            .await
            .with_context(|| format!("{app}: cannot drop {table}"))?;
    }
    conn.execute(
        &format!("DELETE FROM {MIGRATIONS_TABLE} WHERE app = ?1"),
        [app],
    )
    .await
    .with_context(|| format!("{app}: cannot forget the applied migrations"))?;
    read_model
        .migrations
        .to_latest(conn)
        .await
        .map_err(|(applied, err)| {
            anyhow::anyhow!("{app}: migration failed after {applied} applied: {err}")
        })?;
    Ok(())
}

/// Empties one read model and forgets the cursors of the subscriptions that
/// fill it, so their next run goes through the store from the start and
/// fills it again: what the `rebuild` command does with the server stopped,
/// after a change to the tables or a handler. The cursors go whatever
/// region prefixed them (`{region}.{key}`).
pub async fn rebuild_read_model(store: &Store, read_model: &ReadModel) -> Result<()> {
    let mut conn = store.connect()?;
    reset_read_model(&mut conn, read_model).await?;
    let db = Db::from(store.clone());
    let keys: Vec<(String,)> = db.fetch_all("SELECT key FROM subscriber", ()).await?;
    for (key,) in keys {
        let fills = read_model
            .subscriptions
            .iter()
            .chain(read_model.sagas)
            .any(|own| key == *own || key.ends_with(&format!(".{own}")));
        if fills {
            db.execute("DELETE FROM subscriber WHERE key = ?1", [key.as_str()])
                .await
                .with_context(|| format!("cannot forget the cursor of {key}"))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// After a reset the read-model tables are there again, empty, with
    /// their indexes and the rows their migrations put in (the totals), and
    /// the migrations are recorded again, so the next `open_store` has
    /// nothing to apply.
    #[tokio::test]
    async fn a_reset_recreates_the_read_models_empty() {
        let store = Turso::in_memory_concurrent().await.unwrap();
        migrate_store_read_models(&store).await.unwrap();
        let db = Db::from(store.clone());
        let schema_before = schema(&db).await;
        let migrations_before: i64 = db
            .fetch_scalar(&format!("SELECT count(*) FROM {MIGRATIONS_TABLE}"), ())
            .await
            .unwrap();
        db.execute(
            "INSERT INTO identity_users (id, account, label, is_admin, deleted, registered_at, \
             sign_in_count, oauth2_count, passkey_count, has_password, account_search, label_search) \
             VALUES ('u1', 'ada@example.com', 'Ada', 0, 0, 1, 0, 0, 0, 1, 'ada', 'ada')",
            (),
        )
        .await
        .unwrap();
        db.execute("UPDATE identity_totals SET users = 1 WHERE id = 1", ())
            .await
            .unwrap();

        reset_store_read_models(&store).await.unwrap();

        let users: i64 = db
            .fetch_scalar("SELECT count(*) FROM identity_users", ())
            .await
            .unwrap();
        assert_eq!(users, 0, "the rows are gone");
        let totals: i64 = db
            .fetch_scalar("SELECT users FROM identity_totals WHERE id = 1", ())
            .await
            .unwrap();
        assert_eq!(totals, 0, "the totals row is back at zero");
        assert_eq!(
            schema(&db).await,
            schema_before,
            "tables and indexes as before"
        );
        let migrations_after: i64 = db
            .fetch_scalar(&format!("SELECT count(*) FROM {MIGRATIONS_TABLE}"), ())
            .await
            .unwrap();
        assert_eq!(
            migrations_after, migrations_before,
            "every migration recorded again"
        );
        // Nothing left for the next open to apply.
        migrate_store_read_models(&store).await.unwrap();
    }

    /// Each read model's list of tables is what its migrations create, no
    /// more and no less: a table left off the list would survive a copy of
    /// the store with stale rows, and its migration would fail to run again.
    #[tokio::test]
    async fn every_read_model_lists_the_tables_its_migrations_create() {
        for read_model in STORE_READ_MODELS {
            let store = Turso::in_memory_concurrent().await.unwrap();
            let mut conn = store.connect().unwrap();
            read_model
                .migrations
                .to_latest(&mut conn)
                .await
                .map_err(|(_, err)| err)
                .unwrap();
            let mut created: Vec<String> = schema(&Db::from(store))
                .await
                .into_iter()
                .filter(|(kind, _)| kind == "table")
                .map(|(_, name)| name)
                .collect();
            created.sort();
            let mut listed: Vec<String> = read_model.tables.iter().map(|t| t.to_string()).collect();
            listed.sort();
            assert_eq!(created, listed, "{}", read_model.migrations.app());
        }
    }

    /// A rebuilt read model is empty and its subscription, cursor forgotten
    /// (region prefix and all), fills it again from the start of the store.
    #[tokio::test]
    async fn a_rebuilt_read_model_is_filled_again_from_the_start() {
        use evento::Evento;
        use timada_tenancy::{Command, CreateTenant, active_tenant_ids};

        let turso = Turso::in_memory_concurrent().await.unwrap();
        migrate_store_read_models(&turso).await.unwrap();
        let store = Evento::new(turso.clone()).default_routing_key("eu");
        let db = Db::from(turso.clone());
        let tenant = Command(store.clone())
            .create_tenant(CreateTenant {
                name: "acme".into(),
                owner_user_id: "alice".into(),
            })
            .await
            .unwrap();
        let mut subscription = timada_tenancy::read_model_subscription::<Evento>(db.clone());
        subscription.run_once(&store).await.unwrap();
        assert!(active_tenant_ids(&db).await.unwrap().contains(&tenant.id));
        let key = format!("eu.{}", timada_tenancy::READ_MODEL_SUBSCRIPTION);
        let cursors = |key: String| {
            let db = db.clone();
            async move {
                db.fetch_scalar::<i64>("SELECT count(*) FROM subscriber WHERE key = ?1", [key])
                    .await
                    .unwrap()
            }
        };
        assert_eq!(
            cursors(key.clone()).await,
            1,
            "the cursor carries the region"
        );

        rebuild_read_model(&turso, read_model("tenancy").unwrap())
            .await
            .unwrap();

        assert!(active_tenant_ids(&db).await.unwrap().is_empty(), "emptied");
        assert_eq!(cursors(key.clone()).await, 0, "the cursor is forgotten");
        let mut subscription = timada_tenancy::read_model_subscription::<Evento>(db.clone());
        subscription.run_once(&store).await.unwrap();
        assert!(
            active_tenant_ids(&db).await.unwrap().contains(&tenant.id),
            "filled again from the start"
        );
        assert_eq!(cursors(key).await, 1);
        assert!(
            read_model("mailroom").is_none(),
            "the sagas' tables are not rebuilt alone"
        );
    }

    /// A rebuild also forgets the cursor of a saga that keeps tables among
    /// the read model's (the automations' listeners): it fills them again
    /// from the start of the store, rather than leave them empty.
    #[tokio::test]
    async fn a_rebuild_forgets_the_sagas_that_keep_its_tables() {
        use evento::Evento;

        let turso = Turso::in_memory_concurrent().await.unwrap();
        migrate_store_read_models(&turso).await.unwrap();
        let store = Evento::new(turso.clone()).default_routing_key("eu");
        let db = Db::from(turso.clone());
        let automations = timada_automations::Command::new(store.clone(), "acme");
        let id = automations
            .create_scenario("alice", "Big orders")
            .await
            .unwrap();
        automations.archive_scenario("alice", &id).await.unwrap();
        timada_automations::read_model_subscription::<Evento>(db.clone())
            .run_once(&store)
            .await
            .unwrap();
        timada_automations::listener_subscription::<Evento>(db.clone())
            .run_once(&store)
            .await
            .unwrap();
        let cursor = |key: &str| {
            let db = db.clone();
            let key = format!("eu.{key}");
            async move {
                db.fetch_scalar::<i64>("SELECT count(*) FROM subscriber WHERE key = ?1", [key])
                    .await
                    .unwrap()
            }
        };
        assert_eq!(cursor(timada_automations::TRIGGER_SUBSCRIPTION).await, 1);

        rebuild_read_model(&turso, read_model("automations").unwrap())
            .await
            .unwrap();

        assert_eq!(cursor(timada_automations::READ_MODEL_SUBSCRIPTION).await, 0);
        assert_eq!(
            cursor(timada_automations::TRIGGER_SUBSCRIPTION).await,
            0,
            "the saga's cursor is forgotten with its tables"
        );
    }

    /// Every table and index of the read models, by name: what is neither
    /// the store's own nor the migrations' record.
    async fn schema(db: &Db) -> Vec<(String, String)> {
        let mut rows: Vec<(String, String)> = db
            .fetch_all(
                "SELECT type, name FROM sqlite_schema \
                 WHERE type IN ('table', 'index') AND name NOT LIKE 'sqlite_%' \
                 AND name NOT LIKE '\\_evento%' ESCAPE '\\' \
                 AND name NOT LIKE '\\_\\_turso\\_internal%' ESCAPE '\\' \
                 AND name NOT IN ('event', 'snapshot', 'subscriber', 'dead_letter', 'meta') \
                 AND name NOT LIKE 'idx\\_%' ESCAPE '\\' AND name NOT LIKE 'cluster\\_%' ESCAPE '\\'",
                (),
            )
            .await
            .unwrap();
        rows.sort();
        rows
    }

    /// A fresh directory under the test's own temporary directory, which
    /// exists; the test removes it when it needs one that is gone.
    fn scratch(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("timada-db-tests-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn temp_dir_check_passes_on_a_writable_directory() {
        let dir = scratch("writable");
        check_temp_dir(&dir).unwrap();
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            0,
            "the probe file is removed"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn temp_dir_check_names_a_missing_directory_and_tmpdir() {
        // What a stale `TMPDIR` (a dev shell whose directory was cleaned
        // up) hands `std::env::temp_dir()`: a path that is gone.
        let dir = scratch("missing");
        std::fs::remove_dir_all(&dir).unwrap();
        let err = check_temp_dir(&dir).unwrap_err().to_string();
        assert!(err.contains(&dir.display().to_string()), "{err}");
        assert!(err.contains("does not exist"), "{err}");
        assert!(err.contains("TMPDIR"), "{err}");
    }

    #[test]
    fn temp_dir_check_refuses_a_file() {
        let dir = scratch("file");
        let file = dir.join("not-a-directory");
        std::fs::write(&file, b"").unwrap();
        let err = check_temp_dir(&file).unwrap_err().to_string();
        assert!(err.contains("is not a directory"), "{err}");
        assert!(err.contains("TMPDIR"), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Checks run at once in one process all pass and leave nothing behind:
    /// the cluster tests open many stores at once, and two probes that read
    /// the same time must not take the same name (#574).
    #[test]
    fn temp_dir_checks_pass_when_run_at_once() {
        let dir = scratch("at-once");
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    for _ in 0..2_000 {
                        check_temp_dir(&dir).unwrap();
                    }
                });
            }
        });
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            0,
            "every probe file is removed"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
