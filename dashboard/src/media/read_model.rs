//! What the store knows of the pictures' files, and what this node has yet
//! to fetch.
//!
//! One subscription follows the catalog's, the pages' and the reviews'
//! streams in every tenant ([`subscription`]) and another the tenants' in
//! the platform's ([`logo_subscription`], for the mail logos). For each
//! picture recorded they write one row per rendition into `media_files`,
//! the read model every node shares (what the file is, what it weighs, its
//! digest, and what references it), and one into `media_wanted`, this
//! node's own list of files to fetch, which the [`Fetcher`] works off the
//! subscription's cursor. For each picture taken off they delete the rows
//! and this node's files. Nothing here asks a peer or fails for one: a
//! picture nobody holds any more costs a log line in the fetcher, never the
//! cursor of every tenant's pictures (#468).
//!
//! The rows are keyed by what references the picture (`source`,
//! `source_id`), so a deleted product, a rejected review or a deleted page
//! takes its pictures with it without reading its stream again, and a
//! node replaying an old stream inserts and then deletes the rows of a
//! picture long gone rather than looking for its file.

use std::sync::Arc;

use anyhow::Result;
use evento::subscription::{Context, SubscriptionBuilder};
use evento::{Executor, metadata::Event};
use timada_catalog::aggregator::{Deleted as ProductDeleted, ImageAdded, ImageRemoved};
use timada_core::db::{Connection, Db, FromRow, execute, fetch_all};
use timada_core::{Picture, Rendition, RenditionFile};
use timada_pages::aggregator::{
    Deleted as PageDeleted, ImageAdded as PageImageAdded, ImageRemoved as PageImageRemoved,
};
use timada_reviews::aggregator::{
    PhotoRemoved as ReviewPhotoRemoved, Rejected as ReviewRejected, Revised as ReviewRevised,
    Submitted as ReviewSubmitted,
};
use timada_tenancy::aggregator::{MailBrandingCleared, MailBrandingSet};

use super::Fetcher;

pub use super::schema::{LOGO_SUBSCRIPTION, SUBSCRIPTION};

/// What references a picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Product,
    Page,
    Review,
    Logo,
}

impl Source {
    pub fn key(self) -> &'static str {
        match self {
            Self::Product => "product",
            Self::Page => "page",
            Self::Review => "review",
            Self::Logo => "logo",
        }
    }
}

/// Follows every tenant's products, pages and reviews: records each picture
/// added, with what this node has to fetch, and forgets each one removed,
/// deleting this node's files. A handler fails only for the store or the
/// disk, never for a peer.
pub fn subscription<E: Executor>(fetcher: Arc<Fetcher>) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(SUBSCRIPTION)
        .data(fetcher)
        .handler(on_image_added())
        .handler(on_image_removed())
        .handler(on_product_deleted())
        .handler(on_review_submitted())
        .handler(on_review_rejected())
        .handler(on_review_photo_removed())
        .handler(on_review_revised())
        .handler(on_page_image_added())
        .handler(on_page_image_removed())
        .handler(on_page_deleted())
        .any_routing_key()
        .retry(5)
}

/// Follows the tenants in the platform store (the default tenant, where they
/// live: a shop's store key can write the same event in the shop's own
/// tenant, under any `aggregate_id`, and such an event is never read) and
/// records each mail logo set, forgetting the one it replaces.
pub fn logo_subscription<E: Executor>(fetcher: Arc<Fetcher>) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(LOGO_SUBSCRIPTION)
        .data(fetcher)
        .handler(on_mail_branding_set())
        .handler(on_mail_branding_cleared())
        .any_routing_key()
        .tenant("")
        .retry(5)
}

/// One rendition file the store knows.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct Known {
    pub tenant_id: String,
    pub image_id: String,
    /// The rendition's directory name.
    pub rendition: String,
    pub size: i64,
    /// Lowercase hex.
    pub sha256: String,
    pub source: String,
    pub source_id: String,
    pub added_at: i64,
}

