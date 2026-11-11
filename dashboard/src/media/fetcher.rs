//! The pictures this node lacks, fetched from the peers off the
//! subscriptions' cursors (#468).
//!
//! The pictures subscription used to fetch inside its pass and fail the pass
//! when no peer had a file, so one picture missing everywhere stalled every
//! tenant's pictures on the node for good. Now the subscription writes what
//! this node wants into `media_wanted` ([`super::read_model`]) and the
//! [`Fetcher`] works the table: woken by the subscription and on a timer, it
//! takes the files due, asks the peers for several at once, and keeps a copy
//! only if it is the file recorded ([`super::Media::fetch_rendition`]).
//!
//! What the peers said decides what happens to the row. Fetched, or held
//! already: the row goes. Every peer that could be asked has no copy: the
//! file is gone for good (an upload whose node died before the copies, a
//! disk lost everywhere), and so is asking again: the row goes, with a
//! warning, and a miss on `/media` will ask once more should a copy turn up.
//! Some peer could not be asked (down, or not serving yet): the row waits,
//! the waits growing ([`timada_core::RETRY_DELAYS_SECS`], half a minute to
//! twelve hours), and once they run out the file is given up on, which
//! `/admin/cluster` shows for a week.
//!
//! The table is this node's own: a copy of the store from a peer leaves it
//! alone, and the subscription catching up afterwards fills it again with
//! every file the store knows, most of which the disk already holds (one
//! `stat` each).

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use timada_core::RETRY_DELAYS_SECS;
use timada_core::db::{Connection, Db, FromRow, execute};
use timada_core::{Rendition, RenditionFile};
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use super::{Fetch, Media};

/// How often the table is looked at without a subscription waking the
/// fetcher: a retry falls due.
pub const CHECK_EVERY: Duration = Duration::from_secs(5);

/// The most files one beat takes; the rest wait for the next.
const FILES_PER_BEAT: i64 = 64;

/// How long a file given up on stays on the cluster page.
const GIVEN_UP_KEPT_SECS: u64 = 7 * 24 * 60 * 60;

/// How long a file waits after a failure of this node's own (the disk, the
/// store) before it is looked at again.
const OWN_FAULT_RETRY_SECS: u64 = 60;

/// One row of `media_wanted`.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct Wanted {
    pub tenant_id: String,
    pub image_id: String,
    /// The rendition's directory name.
    pub rendition: String,
    pub size: i64,
    /// Lowercase hex.
    pub sha256: String,
    /// Unix seconds: when to try next.
    pub due_at: i64,
    pub attempts: i64,
    pub given_up: bool,
    /// Why the last try failed.
    pub reason: String,
    pub updated_at: i64,
}

impl Wanted {
    fn recorded(&self) -> Option<(Rendition, RenditionFile)> {
        let rendition = Rendition::from_dir(&self.rendition)?;
        let known = super::read_model::Known {
            tenant_id: self.tenant_id.clone(),
            image_id: self.image_id.clone(),
            rendition: self.rendition.clone(),
            size: self.size,
            sha256: self.sha256.clone(),
            source: String::new(),
            source_id: String::new(),
            added_at: 0,
        };
        Some((rendition, known.recorded()?))
    }
}

/// How many files this node still wants, and how many it gave up on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Summary {
    pub wanted: i64,
    pub given_up: i64,
}

/// The counts for the cluster page.
pub async fn summary(db: &Db) -> Result<Summary> {
    let (wanted, given_up): (i64, i64) = db
        .fetch_one(
            "SELECT coalesce(sum(CASE WHEN given_up = 0 THEN 1 ELSE 0 END), 0),
                    coalesce(sum(CASE WHEN given_up = 1 THEN 1 ELSE 0 END), 0)
             FROM media_wanted",
            (),
        )
        .await?;
    Ok(Summary { wanted, given_up })
}

/// Puts one rendition file on the wanted list, in the caller's transaction,
/// due now: what the read model's handlers do as they record a picture. A
/// file wanted already is wanted afresh.
pub(super) async fn want(
    tx: &Connection,
    tenant_id: &str,
    image_id: &str,
    rendition: Rendition,
    file: &RenditionFile,
    now: i64,
) -> Result<()> {
    execute(
        tx,
        "INSERT INTO media_wanted (tenant_id, image_id, rendition, size, sha256, due_at, attempts,
            given_up, reason, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, 0, '', ?6)
         ON CONFLICT (tenant_id, image_id, rendition) DO UPDATE SET
            size = excluded.size, sha256 = excluded.sha256, due_at = excluded.due_at,
            attempts = 0, given_up = 0, reason = '', updated_at = excluded.updated_at",
        (
            tenant_id,
            image_id,
            rendition.dir(),
            file.size as i64,
            file.sha256_hex(),
            now,
        ),
    )
    .await?;
    Ok(())
}

