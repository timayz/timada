use evento::{Executor, ProjectionAggregate, metadata::Event};

use crate::aggregator::PopupSet;
use crate::error::PageError;
use crate::value_object::{
    MAX_POPUP_DELAY_SECS, MAX_POPUP_TEXT_LEN, MAX_POPUP_TITLE_LEN, newsletter_popup_id,
};

/// The pop-up that invites a visitor to the newsletter, as the merchant set
/// it. Not shown until they say so.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NewsletterPopupSettings {
    /// Whether the storefront shows it.
    pub shown: bool,
    /// What it offers, in a few words.
    pub title: String,
    /// A sentence or two under the title; may be empty.
    pub text: String,
    /// Seconds on a page before it opens.
    pub delay_secs: u32,
    /// Whether it also opens when the pointer leaves the page upwards.
    pub on_leave: bool,
}

/// The write model of the shop's newsletter pop-up, and what a storefront
/// copies of it.
#[evento::projection(id = id, name = "timada-pages/NewsletterPopupState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct NewsletterPopupState {
    pub id: String,
    pub settings: NewsletterPopupSettings,
}

pub fn newsletter_popup_projection<E: Executor>() -> evento::Projection<E, NewsletterPopupState> {
    evento::Projection::new::<crate::aggregator::NewsletterPopup>()
        .handler(on_popup_set())
        .strict()
}

#[evento::handler]
async fn on_popup_set(
    event: Event<PopupSet>,
    state: &mut NewsletterPopupState,
) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.settings = NewsletterPopupSettings {
        shown: event.data.shown,
        title: event.data.title.clone(),
        text: event.data.text.clone(),
        delay_secs: event.data.delay_secs,
        on_leave: event.data.on_leave,
    };
    Ok(())
}

impl<E: Executor> super::Command<E> {
    /// The shop's newsletter pop-up; `None` before it is first set.
    pub async fn load_newsletter_popup(&self) -> anyhow::Result<Option<NewsletterPopupState>> {
        newsletter_popup_projection()
            .load(newsletter_popup_id())
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await
    }

    /// The pop-up as it is set: not shown before the first time.
    pub async fn newsletter_popup(&self) -> anyhow::Result<NewsletterPopupSettings> {
        Ok(self
            .load_newsletter_popup()
            .await?
            .map(|state| state.settings)
            .unwrap_or_default())
    }
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Sets the pop-up as `settings` say, whole. The title and the text are
    /// put on one line each; one shown needs a title, and opens within
    /// [`MAX_POPUP_DELAY_SECS`]. Settings that change nothing write
    /// nothing.
    pub async fn set_newsletter_popup(
        &self,
        actor: &str,
        settings: NewsletterPopupSettings,
        routing_key: Option<String>,
    ) -> Result<(), PageError> {
        let one_line = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
        let settings = NewsletterPopupSettings {
            title: one_line(&settings.title),
            text: one_line(&settings.text),
            ..settings
        };
        let title = settings.title.chars().count();
        if title > MAX_POPUP_TITLE_LEN
            || (settings.shown && title == 0)
            || settings.text.chars().count() > MAX_POPUP_TEXT_LEN
            || settings.delay_secs > MAX_POPUP_DELAY_SECS
        {
            return Err(PageError::InvalidPopup);
        }
        let state = self.load_newsletter_popup().await?;
        let was = state
            .as_ref()
            .map(|state| state.settings.clone())
            .unwrap_or_default();
        if was == settings {
            return Ok(());
        }
        let mut write = match &state {
            Some(state) => state.write()?,
            None => {
                let mut write = evento::append(newsletter_popup_id());
                write.original_version(0);
                write
            }
        };
        write
            .routing_key_opt(routing_key)
            .event(&PopupSet {
                shown: settings.shown,
                title: settings.title,
                text: settings.text,
                delay_secs: settings.delay_secs,
                on_leave: settings.on_leave,
                set_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }
}