impl Known {
    /// What a copy is checked against.
    pub fn recorded(&self) -> Option<RenditionFile> {
        let rendition = Rendition::from_dir(&self.rendition)?;
        Some(RenditionFile {
            // The read model does not keep the dimensions; a check needs
            // none.
            width: rendition.max_side(),
            height: rendition.max_side(),
            size: u64::try_from(self.size).ok()?,
            sha256: parse_sha256(&self.sha256)?,
        })
    }
}

fn parse_sha256(hex: &str) -> Option<[u8; 32]> {
    if hex.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(out)
}

/// What the store recorded of one rendition of a picture, for a copy to be
/// checked against; `None` for a file the store does not know.
pub async fn recorded(
    db: &Db,
    tenant_id: &str,
    image_id: &str,
    rendition: Rendition,
) -> Result<Option<RenditionFile>> {
    let known: Option<Known> = db
        .fetch_optional(
            "SELECT tenant_id, image_id, rendition, size, sha256, source, source_id, added_at
             FROM media_files WHERE tenant_id = ?1 AND image_id = ?2 AND rendition = ?3",
            (tenant_id, image_id, rendition.dir()),
        )
        .await?;
    Ok(known.and_then(|known| known.recorded()))
}

/// The pictures referenced by `source_id` in `tenant_id`, by id.
async fn pictures_of(
    tx: &Connection,
    tenant_id: &str,
    source: Source,
    source_id: &str,
) -> Result<Vec<String>> {
    let rows: Vec<(String,)> = fetch_all(
        tx,
        "SELECT DISTINCT image_id FROM media_files
         WHERE tenant_id = ?1 AND source = ?2 AND source_id = ?3",
        (tenant_id, source.key(), source_id),
    )
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

impl Fetcher {
    /// Records a picture: its files in the read model and on this node's
    /// wanted list, in one transaction, then wakes the fetcher. Safe to
    /// repeat: a replayed event writes the same rows.
    pub(crate) async fn record(
        &self,
        tenant_id: &str,
        image_id: &str,
        picture: &Picture,
        source: Source,
        source_id: &str,
        now: u64,
    ) -> Result<()> {
        let env = (tenant_id, image_id, picture, source, source_id, now as i64);
        self.db
            .write(
                &env,
                |tx, (tenant_id, image_id, picture, source, source_id, now)| {
                    Box::pin(async move {
                        for (rendition, file) in picture.files() {
                            execute(
                            tx,
                            "INSERT INTO media_files (tenant_id, image_id, rendition, size, sha256,
                                source, source_id, added_at)
                             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                             ON CONFLICT (tenant_id, image_id, rendition) DO UPDATE SET
                                size = excluded.size, sha256 = excluded.sha256,
                                source = excluded.source, source_id = excluded.source_id",
                            (
                                *tenant_id,
                                *image_id,
                                rendition.dir(),
                                file.size as i64,
                                file.sha256_hex(),
                                source.key(),
                                *source_id,
                                *now,
                            ),
                        )
                        .await?;
                            super::fetcher::want(tx, tenant_id, image_id, rendition, file, *now)
                                .await?;
                        }
                        Ok(())
                    })
                },
            )
            .await?;
        self.wake();
        Ok(())
    }

    /// Forgets a picture: its rows in both tables, then this node's files.
    pub(crate) async fn forget(&self, tenant_id: &str, image_id: &str) -> Result<()> {
        self.db
            .write(&(tenant_id, image_id), |tx, (tenant_id, image_id)| {
                Box::pin(async move {
                    execute(
                        tx,
                        "DELETE FROM media_files WHERE tenant_id = ?1 AND image_id = ?2",
                        (*tenant_id, *image_id),
                    )
                    .await?;
                    execute(
                        tx,
                        "DELETE FROM media_wanted WHERE tenant_id = ?1 AND image_id = ?2",
                        (*tenant_id, *image_id),
                    )
                    .await?;
                    Ok(())
                })
            })
            .await?;
        self.media.remove_image(tenant_id, image_id).await;
        Ok(())
    }

    /// Forgets every picture `source_id` references.
    pub(crate) async fn forget_all_of(
        &self,
        tenant_id: &str,
        source: Source,
        source_id: &str,
    ) -> Result<()> {
        let ids = self
            .db
            .read(
                &(tenant_id, source, source_id),
                |tx, (tenant_id, source, source_id)| {
                    Box::pin(pictures_of(tx, tenant_id, *source, source_id))
                },
            )
            .await?;
        for image_id in ids {
            self.forget(tenant_id, &image_id).await?;
        }
        Ok(())
    }
}

