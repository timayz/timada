use evento::{Executor, ProjectionAggregate, metadata::Event};

use crate::aggregator::{
    AnnouncementAdded, AnnouncementChanged, AnnouncementHidden, AnnouncementMoved,
    AnnouncementRemoved, AnnouncementShown,
};
use crate::error::PageError;
use crate::value_object::{
    MAX_ANNOUNCEMENTS, announcements_id, new_announcement_id, normalize_announcement,
};

/// A message as the merchant writes it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NewAnnouncement {
    pub text: String,
    /// Unix seconds; none shows it at once.
    pub starts_at: Option<u64>,
    /// Unix seconds; none keeps it up until it is hidden.
    pub ends_at: Option<u64>,
}

/// One message of the bar.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Announcement {
    pub id: String,
    pub text: String,
    pub starts_at: Option<u64>,
    pub ends_at: Option<u64>,
    /// Hidden ones stay in the back office, off the bar.
    pub shown: bool,
}

/// Where a message stands at a given time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnouncementStatus {
    /// On the bar now.
    Live,
    /// Shown, its window still to open.
    Scheduled,
    /// Shown, its window closed.
    Ended,
    Hidden,
}

impl Announcement {
    /// Where it stands at `now` (Unix seconds).
    pub fn status(&self, now: u64) -> AnnouncementStatus {
        if !self.shown {
            AnnouncementStatus::Hidden
        } else if self.starts_at.is_some_and(|starts_at| now < starts_at) {
            AnnouncementStatus::Scheduled
        } else if self.ends_at.is_some_and(|ends_at| now >= ends_at) {
            AnnouncementStatus::Ended
        } else {
            AnnouncementStatus::Live
        }
    }
}

/// The write model of the shop's announcements, and what a storefront
/// copies of them.
#[evento::projection(id = id, name = "timada-pages/AnnouncementsState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct AnnouncementsState {
    pub id: String,
    /// In the order the bar shows them.
    pub announcements: Vec<Announcement>,
}

impl AnnouncementsState {
    pub fn announcement(&self, announcement_id: &str) -> Option<&Announcement> {
        self.announcements
            .iter()
            .find(|announcement| announcement.id == announcement_id)
    }

    fn announcement_mut(&mut self, announcement_id: &str) -> Option<&mut Announcement> {
        self.announcements
            .iter_mut()
            .find(|announcement| announcement.id == announcement_id)
    }
}

pub fn announcements_projection<E: Executor>() -> evento::Projection<E, AnnouncementsState> {
    evento::Projection::new::<crate::aggregator::Announcements>()
        .handler(on_announcement_added())
        .handler(on_announcement_changed())
        .handler(on_announcement_hidden())
        .handler(on_announcement_shown())
        .handler(on_announcement_moved())
        .handler(on_announcement_removed())
        .strict()
}

#[evento::handler]
async fn on_announcement_added(
    event: Event<AnnouncementAdded>,
    state: &mut AnnouncementsState,
) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.announcements.push(Announcement {
        id: event.data.announcement_id.clone(),
        text: event.data.text.clone(),
        starts_at: event.data.starts_at,
        ends_at: event.data.ends_at,
        shown: true,
    });
    Ok(())
}

#[evento::handler]
async fn on_announcement_changed(
    event: Event<AnnouncementChanged>,
    state: &mut AnnouncementsState,
) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    if let Some(announcement) = state.announcement_mut(&event.data.announcement_id) {
        announcement.text = event.data.text.clone();
        announcement.starts_at = event.data.starts_at;
        announcement.ends_at = event.data.ends_at;
    }
    Ok(())
}

#[evento::handler]
async fn on_announcement_hidden(
    event: Event<AnnouncementHidden>,
    state: &mut AnnouncementsState,
) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    if let Some(announcement) = state.announcement_mut(&event.data.announcement_id) {
        announcement.shown = false;
    }
    Ok(())
}

#[evento::handler]
async fn on_announcement_shown(
    event: Event<AnnouncementShown>,
    state: &mut AnnouncementsState,
) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    if let Some(announcement) = state.announcement_mut(&event.data.announcement_id) {
        announcement.shown = true;
    }
    Ok(())
}

#[evento::handler]
async fn on_announcement_moved(
    event: Event<AnnouncementMoved>,
    state: &mut AnnouncementsState,
) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    let Some(from) = state
        .announcements
        .iter()
        .position(|announcement| announcement.id == event.data.announcement_id)
    else {
        return Ok(());
    };
    let announcement = state.announcements.remove(from);
    let to = (event.data.position as usize).min(state.announcements.len());
    state.announcements.insert(to, announcement);
    Ok(())
}

#[evento::handler]
async fn on_announcement_removed(
    event: Event<AnnouncementRemoved>,
    state: &mut AnnouncementsState,
) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state
        .announcements
        .retain(|announcement| announcement.id != event.data.announcement_id);
    Ok(())
}

impl<E: Executor> super::Command<E> {
    /// The shop's announcements; `None` before the first is added.
    pub async fn load_announcements(&self) -> anyhow::Result<Option<AnnouncementsState>> {
        announcements_projection()
            .load(announcements_id())
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await
    }