/// Works this node's wanted list.
pub struct Fetcher {
    pub(crate) media: Arc<Media>,
    pub(crate) db: Db,
    /// Rung by the subscriptions once they wrote a row.
    wake: Notify,
}

impl Fetcher {
    pub fn new(media: Arc<Media>, db: Db) -> Arc<Self> {
        Arc::new(Self {
            media,
            db,
            wake: Notify::new(),
        })
    }

    pub(crate) fn wake(&self) {
        self.wake.notify_one();
    }

    /// Puts files the store knows on the wanted list, afresh: what the
    /// reconcile found missing or wrong on disk.
    pub(crate) async fn want_known(
        &self,
        files: &[&super::read_model::Known],
        now: u64,
    ) -> Result<()> {
        let rows: Vec<(String, String, Rendition, RenditionFile)> = files
            .iter()
            .filter_map(|known| {
                Some((
                    known.tenant_id.clone(),
                    known.image_id.clone(),
                    Rendition::from_dir(&known.rendition)?,
                    known.recorded()?,
                ))
            })
            .collect();
        self.db
            .write(&(rows, now as i64), |tx, (rows, now)| {
                Box::pin(async move {
                    for (tenant_id, image_id, rendition, file) in rows {
                        want(tx, tenant_id, image_id, *rendition, file, *now).await?;
                    }
                    Ok(())
                })
            })
            .await?;
        self.wake();
        Ok(())
    }

