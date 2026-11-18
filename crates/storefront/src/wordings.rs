//! What the shop's short texts say in its other languages
//! (`timada-pages`' `ShopWording`): the messages of the top bar and the
//! newsletter pop-up. The catalog subscription copies a language's whole
//! into `storefront_wordings`; [`announcements`](crate::announcements) and
//! [`newsletter_popup`](crate::newsletter_popup) read them for a request
//! in that language.

use std::collections::HashMap;

use evento::{Executor, metadata::RawEvent, subscription::Context};
use timada_core::db::{Db, execute};
use timada_pages::aggregator::ShopWording;
use timada_pages::shop_wording_projection;

/// The keys of the pop-up's title and of its text.
pub(crate) const POPUP_TITLE: &str = "popup:title";
pub(crate) const POPUP_TEXT: &str = "popup:text";

/// The key of a message of the bar.
pub(crate) fn announcement_key(announcement_id: &str) -> String {
    format!("announcement:{announcement_id}")
}

/// What the shop worded in `language`, by key: a handful of rows.
pub(crate) async fn wordings_in(
    db: &Db,
    language: &str,
) -> anyhow::Result<HashMap<String, String>> {
    let rows: Vec<(String, String)> = db
        .fetch_all(
            "SELECT key, text FROM storefront_wordings WHERE language = ?1",
            (language,),
        )
        .await?;
    Ok(rows.into_iter().collect())
}

/// Copies a language's short texts, whole.
#[evento::subscription_all]
pub(crate) async fn on_shop_wording_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<ShopWording>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let Some(state) = shop_wording_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    let mut rows: Vec<(String, String)> = state
        .announcements
        .iter()
        .map(|(id, text)| (announcement_key(id), text.clone()))
        .collect();
    rows.push((POPUP_TITLE.to_owned(), state.popup_title.clone()));
    rows.push((POPUP_TEXT.to_owned(), state.popup_text.clone()));
    rows.retain(|(_, text)| !text.is_empty());
    db.write(&(state.language, rows), |tx, (language, rows)| {
        Box::pin(async move {
            execute(
                tx,
                "DELETE FROM storefront_wordings WHERE language = ?1",
                (language.as_str(),),
            )
            .await?;
            for (key, text) in rows {
                execute(
                    tx,
                    "INSERT INTO storefront_wordings (language, key, text) VALUES (?1, ?2, ?3)",
                    (language.as_str(), key.as_str(), text.as_str()),
                )
                .await?;
            }
            Ok(())
        })
    })
    .await
}