#[evento::subscription]
async fn on_image_added<E: Executor>(
    context: &Context<'_, E>,
    event: Event<ImageAdded>,
) -> Result<()> {
    let fetcher: Arc<Fetcher> = context.extract();
    fetcher
        .record(
            &event.tenant,
            &event.data.image_id,
            &event.data.picture,
            Source::Product,
            &event.aggregate_id,
            event.timestamp,
        )
        .await
}

#[evento::subscription]
async fn on_image_removed<E: Executor>(
    context: &Context<'_, E>,
    event: Event<ImageRemoved>,
) -> Result<()> {
    let fetcher: Arc<Fetcher> = context.extract();
    fetcher.forget(&event.tenant, &event.data.image_id).await
}

#[evento::subscription]
async fn on_product_deleted<E: Executor>(
    context: &Context<'_, E>,
    event: Event<ProductDeleted>,
) -> Result<()> {
    let fetcher: Arc<Fetcher> = context.extract();
    fetcher
        .forget_all_of(&event.tenant, Source::Product, &event.aggregate_id)
        .await
}

#[evento::subscription]
async fn on_review_submitted<E: Executor>(
    context: &Context<'_, E>,
    event: Event<ReviewSubmitted>,
) -> Result<()> {
    let fetcher: Arc<Fetcher> = context.extract();
    for photo in &event.data.photos {
        fetcher
            .record(
                &event.tenant,
                &photo.id,
                &photo.picture,
                Source::Review,
                &event.aggregate_id,
                event.timestamp,
            )
            .await?;
    }
    Ok(())
}

#[evento::subscription]
async fn on_review_rejected<E: Executor>(
    context: &Context<'_, E>,
    event: Event<ReviewRejected>,
) -> Result<()> {
    let fetcher: Arc<Fetcher> = context.extract();
    fetcher
        .forget_all_of(&event.tenant, Source::Review, &event.aggregate_id)
        .await
}

#[evento::subscription]
async fn on_review_photo_removed<E: Executor>(
    context: &Context<'_, E>,
    event: Event<ReviewPhotoRemoved>,
) -> Result<()> {
    let fetcher: Arc<Fetcher> = context.extract();
    fetcher.forget(&event.tenant, &event.data.image_id).await
}

#[evento::subscription]
async fn on_review_revised<E: Executor>(
    context: &Context<'_, E>,
    event: Event<ReviewRevised>,
) -> Result<()> {
    let fetcher: Arc<Fetcher> = context.extract();
    for image_id in &event.data.removed {
        fetcher.forget(&event.tenant, image_id).await?;
    }
    for photo in &event.data.added {
        fetcher
            .record(
                &event.tenant,
                &photo.id,
                &photo.picture,
                Source::Review,
                &event.aggregate_id,
                event.timestamp,
            )
            .await?;
    }
    Ok(())
}

#[evento::subscription]
async fn on_page_image_added<E: Executor>(
    context: &Context<'_, E>,
    event: Event<PageImageAdded>,
) -> Result<()> {
    let fetcher: Arc<Fetcher> = context.extract();
    fetcher
        .record(
            &event.tenant,
            &event.data.image_id,
            &event.data.picture,
            Source::Page,
            &event.aggregate_id,
            event.timestamp,
        )
        .await
}

#[evento::subscription]
async fn on_page_image_removed<E: Executor>(
    context: &Context<'_, E>,
    event: Event<PageImageRemoved>,
) -> Result<()> {
    let fetcher: Arc<Fetcher> = context.extract();
    fetcher.forget(&event.tenant, &event.data.image_id).await
}