    /// The shop's announcements in the bar's order, shown or hidden.
    pub async fn announcements(&self) -> anyhow::Result<Vec<Announcement>> {
        Ok(self
            .load_announcements()
            .await?
            .map(|state| state.announcements)
            .unwrap_or_default())
    }

    async fn require_announcement(
        &self,
        announcement_id: &str,
    ) -> Result<AnnouncementsState, PageError> {
        self.load_announcements()
            .await?
            .filter(|state| state.announcement(announcement_id).is_some())
            .ok_or(PageError::AnnouncementNotFound)
    }

    async fn write_announcements<D>(
        &self,
        state: &AnnouncementsState,
        event: &D,
        routing_key: Option<String>,
    ) -> Result<(), PageError>
    where
        D: evento::AggregateEvent + bitcode::Encode,
    {
        state
            .write()?
            .routing_key_opt(routing_key)
            .event(event)
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Adds a message, shown, last in the bar; returns its id.
    pub async fn add_announcement(
        &self,
        actor: &str,
        cmd: NewAnnouncement,
        routing_key: Option<String>,
    ) -> Result<String, PageError> {
        let text = normalize_announcement(&cmd.text, cmd.starts_at, cmd.ends_at)?;
        let state = self.load_announcements().await?;
        if state
            .as_ref()
            .is_some_and(|state| state.announcements.len() >= MAX_ANNOUNCEMENTS)
        {
            return Err(PageError::TooManyAnnouncements);
        }
        let mut write = match &state {
            Some(state) => state.write()?,
            None => {
                let mut write = evento::append(announcements_id());
                write.original_version(0);
                write
            }
        };
        let id = new_announcement_id();
        write
            .routing_key_opt(routing_key)
            .event(&AnnouncementAdded {
                announcement_id: id.clone(),
                text,
                starts_at: cmd.starts_at,
                ends_at: cmd.ends_at,
                added_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(id)
    }

    /// Rewrites a message and its window; unchanged is a no-op.
    pub async fn change_announcement(
        &self,
        actor: &str,
        announcement_id: &str,
        cmd: NewAnnouncement,
        routing_key: Option<String>,
    ) -> Result<(), PageError> {
        let text = normalize_announcement(&cmd.text, cmd.starts_at, cmd.ends_at)?;
        let state = self.require_announcement(announcement_id).await?;
        let current = state.announcement(announcement_id).expect("required above");
        if current.text == text
            && current.starts_at == cmd.starts_at
            && current.ends_at == cmd.ends_at
        {
            return Ok(());
        }
        self.write_announcements(
            &state,
            &AnnouncementChanged {
                announcement_id: announcement_id.to_owned(),
                text,
                starts_at: cmd.starts_at,
                ends_at: cmd.ends_at,
                changed_by: actor.to_owned(),
            },
            routing_key,
        )
        .await
    }

    /// Takes a message off the bar, keeping it; hidden already is a no-op.
    pub async fn hide_announcement(
        &self,
        actor: &str,
        announcement_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), PageError> {
        let state = self.require_announcement(announcement_id).await?;
        if !state
            .announcement(announcement_id)
            .expect("required above")
            .shown
        {
            return Ok(());
        }
        self.write_announcements(
            &state,
            &AnnouncementHidden {
                announcement_id: announcement_id.to_owned(),
                hidden_by: actor.to_owned(),
            },
            routing_key,
        )
        .await
    }

    /// Puts a hidden message back on the bar; shown already is a no-op.
    pub async fn show_announcement(
        &self,
        actor: &str,
        announcement_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), PageError> {
        let state = self.require_announcement(announcement_id).await?;
        if state
            .announcement(announcement_id)
            .expect("required above")
            .shown
        {
            return Ok(());
        }
        self.write_announcements(
            &state,
            &AnnouncementShown {
                announcement_id: announcement_id.to_owned(),
                shown_by: actor.to_owned(),
            },
            routing_key,
        )
        .await
    }

    /// Moves a message to `position` in the bar's order (0 is first; past
    /// the end is last); where it is already is a no-op.
    pub async fn move_announcement(
        &self,
        actor: &str,
        announcement_id: &str,
        position: u32,
        routing_key: Option<String>,
    ) -> Result<(), PageError> {
        let state = self.require_announcement(announcement_id).await?;
        let last = state.announcements.len() - 1;
        let position = (position as usize).min(last);
        if state.announcements[position].id == announcement_id {
            return Ok(());
        }
        self.write_announcements(
            &state,
            &AnnouncementMoved {
                announcement_id: announcement_id.to_owned(),
                position: position as u32,
                moved_by: actor.to_owned(),
            },
            routing_key,
        )
        .await
    }

    /// Deletes a message for good.
    pub async fn remove_announcement(
        &self,
        actor: &str,
        announcement_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), PageError> {
        let state = self.require_announcement(announcement_id).await?;
        self.write_announcements(
            &state,
            &AnnouncementRemoved {
                announcement_id: announcement_id.to_owned(),
                removed_by: actor.to_owned(),
            },
            routing_key,
        )
        .await
    }
}
