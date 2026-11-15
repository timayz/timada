//! A copy of the store taken while the node serves, for `snapshot save`
//! (#593).
//!
//! The store is three files: the database, its write-ahead log and its
//! commit log. A write lands in the commit log, appended to it, and the
//! database folds the log into the two other files now and then (a
//! checkpoint, every 256 KiB of log), emptying it. Copied under a running
//! server the three would not go together: a checkpoint between two of them
//! leaves a copy that is neither the store before it nor after.
//!
//! The database runs no checkpoint while a transaction is open, though. So
//! the server opens one, on a connection of its own, and copies the files
//! behind it: the database's file and the write-ahead log stand still, and
//! the commit log only grows. What the copy takes of it ends wherever the
//! last write had got to, which the database reads as it would after a power
//! cut: every write whole up to the first one cut short. The copy is the
//! store as stopping the server at that moment would leave it, the events,
//! the read models, the subscription cursors and the consensus journal
//! together, and holds every write that returned before it began. The
//! server checks that the two files did stand still, and gives the copy up
//! otherwise.
//!
//! Only the process that has the store open can hold its checkpoints back,
//! so the server takes the copy and `snapshot save` asks it to, over a
//! socket next to the store's file ([`socket_path`]) that the server's user
//! alone can open. A request is one line, `copy <file>`, and so is its
//! answer: `ok`, or `error <why>`.
//!
//! The copy costs the node what the files take to read. The writes made
//! meanwhile wait in the commit log and are folded in when it ends, every
//! write waiting for that: the longer the copy and the more written during
//! it, the longer that wait (`docs/sizing.md`).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, Result, bail, ensure};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use crate::db::{Store, store_files, with_suffix};

/// How many times the copy's transaction is begun again when the database
/// is at a checkpoint, and how long between two.
const BEGIN_ATTEMPTS: u32 = 200;
const BEGIN_PAUSE: std::time::Duration = std::time::Duration::from_millis(20);

/// The socket of the store at `store`, present while a server serves it.
pub fn socket_path(store: &Path) -> PathBuf {
    with_suffix(store, ".sock")
}

/// Writes a copy of `store`, whose file is `store_path`, to the new file
/// `to` and its logs beside it (`db::store_files`), while the writes go on.
/// The copy is a store of its own: opened, it holds every write that
/// returned before the copy began and none half-done. Nothing is left at
/// `to` when it fails.
pub async fn copy(store: &Store, store_path: &Path, to: &Path) -> Result<()> {
    ensure!(!to.exists(), "{} exists already", to.display());
    let conn = store
        .connect_read()
        .context("cannot open a connection for the copy")?;
    hold_checkpoints(&conn).await?;
    let copied = copy_files(store_files(store_path), store_files(to)).await;
    // The transaction read nothing worth keeping; the connection ends it
    // anyway when dropped.
    let _ = conn.execute("ROLLBACK", ()).await;
    if copied.is_err() {
        let _ = crate::db::remove_store(to);
    }
    copied
}

/// Opens a transaction on `conn` and reads through it, so it is under way:
/// the database runs no checkpoint until it ends.
async fn hold_checkpoints(conn: &evento::turso::turso::Connection) -> Result<()> {
    use evento::turso::turso::Error;

    let mut attempt = 0;
    loop {
        attempt += 1;
        let begun = async {
            conn.execute("BEGIN CONCURRENT", ()).await?;
            let mut rows = conn.query("SELECT COUNT(*) FROM sqlite_schema", ()).await?;
            while rows.next().await?.is_some() {}
            Ok::<_, Error>(())
        };
        match begun.await {
            Ok(()) => return Ok(()),
            // A checkpoint is under way: it lets nothing begin.
            Err(Error::Busy(_)) if attempt < BEGIN_ATTEMPTS => {
                let _ = conn.execute("ROLLBACK", ()).await;
                tokio::time::sleep(BEGIN_PAUSE).await;
            }
            Err(err) => {
                return Err(err).context("cannot hold the store's checkpoints back for the copy");
            }
        }
    }
}

/// What tells a file changed: its size and when it was last written.
fn stamp(file: &Path) -> Option<(u64, std::time::SystemTime)> {
    let meta = std::fs::metadata(file).ok()?;
    Some((meta.len(), meta.modified().ok()?))
}

/// Copies the store's files `from` to `to`, and checks that the database's
/// file and its write-ahead log did not change meanwhile.
async fn copy_files(from: [PathBuf; 3], to: [PathBuf; 3]) -> Result<()> {
    ensure!(from[0].is_file(), "{} does not exist", from[0].display());
    let still = |files: &[PathBuf; 3]| (stamp(&files[0]), stamp(&files[1]));
    let before = still(&from);
    let files = from.clone();
    tokio::task::spawn_blocking(move || {
        for (source, target) in files.iter().zip(to.iter()) {
            if source.is_file() {
                std::fs::copy(source, target)
                    .with_context(|| format!("cannot copy {}", source.display()))?;
            }
        }
        anyhow::Ok(())
    })
    .await
    .context("the copy of the store's files stopped")??;
    ensure!(
        still(&from) == before,
        "the database wrote to {} while it was copied, which the copy counted on it not doing: \
         the copy cannot be trusted and was given up",
        from[0].display()
    );
    Ok(())
}