#[evento::subscription]
async fn on_page_deleted<E: Executor>(
    context: &Context<'_, E>,
    event: Event<PageDeleted>,
) -> Result<()> {
    let fetcher: Arc<Fetcher> = context.extract();
    fetcher
        .forget_all_of(&event.tenant, Source::Page, &event.aggregate_id)
        .await
}

/// A logo is the tenant's (the stream's id): the one set replaces the one
/// before, whose files go.
#[evento::subscription]
async fn on_mail_branding_set<E: Executor>(
    context: &Context<'_, E>,
    event: Event<MailBrandingSet>,
) -> Result<()> {
    let fetcher: Arc<Fetcher> = context.extract();
    let tenant_id = event.aggregate_id.as_str();
    let kept = event.data.logo.as_ref().map(|logo| logo.id.as_str());
    let before = fetcher
        .db
        .read(&tenant_id, |tx, tenant_id| {
            Box::pin(pictures_of(tx, tenant_id, Source::Logo, tenant_id))
        })
        .await?;
    for image_id in before {
        if Some(image_id.as_str()) != kept {
            fetcher.forget(tenant_id, &image_id).await?;
        }
    }
    if let Some(logo) = &event.data.logo {
        fetcher
            .record(
                tenant_id,
                &logo.id,
                &logo.picture,
                Source::Logo,
                tenant_id,
                event.timestamp,
            )
            .await?;
    }
    Ok(())
}

#[evento::subscription]
async fn on_mail_branding_cleared<E: Executor>(
    context: &Context<'_, E>,
    event: Event<MailBrandingCleared>,
) -> Result<()> {
    let fetcher: Arc<Fetcher> = context.extract();
    let tenant_id = event.aggregate_id.as_str();
    fetcher
        .forget_all_of(tenant_id, Source::Logo, tenant_id)
        .await
}

