//! The pop-up that invites a visitor to the shop's newsletter, as the
//! merchant set it in the back office (`timada-pages`): what it offers and
//! when it opens. The catalog subscription copies it into
//! `storefront_newsletter_popup` while the merchant has it shown. The
//! library gives the settings; the dialog, its form and when a visitor has
//! seen it are the host's.

use evento::{Executor, metadata::RawEvent, subscription::Context};
use timada_core::db::{Db, FromRow};
use timada_pages::aggregator::NewsletterPopup as Popup;
use timada_pages::newsletter_popup_projection;
use topcoat_core::context::Cx;
use topcoat_core::error::{Error, Result};

use crate::context::store;

/// The pop-up a shop shows.
#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
pub struct NewsletterPopup {
    /// What it offers, in a few words.
    pub title: String,
    /// A sentence or two under the title; may be empty.
    pub text: String,
    /// Seconds on a page before it opens.
    pub delay_secs: i64,
    /// Whether it also opens when the pointer leaves the page upwards.
    pub on_leave: bool,
}

/// The pop-up, while the merchant has one shown.
pub async fn shown_newsletter_popup(db: &Db) -> anyhow::Result<Option<NewsletterPopup>> {
    db.fetch_optional(
        "SELECT title, text, delay_secs, on_leave FROM storefront_newsletter_popup",
        (),
    )
    .await
}

/// The pop-up, while the merchant has one shown; see
/// [`shown_newsletter_popup`]. A request read in another language
/// ([`read_in`](crate::read_in)) gets its title and its text as the shop
/// worded them there, each in the main language where it did not.
pub async fn newsletter_popup(cx: &Cx) -> Result<Option<NewsletterPopup>> {
    let db = store(cx).db();
    let mut popup = shown_newsletter_popup(db)
        .await
        .map_err(Error::from_anyhow)?;
    if let (Some(language), Some(popup)) = (crate::read_in(cx), &mut popup) {
        let mut worded = crate::wordings::wordings_in(db, language)
            .await
            .map_err(Error::from_anyhow)?;
        if let Some(title) = worded.remove(crate::wordings::POPUP_TITLE) {
            popup.title = title;
        }
        if let Some(text) = worded.remove(crate::wordings::POPUP_TEXT) {
            popup.text = text;
        }
    }
    Ok(popup)
}

/// Copies the pop-up while it is shown, and drops it once it is not.
#[evento::subscription_all]
pub(crate) async fn on_newsletter_popup_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<Popup>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let settings = newsletter_popup_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
        .map(|state| state.settings)
        .unwrap_or_default();
    if !settings.shown {
        db.execute("DELETE FROM storefront_newsletter_popup", ())
            .await?;
        return Ok(());
    }
    db.execute(
        "INSERT INTO storefront_newsletter_popup (id, title, text, delay_secs, on_leave)
         VALUES (1, ?1, ?2, ?3, ?4)
         ON CONFLICT (id) DO UPDATE SET
            title = excluded.title, text = excluded.text, delay_secs = excluded.delay_secs,
            on_leave = excluded.on_leave",
        (
            settings.title.as_str(),
            settings.text.as_str(),
            i64::from(settings.delay_secs),
            settings.on_leave,
        ),
    )
    .await?;
    Ok(())
}
