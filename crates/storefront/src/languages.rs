//! The languages the shop's storefront is read in, as the merchant set
//! them in the back office (`timada-pages`): the main one, which the
//! products, the categories, the pages and the articles are written in, and
//! the others they are translated into. The catalog subscription copies
//! them into `storefront_languages`. The library gives the list; which
//! address shows which language is the host's.

use evento::{Executor, metadata::RawEvent, subscription::Context};
use timada_core::db::Db;
pub use timada_pages::ShopLanguages;
use timada_pages::aggregator::Languages;
use timada_pages::languages_projection;
use topcoat_core::context::Cx;
use topcoat_core::error::{Error, Result};

use crate::context::store;

/// The languages the storefront is read in; `None` while the shop never
/// set them, which leaves it in the host's own language.
pub async fn published_languages(db: &Db) -> anyhow::Result<Option<ShopLanguages>> {
    let row: Option<(String, String)> = db
        .fetch_optional("SELECT main, others_json FROM storefront_languages", ())
        .await?;
    Ok(row.map(|(main, others)| ShopLanguages {
        main,
        others: serde_json::from_str(&others).unwrap_or_default(),
    }))
}

/// The languages the storefront is read in; see [`published_languages`].
pub async fn shop_languages(cx: &Cx) -> Result<Option<ShopLanguages>> {
    published_languages(store(cx).db())
        .await
        .map_err(Error::from_anyhow)
}

/// Copies the shop's languages as they stand.
#[evento::subscription_all]
pub(crate) async fn on_languages_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<Languages>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let Some(state) = languages_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    db.execute(
        "INSERT INTO storefront_languages (id, main, others_json) VALUES (1, ?1, ?2)
         ON CONFLICT (id) DO UPDATE SET main = excluded.main, others_json = excluded.others_json",
        (state.main.as_str(), serde_json::to_string(&state.others)?),
    )
    .await?;
    Ok(())
}
