//! The shop's announcements: the short messages ("-10% maintenant avec le
//! code BIENVENUE10") the merchant writes in the back office
//! (`timada-pages`) for the top bar. The catalog subscription copies the
//! shown ones into `storefront_announcements`, in the bar's order, and each
//! request asks [`announcements`] for those whose window is open now, so a
//! message starts and stops at its time with no event to wait for.

use evento::{Executor, metadata::RawEvent, subscription::Context};
use timada_core::db::{Db, FromRow, execute};
use timada_pages::aggregator::Announcements;
use timada_pages::{announcements_projection, render_announcement};
use topcoat_core::context::Cx;
use topcoat_core::error::{Error, Result};

use crate::context::store;

/// A message on the bar now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveAnnouncement {
    pub id: String,
    /// Safe to write into the page unescaped.
    pub html: String,
}

#[derive(FromRow)]
struct AnnouncementRow {
    id: String,
    text: String,
}

/// The messages whose window is open at `now` (Unix seconds), in the bar's
/// order.
pub async fn live_announcements(db: &Db, now: u64) -> anyhow::Result<Vec<LiveAnnouncement>> {
    let rows: Vec<AnnouncementRow> = db
        .fetch_all(
            "SELECT id, text FROM storefront_announcements
             WHERE (starts_at IS NULL OR starts_at <= ?1) AND (ends_at IS NULL OR ends_at > ?1)
             ORDER BY position",
            (now as i64,),
        )
        .await?;
    Ok(rows
        .into_iter()
        .map(|row| LiveAnnouncement {
            html: render_announcement(&row.text),
            id: row.id,
        })
        .collect())
}

/// The messages on the bar now; see [`live_announcements`].
pub async fn announcements(cx: &Cx) -> Result<Vec<LiveAnnouncement>> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    live_announcements(store(cx).db(), now)
        .await
        .map_err(Error::from_anyhow)
}

/// Rewrites the copy whole: a shop has a handful of messages.
#[evento::subscription_all]
pub(crate) async fn on_announcements_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<Announcements>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let shown: Vec<_> = announcements_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
        .map(|state| state.announcements)
        .unwrap_or_default()
        .into_iter()
        .filter(|announcement| announcement.shown)
        .collect();
    db.write(&shown, |tx, shown| {
        Box::pin(async move {
            execute(tx, "DELETE FROM storefront_announcements", ()).await?;
            for (position, announcement) in shown.iter().enumerate() {
                execute(
                    tx,
                    "INSERT INTO storefront_announcements (id, position, text, starts_at, ends_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    (
                        announcement.id.as_str(),
                        position as i64,
                        announcement.text.as_str(),
                        announcement.starts_at.map(|at| at as i64),
                        announcement.ends_at.map(|at| at as i64),
                    ),
                )
                .await?;
            }
            Ok(())
        })
    })
    .await
}
