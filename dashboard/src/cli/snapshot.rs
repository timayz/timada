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
//! when told which of two restores it is part of. `--all-nodes`: every node
//! is restored from its own copy, all taken together with every node
//! stopped, and the cluster goes on as it was at that stop.
//! `--from-one-node` (#597): one node's copy, such as one taken while the
//! cluster served, is put back on every node, all of them stopped. Its
//! journal is that node's alone, at a moment of its own, so no node goes on
//! from it: the store is marked ([`crate::cluster::mark_for_founding`]) and
//! each node, at its next start, drops that journal and begins a new
//! cluster over the copy's events, read models and cursors. What the old
//! cluster wrote after the copy is gone, as with any backup. The nodes
//! serve once most of them started over the same copy; one left out comes
//! back with the same restore, or copies the store from the others
//! (`rejoin --drop-journal`). A copy taken while a cluster's node served
//! goes back on a node alone the same way, with no flag. A cluster's node
//! that lost its files needs no copy: it takes the store from its peers
//! (`rejoin`).

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
        /// On a cluster: every node is restored from its own copy, all taken
        /// together with every node stopped
        #[arg(long, conflicts_with = "from_one_node")]
        all_nodes: bool,
        /// On a cluster: this one node's copy is put back on every node, all
        /// of them stopped, and the cluster begins again from it
        #[arg(long)]
        from_one_node: bool,
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
            let one_of_several = serving.as_ref().is_some_and(|s| s.cluster_node);
            save(&paths, &dir, replace, &also, serving).await?;
            if one_of_several {
                tracing::info!(
                    dir = %dir.display(),
                    "platform kept while this node served: the cluster is brought back from it \
                     by putting it on every node, all stopped (snapshot restore --from-one-node)"
                );
            } else {
                tracing::info!(dir = %dir.display(), "platform kept");
            }
        }
        SnapshotAction::Restore {
            dir,
            all_nodes,
            from_one_node,
            also,
        } => {
            super::refuse_if_serving(&config).await?;
            let cluster_node = config.cluster.is_some();
            let put_back = restorable(&dir, cluster_node, all_nodes, from_one_node)?;
            restore(&paths, &dir, &also, put_back).await?;
            match (put_back, cluster_node) {
                (PutBack::AsItWas, _) => {
                    tracing::info!(dir = %dir.display(), "platform restored");
                }
                (PutBack::NewCluster, false) => tracing::info!(
                    dir = %dir.display(),
                    "platform restored from a copy of another node: this node starts over its \
                     events, with a journal of its own"
                ),
                (PutBack::NewCluster, true) => tracing::warn!(
                    dir = %dir.display(),
                    "platform restored: at its next start this node begins a new cluster over \
                     the copy. Put the same copy back on the other nodes before starting any, \
                     with every node of the old cluster stopped; they serve once most of them \
                     started. A node left out refuses writes until it is restored the same way \
                     or copies the store from the others (rejoin --drop-journal)"
                ),
            }
        }
    }
    Ok(())
}

/// How a snapshot's store goes back on a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PutBack {
    /// With its consensus journal: the node goes on from where the copy
    /// left it.
    AsItWas,
    /// As what a new cluster begins with: the journal in the copy is one
    /// node's at a moment of its own, and is dropped at the next start
    /// ([`crate::cluster::mark_for_founding`]).
    NewCluster,
}