    /// Fetches what is due every [`CHECK_EVERY`], and as soon as a
    /// subscription wrote a row.
    pub fn start(self: Arc<Self>) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(CHECK_EVERY);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = tick.tick() => {}
                    _ = self.wake.notified() => {}
                }
                if let Err(err) = self.beat(crate::auth::now()).await {
                    tracing::error!(error = %format!("{err:#}"), "wanted pictures not fetched");
                }
            }
        })
    }

    /// Fetches the files due at `now` (unix seconds), several at once, and
    /// records what each try came to; how many files were fetched. A store
    /// that cannot be read ends the beat, logged by the caller.
    pub async fn beat(self: &Arc<Self>, now: u64) -> Result<usize> {
        let due: Vec<Wanted> = self
            .db
            .fetch_all(
                "SELECT tenant_id, image_id, rendition, size, sha256, due_at, attempts, given_up,
                        reason, updated_at
                 FROM media_wanted WHERE given_up = 0 AND due_at <= ?1
                 ORDER BY due_at, tenant_id, image_id, rendition LIMIT ?2",
                (now as i64, FILES_PER_BEAT),
            )
            .await?;
        let mut lanes = tokio::task::JoinSet::new();
        for wanted in due {
            let fetcher = self.clone();
            lanes.spawn(async move { fetcher.take(wanted, now).await });
        }
        let mut fetched = 0;
        while let Some(joined) = lanes.join_next().await {
            fetched += usize::from(joined.unwrap_or(false));
        }
        self.db
            .execute(
                "DELETE FROM media_wanted WHERE given_up = 1 AND updated_at <= ?1",
                (now.saturating_sub(GIVEN_UP_KEPT_SECS) as i64,),
            )
            .await?;
        Ok(fetched)
    }

    /// One file: asks the peers and records the outcome on its row; whether
    /// the file was fetched.
    async fn take(&self, wanted: Wanted, now: u64) -> bool {
        let Some((rendition, recorded)) = wanted.recorded() else {
            tracing::warn!(tenant = %wanted.tenant_id, image = %wanted.image_id, rendition = %wanted.rendition, "a wanted picture's row is not one of ours: dropped");
            self.done(&wanted).await;
            return false;
        };
        let outcome = self
            .media
            .fetch_rendition(&wanted.tenant_id, &wanted.image_id, rendition, &recorded)
            .await;
        match outcome {
            Ok(Fetch::Held) => {
                self.done(&wanted).await;
                false
            }
            Ok(Fetch::Fetched) => {
                self.done(&wanted).await;
                true
            }
            Ok(Fetch::Gone) => {
                tracing::warn!(
                    tenant = %wanted.tenant_id,
                    image = %wanted.image_id,
                    rendition = %wanted.rendition,
                    "no node holds this picture any more: not fetched"
                );
                self.done(&wanted).await;
                false
            }
            Ok(Fetch::Unreachable(reason)) => {
                self.retry_later(&wanted, &reason, now).await;
                false
            }
            Err(err) => {
                tracing::error!(
                    tenant = %wanted.tenant_id,
                    image = %wanted.image_id,
                    rendition = %wanted.rendition,
                    error = %format!("{err:#}"),
                    "a wanted picture could not be stored"
                );
                self.put_off(
                    &wanted,
                    &format!("{err:#}"),
                    now + OWN_FAULT_RETRY_SECS,
                    now,
                )
                .await;
                false
            }
        }
    }

    /// The row goes: the file is here, or nowhere.
    async fn done(&self, wanted: &Wanted) {
        if let Err(err) = self
            .db
            .execute(
                "DELETE FROM media_wanted WHERE tenant_id = ?1 AND image_id = ?2 AND rendition = ?3",
                (
                    wanted.tenant_id.as_str(),
                    wanted.image_id.as_str(),
                    wanted.rendition.as_str(),
                ),
            )
            .await
        {
            tracing::warn!(error = %format!("{err:#}"), "a wanted picture's row could not be dropped");
        }
    }

    /// The next try, after the next of the growing waits; given up on once
    /// they run out.
    async fn retry_later(&self, wanted: &Wanted, reason: &str, now: u64) {
        let attempts = usize::try_from(wanted.attempts).unwrap_or(usize::MAX);
        match RETRY_DELAYS_SECS.get(attempts) {
            Some(delay) => self.put_off(wanted, reason, now + delay, now).await,
            None => {
                tracing::error!(
                    tenant = %wanted.tenant_id,
                    image = %wanted.image_id,
                    rendition = %wanted.rendition,
                    %reason,
                    "a wanted picture could not be fetched after every retry: given up"
                );
                if let Err(err) = self
                    .db
                    .execute(
                        "UPDATE media_wanted SET given_up = 1, attempts = attempts + 1, reason = ?4,
                            updated_at = ?5
                         WHERE tenant_id = ?1 AND image_id = ?2 AND rendition = ?3",
                        (
                            wanted.tenant_id.as_str(),
                            wanted.image_id.as_str(),
                            wanted.rendition.as_str(),
                            reason,
                            now as i64,
                        ),
                    )
                    .await
                {
                    tracing::warn!(error = %format!("{err:#}"), "a wanted picture could not be given up on");
                }
            }
        }
    }

    async fn put_off(&self, wanted: &Wanted, reason: &str, until: u64, now: u64) {
        if let Err(err) = self
            .db
            .execute(
                "UPDATE media_wanted SET due_at = ?4, attempts = attempts + 1, reason = ?5,
                    updated_at = ?6
                 WHERE tenant_id = ?1 AND image_id = ?2 AND rendition = ?3 AND given_up = 0",
                (
                    wanted.tenant_id.as_str(),
                    wanted.image_id.as_str(),
                    wanted.rendition.as_str(),
                    until as i64,
                    reason,
                    now as i64,
                ),
            )
            .await
        {
            tracing::warn!(error = %format!("{err:#}"), "a wanted picture could not be put off");
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::super::test_support::{png, rendered};
    use super::*;
    use crate::peers::test_support::{answering, down, peer};
    use timada_core::Picture;

    /// A store with the read models' tables, and a fetcher on a fresh media
    /// directory with these peers.
    pub(crate) async fn fetcher_with(
        peers: Vec<String>,
        nodes: usize,
    ) -> (evento::Turso, Arc<Fetcher>) {
        let store = evento::Turso::in_memory_concurrent().await.unwrap();
        crate::db::migrate_store_read_models(&store).await.unwrap();
        let media = Arc::new(super::super::tests::media(peers, nodes));
        let fetcher = Fetcher::new(media, Db::from(store.clone()));
        (store, fetcher)
    }

    /// A single node: nothing to fetch from.
    pub(crate) async fn fetcher_alone() -> (evento::Turso, Arc<Fetcher>) {
        let store = evento::Turso::in_memory_concurrent().await.unwrap();
        crate::db::migrate_store_read_models(&store).await.unwrap();
        let media = Arc::new(super::super::tests::alone(super::super::tests::media(
            vec![],
            1,
        )));
        let fetcher = Fetcher::new(media, Db::from(store.clone()));
        (store, fetcher)
    }

    pub(crate) async fn wanted(db: &Db) -> Vec<Wanted> {
        db.fetch_all(
            "SELECT tenant_id, image_id, rendition, size, sha256, due_at, attempts, given_up,
                    reason, updated_at
             FROM media_wanted ORDER BY image_id, rendition",
            (),
        )
        .await
        .unwrap()
    }

    async fn want_picture(fetcher: &Fetcher, image_id: &str, picture: &Picture, now: u64) {
        fetcher
            .db
            .write(
                &(image_id, picture, now as i64),
                |tx, (image_id, picture, now)| {
                    Box::pin(async move {
                        for (rendition, file) in picture.files() {
                            want(tx, "acme", image_id, rendition, file, *now).await?;
                        }
                        Ok(())
                    })
                },
            )
            .await
            .unwrap();
    }

    /// A peer that serves the rendition the request names.
    async fn serving(upload: &super::super::Rendered) -> String {
        let (listing, page) = (upload.listing.clone(), upload.page.clone());
        answering(move |head, _| {
            let body = if head.contains("/400/") {
                listing.to_vec()
            } else {
                page.to_vec()
            };
            (200, body)
        })
        .await
    }

    /// A file a peer has is fetched and its row goes; a file every peer
    /// lacks is dropped as gone; a file a peer might have but could not be
    /// asked for waits for the next try.
    #[tokio::test]
    async fn a_beat_fetches_drops_or_puts_off() {
        let upload = rendered(&png(900, 600, [10, 10, 200, 255]));
        let has_it = serving(&upload).await;
        let (_store, fetcher) = fetcher_with(vec![peer(404, b"").await, has_it], 3).await;
        want_picture(&fetcher, "i1", &upload.picture, 1_000).await;
        // A picture nobody has: the same bytes claimed under another digest.
        let mut lost = upload.picture;
        lost.listing.sha256 = [0xee; 32];
        lost.page.sha256 = [0xee; 32];
        want_picture(&fetcher, "lost", &lost, 1_000).await;
        assert_eq!(wanted(&fetcher.db).await.len(), 4);

        assert_eq!(fetcher.beat(1_000).await.unwrap(), 2);
        assert!(
            wanted(&fetcher.db).await.is_empty(),
            "fetched or gone: no row stays"
        );
        for (rendition, file) in upload.picture.files() {
            let path = fetcher.media.path_of("acme", rendition, "i1").unwrap();
            assert_eq!(std::fs::metadata(&path).unwrap().len(), file.size);
        }
        assert!(
            !fetcher
                .media
                .path_of("acme", Rendition::Page, "lost")
                .unwrap()
                .exists()
        );

        // A peer down: the rows wait, longer each time, then are given up on.
        let (_store, fetcher) = fetcher_with(vec![down().await, peer(404, b"").await], 3).await;
        want_picture(&fetcher, "i2", &upload.picture, 1_000).await;
        let mut now = 1_000;
        for (attempt, delay) in RETRY_DELAYS_SECS.iter().enumerate() {
            assert_eq!(fetcher.beat(now).await.unwrap(), 0);
            let rows = wanted(&fetcher.db).await;
            assert_eq!(rows.len(), 2);
            assert!(
                rows.iter()
                    .all(|row| !row.given_up && row.attempts == attempt as i64 + 1)
            );
            assert!(
                rows.iter().all(|row| row.due_at == (now + delay) as i64),
                "{rows:?}"
            );
            assert!(rows[0].reason.contains("http://"), "{}", rows[0].reason);
            // Not due yet: left alone.
            assert_eq!(fetcher.beat(now + 1).await.unwrap(), 0);
            assert_eq!(wanted(&fetcher.db).await[0].attempts, attempt as i64 + 1);
            now += delay;
        }
        assert_eq!(fetcher.beat(now).await.unwrap(), 0);
        let rows = wanted(&fetcher.db).await;
        assert!(rows.iter().all(|row| row.given_up), "{rows:?}");
        assert_eq!(
            summary(&fetcher.db).await.unwrap(),
            Summary {
                wanted: 0,
                given_up: 2
            }
        );
        // Shown for a week, then forgotten.
        fetcher.beat(now + GIVEN_UP_KEPT_SECS + 1).await.unwrap();
        assert!(wanted(&fetcher.db).await.is_empty());

        // Wanted again (the picture recorded afresh), it starts over.
        want_picture(&fetcher, "i2", &upload.picture, now).await;
        assert_eq!(wanted(&fetcher.db).await[0].attempts, 0);
    }

    /// A file this node holds already is simply taken off the list; a
    /// single node, with nobody to ask, drops what it lacks.
    #[tokio::test]
    async fn a_file_held_or_unobtainable_leaves_the_list() {
        let upload = rendered(&png(300, 200, [9, 9, 9, 255]));
        let (_store, fetcher) = fetcher_alone().await;
        fetcher
            .media
            .store("acme", Rendition::Page, "i1", &upload.page)
            .await
            .unwrap();
        want_picture(&fetcher, "i1", &upload.picture, 5).await;
        assert_eq!(fetcher.beat(5).await.unwrap(), 0);
        assert!(wanted(&fetcher.db).await.is_empty());
        assert!(
            fetcher
                .media
                .path_of("acme", Rendition::Page, "i1")
                .unwrap()
                .exists()
        );
    }
}