/// A rendition file the store does not know is looked up as none.
#[cfg(test)]
pub(crate) async fn known(db: &Db, tenant_id: &str) -> Result<Vec<Known>> {
    db.fetch_all(
        "SELECT tenant_id, image_id, rendition, size, sha256, source, source_id, added_at
         FROM media_files WHERE tenant_id = ?1 ORDER BY image_id, rendition",
        (tenant_id,),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::super::fetcher::tests::{fetcher_alone, wanted};
    use super::super::test_support::{png, rendered};
    use super::*;
    use timada_tenancy::MailLogo;

    fn picture() -> Picture {
        rendered(&png(900, 600, [1, 2, 3, 255])).picture
    }

    /// A picture added is known with its two files, each wanted on this
    /// node; a picture removed is forgotten, files and all, however it is
    /// removed.
    #[tokio::test]
    async fn pictures_are_recorded_and_forgotten() {
        let (store, fetcher) = fetcher_alone().await;
        let picture = picture();
        let added = |id: &str, alt: &str| ImageAdded {
            image_id: id.to_owned(),
            picture,
            alt: alt.to_owned(),
            added_by: "alice".to_owned(),
        };
        evento::append("mat")
            .original_version(0)
            .event(&added("i1", "front"))
            .event(&added("i2", "side"))
            .tenant("acme")
            .commit(&store)
            .await
            .unwrap();
        subscription::<evento::Turso>(fetcher.clone())
            .ephemeral()
            .run_once(&store)
            .await
            .unwrap();
        let rows = known(&fetcher.db, "acme").await.unwrap();
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[0].image_id, "i1");
        assert_eq!(rows[0].rendition, "1200");
        assert_eq!(rows[0].size as u64, picture.page.size);
        assert_eq!(rows[0].sha256, picture.page.sha256_hex());
        assert_eq!(
            (rows[0].source.as_str(), rows[0].source_id.as_str()),
            ("product", "mat")
        );
        assert_eq!(rows[0].recorded().unwrap().sha256, picture.page.sha256);
        assert_eq!(wanted(&fetcher.db).await.len(), 4);
        assert_eq!(
            recorded(&fetcher.db, "acme", "i2", Rendition::Listing)
                .await
                .unwrap()
                .map(|file| file.size),
            Some(picture.listing.size)
        );
        assert!(
            recorded(&fetcher.db, "acme", "nope", Rendition::Listing)
                .await
                .unwrap()
                .is_none()
        );

        // A file this node holds goes with the picture.
        fetcher
            .media
            .store(
                "acme",
                Rendition::Listing,
                "i1",
                &rendered(&png(900, 600, [1, 2, 3, 255])).listing,
            )
            .await
            .unwrap();
        evento::append("mat")
            .original_version(2)
            .event(&ImageRemoved {
                image_id: "i1".to_owned(),
                removed_by: "alice".to_owned(),
            })
            .tenant("acme")
            .commit(&store)
            .await
            .unwrap();
        subscription::<evento::Turso>(fetcher.clone())
            .ephemeral()
            .run_once(&store)
            .await
            .unwrap();
        let rows = known(&fetcher.db, "acme").await.unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| row.image_id == "i2"));
        assert_eq!(wanted(&fetcher.db).await.len(), 2);
        assert!(
            !fetcher
                .media
                .path_of("acme", Rendition::Listing, "i1")
                .unwrap()
                .exists()
        );

        // A deleted product takes the rest, by what references them: no
        // stream read.
        evento::append("mat")
            .original_version(3)
            .event(&ProductDeleted {
                deleted_by: "alice".to_owned(),
            })
            .tenant("acme")
            .commit(&store)
            .await
            .unwrap();
        subscription::<evento::Turso>(fetcher.clone())
            .ephemeral()
            .run_once(&store)
            .await
            .unwrap();
        assert!(known(&fetcher.db, "acme").await.unwrap().is_empty());
        assert!(wanted(&fetcher.db).await.is_empty());
    }

    /// A logo set replaces the one before; cleared, none is known.
    #[tokio::test]
    async fn a_logo_replaces_the_one_before() {
        let (store, fetcher) = fetcher_alone().await;
        let set = |id: &str| MailBrandingSet {
            logo: Some(MailLogo {
                id: id.to_owned(),
                picture: picture(),
            }),
            accent: None,
            footer: None,
            set_by: "alice".to_owned(),
        };
        evento::append("acme")
            .original_version(0)
            .event(&set("l1"))
            .event(&set("l2"))
            .tenant("")
            .commit(&store)
            .await
            .unwrap();
        logo_subscription::<evento::Turso>(fetcher.clone())
            .ephemeral()
            .run_once(&store)
            .await
            .unwrap();
        let rows = known(&fetcher.db, "acme").await.unwrap();
        assert_eq!(rows.len(), 2);
        assert!(
            rows.iter()
                .all(|row| row.image_id == "l2" && row.source == "logo")
        );

        evento::append("acme")
            .original_version(2)
            .event(&MailBrandingCleared {
                cleared_by: "alice".to_owned(),
            })
            .tenant("")
            .commit(&store)
            .await
            .unwrap();
        logo_subscription::<evento::Turso>(fetcher.clone())
            .ephemeral()
            .run_once(&store)
            .await
            .unwrap();
        assert!(known(&fetcher.db, "acme").await.unwrap().is_empty());
    }

    /// Tenants live in the platform's streams. A shop's store key can write a
    /// `MailBrandingSet` in the shop's own tenant under another tenant's id:
    /// nothing is recorded for it.
    #[tokio::test]
    async fn a_branding_in_a_shops_tenant_records_nothing() {
        let (store, fetcher) = fetcher_alone().await;
        evento::append("victim")
            .original_version(0)
            .event(&MailBrandingSet {
                logo: Some(MailLogo {
                    id: "i1".to_owned(),
                    picture: picture(),
                }),
                accent: None,
                footer: None,
                set_by: "mallory".to_owned(),
            })
            .tenant("zeta")
            .commit(&store)
            .await
            .unwrap();
        logo_subscription::<evento::Turso>(fetcher.clone())
            .ephemeral()
            .run_once(&store)
            .await
            .unwrap();
        assert!(known(&fetcher.db, "victim").await.unwrap().is_empty());
        assert!(wanted(&fetcher.db).await.is_empty());
    }
}
