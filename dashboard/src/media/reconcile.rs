//! This node's picture files against what the store knows, once a day
//! (#468).
//!
//! The files and the store drift apart in a few known ways: an upload whose
//! commit failed, or a node that died between storing and recording, leaves
//! files nobody references; a disk restored from an old copy lacks files the
//! store knows, or holds older bytes under a picture's id. The subscription
//! only sees events, so a timer walks `media.dir` instead: for each tenant it
//! lists the files on disk and the rows of `media_files`, deletes the files
//! the store does not know once they are a day old (an upload in flight is
//! younger than that), and puts the files the store knows and the disk
//! lacks, or holds at the wrong size, on this node's wanted list for the
//! [`Fetcher`] to take from a peer. What a run did is logged.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use timada_core::{RENDITION_EXT, Rendition};
use tokio::task::JoinHandle;

use super::read_model::Known;
use super::{Fetcher, Media};
use crate::peers::is_id;

/// How old a file the store does not know must be before it is an orphan:
/// an upload is stored before it is recorded, and a commit takes seconds,
/// not a day.
pub const ORPHAN_AFTER_SECS: u64 = 24 * 60 * 60;

/// What one run did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Report {
    pub tenants: usize,
    /// Files on disk that the store knows and that are as recorded.
    pub kept: usize,
    /// Files the store does not know, deleted.
    pub orphans_removed: usize,
    /// Files the store does not know yet, too young to be orphans.
    pub orphans_young: usize,
    /// Files held at another size than recorded, deleted and wanted again.
    pub wrong_size: usize,
    /// Files the store knows and the disk lacks, wanted.
    pub missing: usize,
}

/// A file on disk below a tenant's directory.
struct OnDisk {
    rendition: Rendition,
    image_id: String,
    size: u64,
    /// Unix seconds.
    modified: u64,
}

/// Lists the rendition files of one tenant's directory. Blocking.
fn list_files(tenant_dir: &Path) -> std::io::Result<Vec<OnDisk>> {
    let mut files = Vec::new();
    for rendition in Rendition::ALL {
        let dir = tenant_dir.join(rendition.dir());
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => return Err(err),
        };
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let meta = entry.metadata()?;
            if !meta.is_file() {
                continue;
            }
            let modified = meta
                .modified()
                .ok()
                .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |since| since.as_secs());
            // A rendition's file, or a half-written one left behind: either
            // way a file of ours, judged by the store's knowledge of it.
            let image_id = name
                .strip_suffix(&format!(".{RENDITION_EXT}"))
                .or_else(|| name.strip_suffix(&format!(".{RENDITION_EXT}.part")))
                .filter(|id| is_id(id))
                .unwrap_or(name);
            files.push(OnDisk {
                rendition,
                image_id: image_id.to_owned(),
                size: meta.len(),
                modified,
            });
        }
    }
    Ok(files)
}

/// The tenants with a directory under `dir`. Blocking.
fn list_tenants(dir: &Path) -> std::io::Result<Vec<String>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };
    let mut tenants = Vec::new();
    for entry in entries {
        let entry = entry?;
        if let Some(name) = entry.file_name().to_str()
            && is_id(name)
            && entry.metadata()?.is_dir()
        {
            tenants.push(name.to_owned());
        }
    }
    tenants.sort();
    Ok(tenants)
}

/// The daily walk.
pub struct Reconcile {
    media: Arc<Media>,
    fetcher: Arc<Fetcher>,
}

impl Reconcile {
    pub fn new(media: Arc<Media>, fetcher: Arc<Fetcher>) -> Arc<Self> {
        Arc::new(Self { media, fetcher })
    }

