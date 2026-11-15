//! `snapshot save|restore <dir>`: keeps a node's whole platform as it is (the
//! store, the read models, the pictures, the shopper keys) and puts it back.
//! For a development platform seeded once and brought back to that state in
//! seconds, a load test that starts every run from the same files, or a
//! node's backup.
//!
//! With the server stopped the store is saved as its files (the database,
//! its write-ahead log and its commit log: a consistent set while nothing
//! writes), and so is a database kept with `--also` (a storefront's own,
//! whose app must be stopped too). A restore replaces the files (and drops
//! the logs the replaced ones had and a pending rejoin mark), and the
//! directories, with the server stopped.
//!
//! `save` also runs while the server serves (#593): it then asks the server
//! for the store ([`crate::store_copy`]), which copies its files while it
//! keeps them fit to be copied and the writes go on, and copies the keys and
//! the pictures after it. A key and a picture are on the disk
//! before the event that names them, so the copy holds every one its store
//! names, but for those removed while it was taken: a picture deleted
//! meanwhile is missing where the kept store still shows it, and an account
//! deleted meanwhile stays unreadable in a store that still has it, its key
//! gone as it must be. The copy says it was taken that way
//! ([`WHILE_SERVING`]).
//!
//! A cluster node's store holds its consensus journal: brought back alone to
//! an older state, it is an older copy of itself among peers that went on,
//! which the cluster refuses. On a cluster, `restore` therefore runs only
//! when told that every node is restored from copies taken together, with
//! all of them stopped. A copy taken while a cluster's node served is one
//! node's at a moment of its own, with a journal no other node's copy
//! agrees with: nothing restores it yet (#597), on a cluster or a node
//! alone. A cluster's node that lost its files needs no copy: it takes the
//! store from its peers (`rejoin`).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use clap::{Args, Subcommand};

use crate::config::Config;
use crate::db::{store_files, with_suffix};

#[derive(Args, Debug)]
pub struct SnapshotArgs {
    #[command(subcommand)]
    pub action: SnapshotAction,
}

#[derive(Subcommand, Debug)]
pub enum SnapshotAction {
    /// Keep the platform in a directory, the server stopped or serving
    Save {
        dir: PathBuf,
        /// Replace the directory when it holds a snapshot already
        #[arg(long)]
        replace: bool,
        /// Another file kept with it (repeatable): a database, such as a
        /// storefront's, is saved with its logs (its app stopped), anything
        /// else copied
        #[arg(long = "also")]
        also: Vec<PathBuf>,
    },
    /// Put a kept platform back, replacing this node's (server stopped)
    Restore {
        dir: PathBuf,
        /// On a cluster: every node is restored from copies taken together,
        /// all of them stopped
        #[arg(long)]
        all_nodes: bool,
        /// A file kept with `--also`, put back where it was (repeatable)
        #[arg(long = "also")]
        also: Vec<PathBuf>,
    },
}

/// What a snapshot holds, where this node keeps it.
struct Paths {
    store: PathBuf,
    media: PathBuf,
    data_keys: PathBuf,
}

impl Paths {
    fn of(config: &Config) -> Result<Self> {
        Ok(Self {
            store: config.database.path.clone(),
            media: config.media.dir.clone(),
            data_keys: config.data_keys.dir.clone(),
        })
    }
}

/// The names inside a snapshot directory.
const STORE: &str = "store.db";
const MEDIA: &str = "media";
const DATA_KEYS: &str = "shopper-keys";
const ALSO: &str = "also";
/// Present in a snapshot taken while its server served, and says of what:
/// [`ALONE`] or [`CLUSTER_NODE`].
const WHILE_SERVING: &str = "taken-while-serving";
const ALONE: &str = "a node alone\n";
const CLUSTER_NODE: &str = "a node of a cluster\n";

pub async fn snapshot(config: Config, args: SnapshotArgs) -> Result<()> {
    let paths = Paths::of(&config)?;
    match args.action {
        SnapshotAction::Save { dir, replace, also } => {
            let serving = match crate::store_copy::Serving::find(&paths.store).await? {
                Some(server) => Some(Serving {
                    server,
                    cluster_node: config.cluster.is_some(),
                }),
                // No server answers for this store: none must hold its port
                // either, one from before it answered.
                None => {
                    super::refuse_if_serving(&config).await?;
                    None
                }
            };
            let cannot_restore = serving.as_ref().is_some_and(|s| s.cluster_node);
            save(&paths, &dir, replace, &also, serving).await?;
            if cannot_restore {
                tracing::warn!(
                    dir = %dir.display(),
                    "platform kept while this node served; nothing restores a copy taken while a \
                     cluster's node served yet (#597): a restore of the cluster takes copies of \
                     every node, taken together with all of them stopped"
                );
            } else {
                tracing::info!(dir = %dir.display(), "platform kept");
            }
        }
        SnapshotAction::Restore {
            dir,
            all_nodes,
            also,
        } => {
            super::refuse_if_serving(&config).await?;
            restorable(&dir, config.cluster.is_some(), all_nodes)?;
            restore(&paths, &dir, &also).await?;
            tracing::info!(dir = %dir.display(), "platform restored");
        }
    }
    Ok(())
}

