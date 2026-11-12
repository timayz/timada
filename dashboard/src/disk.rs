//! Writing the files a node keeps beside the store: the pictures
//! ([`crate::media`]) and the personal-data keys ([`crate::key_vault`]).
//!
//! A file is written in full under a temporary name, then takes its own, so
//! a reader never sees half of one. What is written sits in the kernel's
//! cache for up to half a minute (by default) before the disk has it, and a
//! new file's name may get there first: on ext4, say, the name lands with
//! the journal's next commit, which the store's own syncs bring within
//! milliseconds, while the bytes wait for the cache to be written back. A
//! power cut in between leaves the name over an empty file, which the key
//! vault would read as the key's tombstone, and `/media` would serve as a
//! picture a cache keeps for a year. So every file is synced to the disk
//! before it takes its name ([`write_partial`]): a power cut may take a file
//! back, never leave it empty.
//!
//! The name itself is synced too ([`Durability::Synced`]) where something
//! counts on it: a file a majority of the cluster holds before the event
//! naming it commits, the store syncing every write, and a key's tombstone,
//! without which the key comes back. A copy a node takes for itself leaves
//! its name to the kernel ([`Durability::Cached`]): its peers hold the file,
//! and a power cut costs a fetch.

use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Whether a write's name is on the disk when it returns, or only its bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Durability {
    /// The bytes and the name: what a majority of the cluster counts on
    /// before an event names the file (an upload, a key put, and the copies
    /// the peers take of them), and a key's tombstone.
    Synced,
    /// The bytes, the name following within seconds: a copy a node takes
    /// for itself (a fetch, the copy as it joins), which a power cut may
    /// take back, to be fetched again.
    Cached,
}

/// Writes `bytes` to a new file next to `path`, named after it with a random
/// part and `.part` added, so no other write takes the same name and no
/// listing takes it for a file of ours, and syncs it to the disk. Creates
/// the directory first when it is missing. Returns the file's path, for the
/// caller to give it `path`'s name; on an error, it leaves no file behind.
pub fn write_partial(path: &Path, bytes: &[u8]) -> io::Result<PathBuf> {
    let dir = parent(path);
    create_dir_all(dir)?;
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let partial = dir.join(format!("{name}.{}.part", ulid::Ulid::generate()));
    let written = File::create_new(&partial).and_then(|mut file| {
        file.write_all(bytes)?;
        sync(&file, &partial)
    });
    match written {
        Ok(()) => Ok(partial),
        Err(err) => {
            let _ = std::fs::remove_file(&partial);
            Err(err)
        }
    }
}

/// Syncs the names `dir` holds to the disk: a file that took or lost its
/// name in it keeps that through a power cut.
pub fn sync_dir(dir: &Path) -> io::Result<()> {
    sync(&File::open(dir)?, dir)
}

/// Creates `dir` and whatever is missing above it, each directory created
/// synced into its parent, so a file synced into `dir` survives a power cut
/// with the path to it.
fn create_dir_all(dir: &Path) -> io::Result<()> {
    if dir.is_dir() {
        return Ok(());
    }
    let above = parent(dir);
    create_dir_all(above)?;
    match std::fs::create_dir(dir) {
        Ok(()) => sync_dir(above),
        // Created meanwhile, by another write that syncs it.
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(err) => Err(err),
    }
}

/// The directory holding `path`: `.` for a bare name.
fn parent(path: &Path) -> &Path {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    }
}

/// Syncs `file`, open at `path`, to the disk.
fn sync(file: &File, path: &Path) -> io::Result<()> {
    file.sync_all()?;
    synced(path);
    Ok(())
}

/// Notes a path synced, for the tests to tell which writes were.
#[cfg(test)]
fn synced(path: &Path) {
    test_support::SYNCED.lock().unwrap().push(path.to_owned());
}

#[cfg(not(test))]
fn synced(_: &Path) {}

#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::Mutex;

    use super::*;

    /// Every path synced so far, by every test of the binary, in order.
    pub(super) static SYNCED: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

    /// What is synced below one directory, from the moment it is watched.
    pub struct Syncs {
        dir: PathBuf,
        seen: usize,
    }

    impl Syncs {
        pub fn below(dir: &Path) -> Self {
            Self {
                dir: dir.to_owned(),
                seen: SYNCED.lock().unwrap().len(),
            }
        }

        /// What was synced below the directory since the last call, in
        /// order of path, each relative to it: a directory with a `/` after
        /// it (the watched one is `/`), a file by the name it was written
        /// for (`t1/k/k1` for `t1/k/k1.<random>.part`).
        pub fn take(&mut self) -> Vec<String> {
            let synced = SYNCED.lock().unwrap();
            let mut taken: Vec<String> = synced[self.seen..]
                .iter()
                .filter_map(|path| path.strip_prefix(&self.dir).ok())
                .map(|path| {
                    let path = path.to_string_lossy();
                    match path.strip_suffix(".part").and_then(|p| p.rsplit_once('.')) {
                        Some((file, _random)) => file.to_owned(),
                        None => format!("{path}/"),
                    }
                })
                .collect();
            self.seen = synced.len();
            taken.sort_unstable();
            taken
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::Syncs;
    use super::*;

    fn fresh_dir() -> PathBuf {
        std::env::temp_dir().join(format!("timada-disk-{}", ulid::Ulid::generate()))
    }

    #[test]
    fn a_file_is_on_the_disk_before_it_takes_its_name() {
        let root = fresh_dir();
        let mut syncs = Syncs::below(&root);
        let path = root.join("a/b/k1");
        let partial = write_partial(&path, b"sealed").unwrap();
        assert_eq!(partial.parent(), path.parent());
        assert_eq!(std::fs::read(&partial).unwrap(), b"sealed");
        assert!(!path.exists(), "the name is the caller's to give");
        // Each directory created is synced into the one above it, the file
        // under its temporary name.
        assert_eq!(syncs.take(), ["/", "a/", "a/b/k1"]);

        // Another write for the same name takes a name of its own.
        let again = write_partial(&path, b"other").unwrap();
        assert_ne!(again, partial);
        assert_eq!(std::fs::read(&partial).unwrap(), b"sealed");
        assert_eq!(syncs.take(), ["a/b/k1"]);

        std::fs::rename(&again, &path).unwrap();
        sync_dir(path.parent().unwrap()).unwrap();
        assert_eq!(syncs.take(), ["a/b/"]);
        assert_eq!(std::fs::read(&path).unwrap(), b"other");
        let _ = std::fs::remove_dir_all(&root);
    }
}