    /// Runs every `every`, the first time right away.
    pub fn start(self: Arc<Self>, every: Duration) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(every);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tick.tick().await;
                match self.run(crate::auth::now()).await {
                    Ok(report) => {
                        tracing::info!(?report, "picture files reconciled with the store")
                    }
                    Err(err) => {
                        tracing::error!(error = %format!("{err:#}"), "picture files not reconciled")
                    }
                }
            }
        })
    }

    /// One run at `now` (unix seconds): every tenant with a directory, then
    /// every tenant the store knows files of and the disk has nothing for.
    pub async fn run(&self, now: u64) -> Result<Report> {
        let dir = self.media.dir().to_path_buf();
        let mut tenants = tokio::task::spawn_blocking(move || list_tenants(&dir)).await??;
        let known_tenants: Vec<(String,)> = self
            .fetcher
            .db
            .fetch_all("SELECT DISTINCT tenant_id FROM media_files", ())
            .await?;
        for (tenant_id,) in known_tenants {
            if !tenants.contains(&tenant_id) {
                tenants.push(tenant_id);
            }
        }
        let mut report = Report::default();
        for tenant_id in tenants {
            match self.run_tenant(&tenant_id, now).await {
                Ok(part) => {
                    report.tenants += 1;
                    report.kept += part.kept;
                    report.orphans_removed += part.orphans_removed;
                    report.orphans_young += part.orphans_young;
                    report.wrong_size += part.wrong_size;
                    report.missing += part.missing;
                }
                Err(err) => {
                    tracing::error!(tenant = %tenant_id, error = %format!("{err:#}"), "a tenant's picture files were not reconciled");
                }
            }
        }
        Ok(report)
    }

    async fn run_tenant(&self, tenant_id: &str, now: u64) -> Result<Report> {
        let tenant_dir = self.media.dir().join(tenant_id);
        let on_disk = tokio::task::spawn_blocking(move || list_files(&tenant_dir)).await??;
        let known: Vec<Known> = self
            .fetcher
            .db
            .fetch_all(
                "SELECT tenant_id, image_id, rendition, size, sha256, source, source_id, added_at
                 FROM media_files WHERE tenant_id = ?1",
                (tenant_id,),
            )
            .await?;
        let mut expected: HashMap<(String, Rendition), &Known> = known
            .iter()
            .filter_map(|row| {
                Some((
                    (row.image_id.clone(), Rendition::from_dir(&row.rendition)?),
                    row,
                ))
            })
            .collect();
        let mut report = Report::default();
        let mut wanted: Vec<&Known> = Vec::new();
        for file in &on_disk {
            match expected.remove(&(file.image_id.clone(), file.rendition)) {
                Some(row) if row.size as u64 == file.size => report.kept += 1,
                Some(row) => {
                    // Not the file recorded: gone, and fetched again.
                    tracing::warn!(tenant = %tenant_id, image = %file.image_id, rendition = %file.rendition.dir(), on_disk = file.size, recorded = row.size, "a picture file is not the size recorded: fetched again");
                    self.remove(tenant_id, file).await;
                    wanted.push(row);
                    report.wrong_size += 1;
                }
                None if now.saturating_sub(file.modified) >= ORPHAN_AFTER_SECS => {
                    tracing::info!(tenant = %tenant_id, image = %file.image_id, rendition = %file.rendition.dir(), "a picture file nobody references: deleted");
                    self.remove(tenant_id, file).await;
                    report.orphans_removed += 1;
                }
                None => report.orphans_young += 1,
            }
        }
        // What the store knows and the disk has not got.
        report.missing = expected.len();
        wanted.extend(expected.into_values());
        if !wanted.is_empty() {
            self.fetcher.want_known(&wanted, now).await?;
        }
        Ok(report)
    }

    /// Deletes one file on disk, a half-written one included.
    async fn remove(&self, tenant_id: &str, file: &OnDisk) {
        if is_id(&file.image_id) {
            if let Ok(path) = self
                .media
                .path_of(tenant_id, file.rendition, &file.image_id)
            {
                let _ = tokio::fs::remove_file(&path).await;
                let _ =
                    tokio::fs::remove_file(path.with_extension(format!("{RENDITION_EXT}.part")))
                        .await;
            }
        } else {
            // A name that is not a picture's: whatever it is, it is in our
            // directory and nobody references it.
            let path = self
                .media
                .dir()
                .join(tenant_id)
                .join(file.rendition.dir())
                .join(&file.image_id);
            if let Err(err) = tokio::fs::remove_file(&path).await {
                tracing::warn!(path = %path.display(), %err, "cannot delete a stray file");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::fetcher::tests::{fetcher_alone, wanted};
    use super::super::read_model::Source;
    use super::super::test_support::{png, rendered};
    use super::*;

    /// A file the store knows and the disk holds as recorded stays; one
    /// held at another size goes and is wanted again; one the disk lacks is
    /// wanted; one the store does not know goes once it is a day old.
    #[tokio::test]
    async fn files_are_judged_by_what_the_store_knows() {
        let (_store, fetcher) = fetcher_alone().await;
        let reconcile = Reconcile::new(fetcher.media.clone(), fetcher.clone());
        let upload = rendered(&png(900, 600, [4, 5, 6, 255]));
        let media = &fetcher.media;
        // Known, both files present: i1. Known, listing wrong, page missing: i2.
        for id in ["i1", "i2"] {
            fetcher
                .record("acme", id, &upload.picture, Source::Product, "mat", 1)
                .await
                .unwrap();
        }
        media
            .store("acme", Rendition::Listing, "i1", &upload.listing)
            .await
            .unwrap();
        media
            .store("acme", Rendition::Page, "i1", &upload.page)
            .await
            .unwrap();
        media
            .store("acme", Rendition::Listing, "i2", b"RIFF....WEBPshort")
            .await
            .unwrap();
        // Unknown to the store: an orphan, fresh.
        media
            .store("acme", Rendition::Page, "orphan", &upload.page)
            .await
            .unwrap();
        // The wanted rows the recording left: cleared, so the run's own show.
        fetcher
            .db
            .execute("DELETE FROM media_wanted", ())
            .await
            .unwrap();

        let now = crate::auth::now();
        let report = reconcile.run(now).await.unwrap();
        assert_eq!(
            report,
            Report {
                tenants: 1,
                kept: 2,
                orphans_removed: 0,
                orphans_young: 1,
                wrong_size: 1,
                missing: 1,
            }
        );
        assert!(
            !media
                .path_of("acme", Rendition::Listing, "i2")
                .unwrap()
                .exists()
        );
        assert!(
            media
                .path_of("acme", Rendition::Page, "orphan")
                .unwrap()
                .exists()
        );
        let rows = wanted(&fetcher.db).await;
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| row.image_id == "i2"));

        // A day later the orphan goes; what is wanted stays wanted.
        let report = reconcile.run(now + ORPHAN_AFTER_SECS + 1).await.unwrap();
        assert_eq!(report.orphans_removed, 1);
        assert_eq!(report.orphans_young, 0);
        assert_eq!(report.missing, 2, "i2's two files, still lacking");
        assert!(
            !media
                .path_of("acme", Rendition::Page, "orphan")
                .unwrap()
                .exists()
        );
        assert!(
            media
                .path_of("acme", Rendition::Page, "i1")
                .unwrap()
                .exists()
        );
    }
}