/// Whether the snapshot in `dir` may be put back on this node, of a cluster
/// or alone.
fn restorable(dir: &Path, cluster_node: bool, all_nodes: bool) -> Result<()> {
    match std::fs::read_to_string(dir.join(WHILE_SERVING)) {
        Ok(taken_on) => {
            ensure!(
                taken_on != CLUSTER_NODE,
                "{} was taken while a cluster's node served: it is that node at a moment of its \
                 own, which no other node's copy agrees with, and nothing restores it yet (#597)",
                dir.display()
            );
            ensure!(
                !cluster_node,
                "{} was taken while its node served, alone: a cluster is restored from copies of \
                 every node, taken together with all of them stopped",
                dir.display()
            );
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => ensure!(
            !cluster_node || all_nodes,
            "this node is part of a cluster: restored alone, it would be an older copy of itself \
             that the other nodes refuse. Restore every node from copies taken together, all \
             stopped, and say so with --all-nodes"
        ),
        Err(err) => {
            return Err(err).with_context(|| format!("cannot read {}", dir.display()));
        }
    }
    Ok(())
}

/// The server over the store, asked for its copy.
struct Serving {
    server: crate::store_copy::Serving,
    /// Whether it is a node of a cluster.
    cluster_node: bool,
}

async fn save(
    paths: &Paths,
    dir: &Path,
    replace: bool,
    also: &[PathBuf],
    serving: Option<Serving>,
) -> Result<()> {
    ensure!(
        !joining_marker(&paths.store).exists(),
        "the store is marked for a copy from another node: start the node first, or let it \
         start again"
    );
    if dir.exists() {
        ensure!(
            replace,
            "{} exists: pass --replace to keep the platform there instead",
            dir.display()
        );
    }
    // Written aside, then moved in place: a failed save leaves the last one.
    let partial = with_suffix(dir, ".partial");
    if partial.exists() {
        std::fs::remove_dir_all(&partial)?;
    }
    std::fs::create_dir_all(partial.join(ALSO))
        .with_context(|| format!("cannot create {}", partial.display()))?;

    // The store first: the keys and the pictures it names are on the disk
    // before it names them, so those copied after it are all it needs.
    match serving {
        Some(Serving {
            server,
            cluster_node,
        }) => {
            server.copy(&partial.join(STORE)).await?;
            let taken_on = if cluster_node { CLUSTER_NODE } else { ALONE };
            std::fs::write(partial.join(WHILE_SERVING), taken_on)?;
        }
        None => copy_store(&paths.store, &partial.join(STORE))?,
    }
    copy_dir(&paths.media, &partial.join(MEDIA))?;
    copy_dir(&paths.data_keys, &partial.join(DATA_KEYS))?;
    for file in also {
        let name = file_name(file)?;
        if is_sqlite(file)? {
            copy_store(file, &partial.join(ALSO).join(name))?;
        } else {
            std::fs::copy(file, partial.join(ALSO).join(name))
                .with_context(|| format!("cannot copy {}", file.display()))?;
        }
    }

    if dir.exists() {
        std::fs::remove_dir_all(dir)?;
    }
    std::fs::rename(&partial, dir)
        .with_context(|| format!("cannot move the snapshot to {}", dir.display()))?;
    Ok(())
}

async fn restore(paths: &Paths, dir: &Path, also: &[PathBuf]) -> Result<()> {
    ensure!(
        dir.join(STORE).is_file(),
        "{} holds no {STORE}: not a snapshot",
        dir.display()
    );
    for file in also {
        let kept = dir.join(ALSO).join(file_name(file)?);
        ensure!(
            kept.is_file(),
            "{} was not kept with --also",
            file.display()
        );
    }

    put_store(&dir.join(STORE), &paths.store)?;
    let _ = std::fs::remove_file(joining_marker(&paths.store));
    replace_dir(&dir.join(MEDIA), &paths.media)?;
    replace_dir(&dir.join(DATA_KEYS), &paths.data_keys)?;
    for file in also {
        let kept = dir.join(ALSO).join(file_name(file)?);
        if is_sqlite(&kept)? {
            put_store(&kept, file)?;
        } else {
            put_file(&kept, file)?;
        }
    }
    Ok(())
}

/// Copies the store's files (`db::store_files`) to `to` and the same
/// suffixes: with the server stopped nothing writes them, so the set is
/// consistent, and what the logs hold goes along.
fn copy_store(from: &Path, to: &Path) -> Result<()> {
    ensure!(from.is_file(), "{} does not exist", from.display());
    for (source, target) in store_files(from).iter().zip(store_files(to).iter()) {
        if source.is_file() {
            std::fs::copy(source, target)
                .with_context(|| format!("cannot copy {}", source.display()))?;
        }
    }
    Ok(())
}

/// Puts the store's files kept at `from` in place of those at `to`: a log
/// kept with the file comes back with it, one the file at `to` had goes.
fn put_store(from: &Path, to: &Path) -> Result<()> {
    if let Some(parent) = to.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let _ = std::fs::remove_file(with_suffix(to, "-shm"));
    for (source, target) in store_files(from).iter().zip(store_files(to).iter()) {
        let _ = std::fs::remove_file(target);
        if source.is_file() {
            std::fs::copy(source, target)
                .with_context(|| format!("cannot write {}", target.display()))?;
        }
    }
    Ok(())
}

/// Puts the plain file at `from` (kept with `--also`) in place of the one
/// at `to`.
fn put_file(from: &Path, to: &Path) -> Result<()> {
    if let Some(parent) = to.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(from, to).with_context(|| format!("cannot write {}", to.display()))?;
    Ok(())
}

fn replace_dir(from: &Path, to: &Path) -> Result<()> {
    if to.exists() {
        std::fs::remove_dir_all(to).with_context(|| format!("cannot empty {}", to.display()))?;
    }
    copy_dir(from, to)
}

/// Copies the tree at `from` to `to`; nothing when `from` does not exist (a
/// node that never stored a picture). A file still being written
/// ([`crate::disk::write_partial`]) is none of the tree's yet and is left
/// out, and so is one a serving node removed since it was listed.
fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    if !from.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(to).with_context(|| format!("cannot create {}", to.display()))?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else if kind.is_file() {
            if entry.path().extension().is_some_and(|ext| ext == "part") {
                continue;
            }
            match std::fs::copy(entry.path(), &target) {
                Ok(_) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => {
                    return Err(err)
                        .with_context(|| format!("cannot copy {}", entry.path().display()));
                }
            }
        } else {
            bail!(
                "{} is neither a file nor a directory",
                entry.path().display()
            );
        }
    }
    Ok(())
}