/// The server's end of the socket, until shut down.
pub struct CopyServer {
    socket: PathBuf,
    task: JoinHandle<()>,
}

impl CopyServer {
    /// Stops taking requests and removes the socket.
    pub async fn shutdown(self) {
        self.task.abort();
        let _ = self.task.await;
        let _ = std::fs::remove_file(&self.socket);
    }
}

/// Takes the copies asked for over the socket of the store at `store_path`,
/// one at a time.
pub fn serve(store_path: &Path, store: Store) -> Result<CopyServer> {
    let store_path = Arc::new(store_path.to_path_buf());
    use std::os::unix::fs::PermissionsExt;

    let socket = socket_path(&store_path);
    // One a server that was killed left behind: nothing listens on it, as
    // two servers never run over one store (`cli::refuse_if_serving`).
    match std::fs::remove_file(&socket) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err).with_context(|| format!("cannot remove {}", socket.display())),
    }
    let listener = UnixListener::bind(&socket)
        .with_context(|| format!("cannot listen on {}", socket.display()))?;
    // Whoever asks has the server write a file wherever the server may.
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("cannot keep {} to this user", socket.display()))?;
    let one_at_a_time = Arc::new(Mutex::new(()));
    let task = tokio::spawn(async move {
        loop {
            let stream = match listener.accept().await {
                Ok((stream, _)) => stream,
                Err(err) => {
                    tracing::warn!(%err, "cannot take a request for a copy of the store");
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    continue;
                }
            };
            let (store, store_path) = (store.clone(), store_path.clone());
            let one_at_a_time = one_at_a_time.clone();
            tokio::spawn(async move {
                if let Err(err) = answer(stream, &store, &store_path, &one_at_a_time).await {
                    tracing::warn!(err = %format!("{err:#}"), "a request for a copy of the store was dropped");
                }
            });
        }
    });
    Ok(CopyServer { socket, task })
}

async fn answer(
    stream: UnixStream,
    store: &Store,
    store_path: &Path,
    one_at_a_time: &Mutex<()>,
) -> Result<()> {
    let mut stream = BufReader::new(stream);
    let mut request = String::new();
    stream.read_line(&mut request).await?;
    let reply = match request.trim_end_matches('\n').strip_prefix("copy ") {
        Some(to) => {
            let _one = one_at_a_time.lock().await;
            let started = std::time::Instant::now();
            match copy(store, store_path, Path::new(to)).await {
                Ok(()) => {
                    tracing::info!(to, took = ?started.elapsed(), "store copied while serving");
                    "ok\n".to_owned()
                }
                Err(err) => {
                    tracing::warn!(to, err = %format!("{err:#}"), "cannot copy the store");
                    // One line, whatever the error holds.
                    format!("error {}\n", format!("{err:#}").replace('\n', " "))
                }
            }
        }
        None => "error not a request this server knows\n".to_owned(),
    };
    stream.get_mut().write_all(reply.as_bytes()).await?;
    Ok(())
}

/// The server over the store at `store_path`, when one serves it.
pub struct Serving(UnixStream);

impl Serving {
    /// `None` when no server serves the store: no socket, or one nothing
    /// listens on any more.
    pub async fn find(store_path: &Path) -> Result<Option<Self>> {
        let socket = socket_path(store_path);
        match UnixStream::connect(&socket).await {
            Ok(stream) => Ok(Some(Self(stream))),
            Err(err)
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                ) =>
            {
                Ok(None)
            }
            Err(err) => Err(err).with_context(|| format!("cannot reach {}", socket.display())),
        }
    }

    /// Has the server write a copy of its store to the new file `to`, and
    /// waits for it.
    pub async fn copy(self, to: &Path) -> Result<()> {
        // The server reads the name from its own directory.
        let to = std::path::absolute(to)?;
        let name = to
            .to_str()
            .filter(|name| !name.contains('\n'))
            .with_context(|| format!("{} is not a name the server can take", to.display()))?;
        let mut stream = BufReader::new(self.0);
        stream
            .get_mut()
            .write_all(format!("copy {name}\n").as_bytes())
            .await
            .context("cannot ask the server for a copy of the store")?;
        let mut reply = String::new();
        stream
            .read_line(&mut reply)
            .await
            .context("the server did not answer")?;
        match reply.trim_end() {
            "ok" => Ok(()),
            "" => bail!("the server stopped before the copy was done"),
            other => bail!(
                "the server could not copy the store: {}",
                other.strip_prefix("error ").unwrap_or(other)
            ),
        }
    }
}