/// Whether the snapshot in `dir` may be put back on this node, of a cluster
/// or alone, and how.
fn restorable(
    dir: &Path,
    cluster_node: bool,
    all_nodes: bool,
    from_one_node: bool,
) -> Result<PutBack> {
    let while_serving = match std::fs::read_to_string(dir.join(WHILE_SERVING)) {
        Ok(taken_on) => {
            ensure!(
                taken_on == ALONE || taken_on == CLUSTER_NODE,
                "{} does not say what it was taken on",
                dir.join(WHILE_SERVING).display()
            );
            Some(taken_on)
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(err) => {
            return Err(err).with_context(|| format!("cannot read {}", dir.display()));
        }
    };
    if from_one_node {
        return Ok(PutBack::NewCluster);
    }
    if !cluster_node {
        // A cluster node's journal names peers a node alone never hears.
        return Ok(match while_serving.as_deref() {
            Some(CLUSTER_NODE) => PutBack::NewCluster,
            _ => PutBack::AsItWas,
        });
    }
    match (while_serving.is_some(), all_nodes) {
        (false, true) => Ok(PutBack::AsItWas),
        (true, true) => bail!(
            "{} was taken while its node served: it is one node's copy at a moment of its own, \
             no part of copies taken together. Put it back on every node, all of them stopped, \
             with --from-one-node",
            dir.display()
        ),
        (true, false) => bail!(
            "this node is part of a cluster and {} is one node's copy: put it back on every \
             node, all of them stopped, and say so with --from-one-node. The cluster begins \
             again from it, without what was written after it was taken",
            dir.display()
        ),
        (false, false) => bail!(
            "this node is part of a cluster: restored alone, it would be an older copy of itself \
             that the other nodes refuse. Either restore every node from its own copy, all \
             taken together with every node stopped (--all-nodes), or put this one copy back \
             on every node, all of them stopped (--from-one-node)"
        ),
    }
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
    ensure!(
        !crate::cluster::founding_mark(&paths.store).exists(),
        "the store is marked as what a new cluster begins with, its journal still the one of \
         the copy it came from: start the node first"
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

async fn restore(paths: &Paths, dir: &Path, also: &[PathBuf], put_back: PutBack) -> Result<()> {
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

    // Marked before the store is there: a restore cut short never leaves a
    // copy's store to be opened under the journal that came with it.
    crate::cluster::clear_founding_mark(&paths.store)?;
    if put_back == PutBack::NewCluster {
        crate::cluster::mark_for_founding(&paths.store)?;
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
        write_row(&crate::db::migrate_store(path).await.unwrap(), value).await;
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
        read_row(&crate::db::migrate_store(path).await.unwrap()).await
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

        restore(&paths, &kept, &also, PutBack::AsItWas)
            .await
            .unwrap();
        assert_eq!(store_value(&paths.store).await, 1);
        assert_eq!(value(&storefront).await, 100);
        assert!(root.join("data/media/t1/a.jpg").is_file());
        assert!(!kept.join(MEDIA).join("t1/c.jpg.01J.part").exists());
        assert!(!kept.join(WHILE_SERVING).exists());
        // On a cluster it goes back with every node's, taken together, or
        // alone on every node as what a new cluster begins with.
        assert!(restorable(&kept, true, false, false).is_err());
        assert_eq!(
            restorable(&kept, true, true, false).unwrap(),
            PutBack::AsItWas
        );
        assert_eq!(
            restorable(&kept, true, false, true).unwrap(),
            PutBack::NewCluster
        );
        assert_eq!(
            restorable(&kept, false, false, false).unwrap(),
            PutBack::AsItWas
        );
        assert!(!crate::cluster::founding_mark(&paths.store).exists());
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
        let store = crate::db::migrate_store(path).await.unwrap();
        let (cluster, executor, _) =
            crate::cluster::open(None, None, None, &[], false, path, store.clone())
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
            let copy = crate::db::migrate_store(&root.join(format!("kept-{n}")).join(STORE))
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
        assert_eq!(
            restorable(&kept, false, false, false).unwrap(),
            PutBack::AsItWas
        );
        assert!(
            restorable(&kept, true, true, false).is_err(),
            "one node's copy is no part of copies taken together"
        );
        assert!(restorable(&kept, true, false, false).is_err());
        assert_eq!(
            restorable(&kept, true, false, true).unwrap(),
            PutBack::NewCluster
        );

        // Put back on another node alone: it opens, lists and finds what was
        // written before the copy and nothing written after it, and writes.
        let other = Paths {
            store: root.join("other/store.db"),
            media: root.join("other/media"),
            data_keys: root.join("other/keys"),
        };
        restore(&other, &kept, &[], PutBack::AsItWas).await.unwrap();
        assert!(root.join("other/media/t1/a.jpg").is_file());
        let (other_cluster, other_executor, other_store, other_catalog) = node(&other.store).await;
        assert!(!other_cluster.health().founded_at_start);
        assert_eq!(
            found(&other_store, "before", &before).await,
            std::slice::from_ref(&before)
        );
        let new = create_tenant(&other_executor, "After the copy came back").await;
        assert_eq!(found(&other_store, "after", &new).await, [new]);

        // Had a cluster's node taken it, its journal would be no node's to
        // go on from: a node alone begins over its events instead, with no
        // flag, and a cluster's node when told the copy goes on every node.
        std::fs::write(kept.join(WHILE_SERVING), CLUSTER_NODE).unwrap();
        assert!(restorable(&kept, true, true, false).is_err());
        assert!(restorable(&kept, true, false, false).is_err());
        assert_eq!(
            restorable(&kept, true, false, true).unwrap(),
            PutBack::NewCluster
        );
        let put_back = restorable(&kept, false, false, false).unwrap();
        assert_eq!(put_back, PutBack::NewCluster);
        let third = Paths {
            store: root.join("third/store.db"),
            media: root.join("third/media"),
            data_keys: root.join("third/keys"),
        };
        restore(&third, &kept, &[], put_back).await.unwrap();
        let mark = crate::cluster::founding_mark(&third.store);
        assert!(mark.exists());
        assert!(
            save(&third, &root.join("kept-marked"), false, &[], None)
                .await
                .is_err(),
            "a store still under its copy's journal is not kept"
        );
        let (third_cluster, third_executor, third_store, third_catalog) = node(&third.store).await;
        assert!(third_cluster.health().founded_at_start);
        assert!(third_cluster.health().serving());
        assert!(!mark.exists(), "the mark goes once the node began");
        assert_eq!(
            found(&third_store, "before", &before).await,
            std::slice::from_ref(&before)
        );
        let new = create_tenant(&third_executor, "After the new beginning").await;
        assert_eq!(
            found(&third_store, "beginning", &new).await,
            std::slice::from_ref(&new)
        );
        // Its next start is a plain one, over a journal of its own.
        third_catalog.stop();
        third_catalog.stopped().await;
        third_cluster.shutdown().await;
        drop((third_executor, third_store));
        let (third_cluster, _third_executor, third_store, third_catalog) = node(&third.store).await;
        assert!(!third_cluster.health().founded_at_start);
        assert!(third_cluster.health().serving());
        assert_eq!(found(&third_store, "beginning", &new).await, [new]);
        // A plain restore over a store marked takes the mark away.
        crate::cluster::mark_for_founding(&other.store).unwrap();

        for (cluster, catalog) in [
            (cluster, catalog),
            (other_cluster, other_catalog),
            (third_cluster, third_catalog),
        ] {
            catalog.stop();
            catalog.stopped().await;
            cluster.shutdown().await;
        }
        std::fs::write(kept.join(WHILE_SERVING), ALONE).unwrap();
        restore(&other, &kept, &[], PutBack::AsItWas).await.unwrap();
        assert!(!crate::cluster::founding_mark(&other.store).exists());
        copies.shutdown().await;
        assert!(!crate::store_copy::socket_path(&paths.store).exists());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