fn is_sqlite(file: &Path) -> Result<bool> {
    use std::io::Read;
    let mut header = [0u8; 16];
    let mut opened =
        std::fs::File::open(file).with_context(|| format!("cannot open {}", file.display()))?;
    Ok(opened.read_exact(&mut header).is_ok() && &header == b"SQLite format 3\0")
}

fn file_name(file: &Path) -> Result<&std::ffi::OsStr> {
    file.file_name()
        .with_context(|| format!("{} names no file", file.display()))
}

/// The rejoin mark next to the store (`cluster::rejoin_mark`).
fn joining_marker(store: &Path) -> PathBuf {
    crate::cluster::rejoin_mark(store)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A row in a storefront's own file, as its library writes one: the
    /// commit lands in the file's log, which the snapshot must carry.
    async fn database(path: &Path, value: i64) {
        write_row(&evento::Turso::open_concurrent(path).await.unwrap(), value).await;
    }

    /// A row in the store's file, written through the store itself.
    async fn store_database(path: &Path, value: i64) {
        write_row(&crate::db::open_store(path).await.unwrap(), value).await;
    }

    async fn write_row(store: &evento::Turso, value: i64) {
        // Schema changes need a plain statement, outside the concurrent
        // transactions the rows are written in.
        store
            .connect()
            .unwrap()
            .execute("CREATE TABLE IF NOT EXISTS t (v INTEGER)", ())
            .await
            .unwrap();
        store
            .transaction(&value, |tx, value| {
                Box::pin(async move {
                    tx.execute("DELETE FROM t", ()).await?;
                    tx.execute("INSERT INTO t VALUES (?1)", (*value,)).await?;
                    Ok(())
                })
            })
            .await
            .unwrap();
    }

    async fn store_value(path: &Path) -> i64 {
        read_row(&crate::db::open_store(path).await.unwrap()).await
    }

    async fn value(path: &Path) -> i64 {
        read_row(&evento::Turso::open_concurrent(path).await.unwrap()).await
    }

    async fn read_row(store: &evento::Turso) -> i64 {
        let conn = store.connect().unwrap();
        let mut rows = conn.query("SELECT v FROM t", ()).await.unwrap();
        rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap()
    }

    #[tokio::test]
    async fn a_kept_platform_comes_back_whole() {
        let root = std::env::temp_dir().join(format!("timada-snapshot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let paths = Paths {
            store: root.join("data/store.db"),
            media: root.join("data/media"),
            data_keys: root.join("data/keys"),
        };
        std::fs::create_dir_all(root.join("data/media/t1")).unwrap();
        std::fs::create_dir_all(&paths.data_keys).unwrap();
        store_database(&paths.store, 1).await;
        std::fs::write(root.join("data/media/t1/a.jpg"), b"picture").unwrap();
        std::fs::write(paths.data_keys.join("k1"), b"key").unwrap();
        let storefront = root.join("storefront.db");
        database(&storefront, 100).await;
        let env = root.join("seed.env");
        std::fs::write(&env, "STORE_TOKEN=tk\n").unwrap();
        // A picture still being written is none of the platform's yet.
        std::fs::write(root.join("data/media/t1/c.jpg.01J.part"), b"half").unwrap();
        let also = [storefront.clone(), env.clone()];

        let kept = root.join("kept");
        save(&paths, &kept, false, &also, None).await.unwrap();
        assert!(
            save(&paths, &kept, false, &also, None).await.is_err(),
            "not replaced unasked"
        );

        // The platform moves on: more rows, a new picture, a lost key.
        store_database(&paths.store, 2).await;
        database(&storefront, 200).await;
        std::fs::write(root.join("data/media/t1/b.jpg"), b"new").unwrap();
        std::fs::remove_file(paths.data_keys.join("k1")).unwrap();
        std::fs::write(&env, "STORE_TOKEN=other\n").unwrap();
        std::fs::write(joining_marker(&paths.store), b"").unwrap();

        restore(&paths, &kept, &also).await.unwrap();
        assert_eq!(store_value(&paths.store).await, 1);
        assert_eq!(value(&storefront).await, 100);
        assert!(root.join("data/media/t1/a.jpg").is_file());
        assert!(!kept.join(MEDIA).join("t1/c.jpg.01J.part").exists());
        assert!(!kept.join(WHILE_SERVING).exists());
        // On a cluster it goes back with every node's, taken together.
        assert!(restorable(&kept, true, false).is_err());
        restorable(&kept, true, true).unwrap();
        restorable(&kept, false, false).unwrap();
        assert!(
            !root.join("data/media/t1/b.jpg").exists(),
            "the platform's later picture goes"
        );
        assert_eq!(std::fs::read(paths.data_keys.join("k1")).unwrap(), b"key");
        assert_eq!(std::fs::read_to_string(&env).unwrap(), "STORE_TOKEN=tk\n");
        assert!(!joining_marker(&paths.store).exists());

        std::fs::remove_dir_all(&root).unwrap();
    }

    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use timada_tenancy::{Command, CreateTenant, read_model_subscription};

    use crate::cluster::Executor;
    use crate::db::Db;

    /// A node alone over the store at `path`, its catalog of tenants kept
    /// up to date.
    async fn node(
        path: &Path,
    ) -> (
        Arc<crate::cluster::Cluster>,
        Executor,
        crate::db::Store,
        evento::subscription::Subscription,
    ) {
        let store = crate::db::open_store(path).await.unwrap();
        let (cluster, executor, _) =
            crate::cluster::open(None, None, &[], false, path, store.clone())
                .await
                .unwrap();
        let catalog = read_model_subscription::<Executor>(Db::from(store.clone()))
            .start(&executor)
            .await
            .unwrap();
        (cluster, executor, store, catalog)
    }

    async fn create_tenant(executor: &Executor, name: &str) -> String {
        Command(executor.clone())
            .create_tenant(CreateTenant {
                name: name.into(),
                owner_user_id: "alice".into(),
            })
            .await
            .unwrap()
            .id
    }

    /// The tenants the catalog finds by a word of their name, once it
    /// lists `expected`.
    async fn found(store: &crate::db::Store, word: &str, expected: &str) -> Vec<String> {
        let db = Db::from(store.clone());
        let phrase = timada_core::db::fts_query(word).unwrap();
        for _ in 0..200 {
            let rows: Vec<(String,)> = db
                .fetch_all(
                    "SELECT id FROM tenancy_tenants
                     WHERE fts_match(slug_search, name_search, ?1) ORDER BY id",
                    (phrase.as_str(),),
                )
                .await
                .unwrap();
            if rows.iter().any(|(id,)| id == expected) {
                return rows.into_iter().map(|(id,)| id).collect();
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("the catalog never listed {expected}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_platform_is_kept_while_it_serves() {
        let root = std::env::temp_dir().join(format!("timada-serving-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let paths = Paths {
            store: root.join("data/store.db"),
            media: root.join("data/media"),
            data_keys: root.join("data/keys"),
        };
        std::fs::create_dir_all(root.join("data/media/t1")).unwrap();
        std::fs::write(root.join("data/media/t1/a.jpg"), b"picture").unwrap();

        // No server yet: nothing answers, nor on a socket one left behind.
        assert!(
            crate::store_copy::Serving::find(&paths.store)
                .await
                .unwrap()
                .is_none()
        );
        std::fs::create_dir_all(root.join("data")).unwrap();
        drop(std::os::unix::net::UnixListener::bind(
            crate::store_copy::socket_path(&paths.store),
        ));
        assert!(
            crate::store_copy::Serving::find(&paths.store)
                .await
                .unwrap()
                .is_none()
        );

        let (cluster, executor, store, catalog) = node(&paths.store).await;
        let copies = crate::store_copy::serve(&paths.store, store.clone()).unwrap();
        let before = create_tenant(&executor, "Before the copy").await;
        found(&store, "before", &before).await;

        // Writes go on while the platform is kept.
        let stop = Arc::new(AtomicBool::new(false));
        let writer = tokio::spawn({
            let (executor, stop) = (executor.clone(), stop.clone());
            async move {
                let mut written = 0;
                while !stop.load(Ordering::Relaxed) {
                    create_tenant(&executor, &format!("Meanwhile {written}")).await;
                    written += 1;
                }
                written
            }
        });
        // Each copy is taken after a tenant of its own was created.
        let mut marks = Vec::new();
        let kept = root.join("kept");
        for n in 0..6 {
            marks.push(create_tenant(&executor, &format!("Marked {n}")).await);
            let kept = if n < 5 {
                root.join(format!("kept-{n}"))
            } else {
                kept.clone()
            };
            let server = crate::store_copy::Serving::find(&paths.store)
                .await
                .unwrap()
                .expect("the server answers");
            let serving = Serving {
                server,
                cluster_node: false,
            };
            save(&paths, &kept, true, &[], Some(serving)).await.unwrap();
        }
        stop.store(true, Ordering::Relaxed);
        assert!(writer.await.unwrap() > 0, "nothing was written meanwhile");
        let after = create_tenant(&executor, "After the copy").await;
        found(&store, "after", &after).await;
        // Every copy opens, and holds what was written before it was taken.
        for (n, mark) in marks.iter().enumerate().take(5) {
            let copy = crate::db::open_store(&root.join(format!("kept-{n}")).join(STORE))
                .await
                .unwrap();
            let conn = copy.connect().unwrap();
            let mut rows = conn
                .query(
                    "SELECT COUNT(*) FROM event WHERE aggregator_id = ?1",
                    (mark.as_str(),),
                )
                .await
                .unwrap();
            let events = rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap();
            assert!(events > 0, "copy {n} lacks the tenant created before it");
        }

        assert_eq!(
            std::fs::read_to_string(kept.join(WHILE_SERVING)).unwrap(),
            ALONE
        );
        restorable(&kept, false, false).unwrap();
        assert!(
            restorable(&kept, true, true).is_err(),
            "one node's copy is no cluster's"
        );
        std::fs::write(kept.join(WHILE_SERVING), CLUSTER_NODE).unwrap();
        assert!(restorable(&kept, false, false).is_err());
        assert!(restorable(&kept, true, true).is_err());

        // Put back on another node alone: it opens, lists and finds what was
        // written before the copy and nothing written after it, and writes.
        let other = Paths {
            store: root.join("other/store.db"),
            media: root.join("other/media"),
            data_keys: root.join("other/keys"),
        };
        restore(&other, &kept, &[]).await.unwrap();
        assert!(root.join("other/media/t1/a.jpg").is_file());
        let (other_cluster, other_executor, other_store, other_catalog) = node(&other.store).await;
        assert_eq!(found(&other_store, "before", &before).await, [before]);
        let new = create_tenant(&other_executor, "After the copy came back").await;
        assert_eq!(found(&other_store, "after", &new).await, [new]);

        for (cluster, catalog) in [(cluster, catalog), (other_cluster, other_catalog)] {
            catalog.stop();
            catalog.stopped().await;
            cluster.shutdown().await;
        }
        copies.shutdown().await;
        assert!(!crate::store_copy::socket_path(&paths.store).exists());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
