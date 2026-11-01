//! `snapshot save|restore <dir>`: keeps a node's whole platform as it is (the
//! store, the read models, the pictures, the shopper keys) and puts it back,
//! with the server stopped. For a development platform seeded once and
//! brought back to that state in seconds, or a load test that starts every
//! run from the same files.
//!
//! The store is saved as its files (the database, its write-ahead log and
//! its commit log: a consistent set while nothing writes). A SQLite file kept
//! with `--also` (a storefront's) is saved with `VACUUM INTO`: one whole
//! file holding what its write-ahead log held. A restore replaces the files
//! (and drops their logs and a pending rejoin mark), and the directories.
//!
//! A cluster node's store holds its consensus journal: brought back alone to
//! an older state, it is an older copy of itself among peers that went on,
//! which the cluster refuses. On a cluster, `restore` therefore runs only
//! when told that every node is restored from copies taken together, with
//! all of them stopped.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use clap::{Args, Subcommand};
use sqlx::ConnectOptions;
use sqlx::sqlite::SqliteConnectOptions;

use crate::config::Config;
use crate::db::{store_files, with_suffix};

#[derive(Args, Debug)]
pub struct SnapshotArgs {
    #[command(subcommand)]
    pub action: SnapshotAction,
}

#[derive(Subcommand, Debug)]
pub enum SnapshotAction {
    /// Keep the platform in a directory (server stopped)
    Save {
        dir: PathBuf,
        /// Replace the directory when it holds a snapshot already
        #[arg(long)]
        replace: bool,
        /// Another file kept with it (repeatable): a SQLite database, such
        /// as a storefront's, is saved whole, anything else copied
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

pub async fn snapshot(config: Config, args: SnapshotArgs) -> Result<()> {
    super::refuse_if_serving(&config).await?;
    let paths = Paths::of(&config)?;
    match args.action {
        SnapshotAction::Save { dir, replace, also } => {
            save(&paths, &dir, replace, &also).await?;
            tracing::info!(dir = %dir.display(), "platform kept");
        }
        SnapshotAction::Restore {
            dir,
            all_nodes,
            also,
        } => {
            ensure!(
                config.cluster.is_none() || all_nodes,
                "this node is part of a cluster: restored alone, it would be an older copy of itself \
                 that the other nodes refuse. Restore every node from copies taken together, all \
                 stopped, and say so with --all-nodes"
            );
            restore(&paths, &dir, &also).await?;
            tracing::info!(dir = %dir.display(), "platform restored");
        }
    }
    Ok(())
}

async fn save(paths: &Paths, dir: &Path, replace: bool, also: &[PathBuf]) -> Result<()> {
    ensure!(
        !joining_marker(&paths.store).exists(),
        "the store is marked for a copy from another node: start the node first"
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

    copy_store(&paths.store, &partial.join(STORE))?;
    copy_dir(&paths.media, &partial.join(MEDIA))?;
    copy_dir(&paths.data_keys, &partial.join(DATA_KEYS))?;
    for file in also {
        let name = file_name(file)?;
        if is_sqlite(file)? {
            vacuum_into(file, &partial.join(ALSO).join(name)).await?;
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
        put_database(&dir.join(ALSO).join(file_name(file)?), file)?;
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

/// A whole copy of the database at `from`, its write-ahead log included.
async fn vacuum_into(from: &Path, to: &Path) -> Result<()> {
    ensure!(from.is_file(), "{} does not exist", from.display());
    let mut connection = SqliteConnectOptions::new()
        .filename(from)
        .read_only(true)
        .connect()
        .await
        .with_context(|| format!("cannot open {}", from.display()))?;
    sqlx::query("VACUUM INTO ?1")
        .bind(to.to_string_lossy().into_owned())
        .execute(&mut connection)
        .await
        .with_context(|| format!("cannot save {}", from.display()))?;
    Ok(())
}

/// Puts the file at `from` in place of the one at `to`, whose write-ahead
/// log and shared memory go: they belong to the file replaced. A plain file
/// (kept with `--also`) has neither.
fn put_database(from: &Path, to: &Path) -> Result<()> {
    if let Some(parent) = to.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    for suffix in ["-wal", "-shm"] {
        let _ = std::fs::remove_file(with_suffix(to, suffix));
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
/// node that never stored a picture).
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
            std::fs::copy(entry.path(), &target)
                .with_context(|| format!("cannot copy {}", entry.path().display()))?;
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

/// The rejoin mark next to the store (`cluster::joining_marker`).
fn joining_marker(store: &Path) -> PathBuf {
    with_suffix(store, ".joining")
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn database(path: &Path, value: i64) {
        let mut connection = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .connect()
            .await
            .unwrap();
        sqlx::query("CREATE TABLE IF NOT EXISTS t (v INTEGER)")
            .execute(&mut connection)
            .await
            .unwrap();
        sqlx::query("DELETE FROM t")
            .execute(&mut connection)
            .await
            .unwrap();
        sqlx::query("INSERT INTO t VALUES (?1)")
            .bind(value)
            .execute(&mut connection)
            .await
            .unwrap();
        // Left open across the copy would be a server running; closed here,
        // the log may still hold the rows the snapshot must carry. Awaited:
        // a connection dropped closes in the background, and its last
        // checkpoint could land in a file the test has restored since.
        sqlx::Connection::close(connection).await.unwrap();
    }

    /// A row in the store's file, written through the store itself: the
    /// commit lands in the store's log, which the snapshot must carry.
    async fn store_database(path: &Path, value: i64) {
        let store = crate::db::open_store(path).await.unwrap();
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
        let store = crate::db::open_store(path).await.unwrap();
        let conn = store.connect().unwrap();
        let mut rows = conn.query("SELECT v FROM t", ()).await.unwrap();
        rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap()
    }

    async fn value(path: &Path) -> i64 {
        let mut connection = SqliteConnectOptions::new()
            .filename(path)
            .connect()
            .await
            .unwrap();
        sqlx::query_scalar("SELECT v FROM t")
            .fetch_one(&mut connection)
            .await
            .unwrap()
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
        let also = [storefront.clone(), env.clone()];

        let kept = root.join("kept");
        save(&paths, &kept, false, &also).await.unwrap();
        assert!(
            save(&paths, &kept, false, &also).await.is_err(),
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
        assert!(
            !root.join("data/media/t1/b.jpg").exists(),
            "the platform's later picture goes"
        );
        assert_eq!(std::fs::read(paths.data_keys.join("k1")).unwrap(), b"key");
        assert_eq!(std::fs::read_to_string(&env).unwrap(), "STORE_TOKEN=tk\n");
        assert!(!joining_marker(&paths.store).exists());

        std::fs::remove_dir_all(&root).unwrap();
    }
}
