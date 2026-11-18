//! What the shop's short texts say in its other languages: the messages of
//! the top bar and the newsletter pop-up.

use std::collections::BTreeMap;

use evento::{Executor, Projection, ProjectionAggregate, metadata::Event};

use crate::aggregator::{AnnouncementWorded, PopupWorded, ShopWording};
use crate::error::PageError;
use crate::value_object::{
    MAX_POPUP_TEXT_LEN, MAX_POPUP_TITLE_LEN, normalize_announcement, shop_wording_id,
};

use super::normalize_language;

/// The write model of a language's short texts, and what a storefront
/// copies of it.
#[evento::projection(id = id, name = "timada-pages/ShopWordingState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct ShopWordingState {
    pub id: String,
    pub language: String,
    /// A message's id, then its text in the language: only the ones
    /// worded there.
    pub announcements: BTreeMap<String, String>,
    /// Empty where the main language's stands.
    pub popup_title: String,
    pub popup_text: String,
}

pub fn shop_wording_projection<E: Executor>() -> Projection<E, ShopWordingState> {
    Projection::new::<ShopWording>()
        .handler(on_announcement_worded())
        .handler(on_popup_worded())
        .strict()
}

#[evento::handler]
async fn on_announcement_worded(
    event: Event<AnnouncementWorded>,
    state: &mut ShopWordingState,
) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.language = event.data.language.to_owned();
    if event.data.text.is_empty() {
        state.announcements.remove(&event.data.announcement_id);
    } else {
        state.announcements.insert(
            event.data.announcement_id.to_owned(),
            event.data.text.to_owned(),
        );
    }
    Ok(())
}

#[evento::handler]
async fn on_popup_worded(
    event: Event<PopupWorded>,
    state: &mut ShopWordingState,
) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.language = event.data.language.to_owned();
    state.popup_title = event.data.title.to_owned();
    state.popup_text = event.data.text.to_owned();
    Ok(())
}

impl<E: Executor> super::Command<E> {
    async fn load_shop_wording(&self, language: &str) -> anyhow::Result<Option<ShopWordingState>> {
        shop_wording_projection()
            .load(shop_wording_id(language))
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await
    }

    /// What the shop's short texts say in `language`; nothing worded while
    /// it never said.
    pub async fn shop_wording(&self, language: &str) -> anyhow::Result<ShopWordingState> {
        Ok(self.load_shop_wording(language).await?.unwrap_or_default())
    }
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Says what a message of the bar reads in `language`, marked as the
    /// main one is; an empty `text` goes back to the main language's, and
    /// the same one is a no-op. A language is one of the list, whether the
    /// storefront is read in it yet or not.
    pub async fn word_announcement(
        &self,
        actor: &str,
        language: &str,
        announcement_id: &str,
        text: &str,
        routing_key: Option<String>,
    ) -> Result<(), PageError> {
        let language = normalize_language(language)?;
        let text = if text.trim().is_empty() {
            String::new()
        } else {
            normalize_announcement(text, None, None)?
        };
        let known = self
            .load_announcements()
            .await?
            .is_some_and(|state| state.announcement(announcement_id).is_some());
        if !known {
            return Err(PageError::AnnouncementNotFound);
        }
        let state = self.load_shop_wording(&language).await?;
        let current = state
            .as_ref()
            .and_then(|state| state.announcements.get(announcement_id))
            .map_or("", String::as_str);
        if current == text {
            return Ok(());
        }
        let mut write = match &state {
            Some(state) => state.write()?,
            None => {
                let mut write = evento::append(shop_wording_id(&language));
                write.original_version(0);
                write
            }
        };
        write
            .routing_key_opt(routing_key)
            .event(&AnnouncementWorded {
                language,
                announcement_id: announcement_id.to_owned(),
                text,
                worded_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }

    /// Says what the newsletter pop-up reads in `language`: its title and
    /// the text under it, each on one line; an empty one goes back to the
    /// main language's. The same wording is a no-op.
    pub async fn word_newsletter_popup(
        &self,
        actor: &str,
        language: &str,
        title: &str,
        text: &str,
        routing_key: Option<String>,
    ) -> Result<(), PageError> {
        let language = normalize_language(language)?;
        let one_line = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
        let (title, text) = (one_line(title), one_line(text));
        if title.chars().count() > MAX_POPUP_TITLE_LEN || text.chars().count() > MAX_POPUP_TEXT_LEN
        {
            return Err(PageError::InvalidPopup);
        }
        let state = self.load_shop_wording(&language).await?;
        let (was_title, was_text) = state
            .as_ref()
            .map_or(("", ""), |state| (&state.popup_title, &state.popup_text));
        if was_title == title && was_text == text {
            return Ok(());
        }
        let mut write = match &state {
            Some(state) => state.write()?,
            None => {
                let mut write = evento::append(shop_wording_id(&language));
                write.original_version(0);
                write
            }
        };
        write
            .routing_key_opt(routing_key)
            .event(&PopupWorded {
                language,
                title,
                text,
                worded_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }
}
