use evento::{Executor, Projection, ProjectionAggregate, WriteError, metadata::Event};

use crate::aggregator::{
    Claimed, ClaimedForEdition, Drafted, Edited, Failed, Finished, ScheduleActivated,
    ScheduleCleared, ScheduleSet, ScheduleStopped, SendRequested, Skipped,
};
use crate::error::NewsletterError;
use crate::schedule::{Cadence, Schedule, parse_day};
use crate::value_object::{
    CampaignStatus, MAX_CAMPAIGN_BODY_LEN, MAX_CAMPAIGN_SUBJECT_LEN, delivery_id, new_campaign_id,
};

/// What the merchant writes: the copy and who gets it. The templates are
/// checked against the shop's variables by the caller (the crate does not
/// know them); here only their size.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CampaignDraft {
    pub subject: String,
    pub html: String,
    /// Blank is none: the text is derived from the HTML.
    pub text: Option<String>,
    /// A language tag; blank is everyone.
    pub language: Option<String>,
}

impl CampaignDraft {
    /// The rules [`Command::draft_campaign`] applies, for a form to refuse
    /// before anything else.
    pub fn validate(&self) -> Result<(), NewsletterError> {
        self.check().map(|_| ())
    }

    fn check(&self) -> Result<Self, NewsletterError> {
        let subject = self.subject.trim();
        if subject.is_empty()
            || subject.chars().count() > MAX_CAMPAIGN_SUBJECT_LEN
            || subject.chars().any(char::is_control)
        {
            return Err(NewsletterError::InvalidSubject);
        }
        let html = self.html.trim();
        if html.is_empty() || html.len() > MAX_CAMPAIGN_BODY_LEN {
            return Err(NewsletterError::InvalidBody);
        }
        let text = self
            .text
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty());
        if text.is_some_and(|text| text.len() > MAX_CAMPAIGN_BODY_LEN) {
            return Err(NewsletterError::InvalidBody);
        }
        Ok(Self {
            subject: subject.to_owned(),
            html: html.to_owned(),
            text: text.map(str::to_owned),
            language: self
                .language
                .as_deref()
                .and_then(timada_core::primary_language),
        })
    }
}

/// The write model of a campaign.
#[evento::projection(id = id, name = "timada-newsletter/CampaignState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct CampaignState {
    pub id: String,
    pub subject: String,
    pub html: String,
    pub text: Option<String>,
    pub language: Option<String>,
    pub status: CampaignStatus,
    /// Unix seconds of `Drafted`.
    pub created_at: u64,
    /// Unix seconds of `SendRequested`, once sent.
    pub sent_at: Option<u64>,
    /// Unix seconds of `Finished`.
    pub finished_at: Option<u64>,
    /// Unix seconds of the last event.
    pub updated_at: u64,
    /// When it goes out, once scheduled; none goes out at once.
    pub schedule: Option<Schedule>,
    /// Unix seconds of `ScheduleActivated`.
    pub activated_at: Option<u64>,
}

impl CampaignState {
    /// Whether its copy may still change: a draft, or a scheduled campaign
    /// (whose next editions take the new copy).
    pub fn is_editable(&self) -> bool {
        matches!(
            self.status,
            CampaignStatus::Draft | CampaignStatus::Scheduled
        )
    }
}

pub fn campaign_projection<E: Executor>() -> Projection<E, CampaignState> {
    Projection::new::<crate::aggregator::Campaign>()
        .handler(on_drafted())
        .handler(on_edited())
        .handler(on_send_requested())
        .handler(on_finished())
        .handler(on_schedule_set())
        .handler(on_schedule_cleared())
        .handler(on_schedule_activated())
        .handler(on_schedule_stopped())
        .strict()
}

/// The schedule an event records; a value no longer read is dropped, so
/// the campaign stays loadable.
fn schedule_of(data: &ScheduleSet) -> Option<Schedule> {
    Some(Schedule {
        cadence: Cadence::parse(&data.cadence)?,
        weekdays: data.weekdays.clone(),
        month_day: data.month_day,
        starts_on: parse_day(&data.starts_on)?,
        ends_on: data.ends_on.as_deref().and_then(parse_day),
        window_start: data.window_start,
        window_end: data.window_end,
        pace_minutes: data.pace_minutes,
    })
}

#[evento::handler]
async fn on_schedule_set(
    event: Event<ScheduleSet>,
    state: &mut CampaignState,
) -> anyhow::Result<()> {
    state.schedule = schedule_of(&event.data);
    state.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_schedule_cleared(
    event: Event<ScheduleCleared>,
    state: &mut CampaignState,
) -> anyhow::Result<()> {
    state.schedule = None;
    state.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_schedule_activated(
    event: Event<ScheduleActivated>,
    state: &mut CampaignState,
) -> anyhow::Result<()> {
    state.status = CampaignStatus::Scheduled;
    state.activated_at = Some(event.timestamp);
    state.sent_at = Some(event.timestamp);
    state.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_schedule_stopped(
    event: Event<ScheduleStopped>,
    state: &mut CampaignState,
) -> anyhow::Result<()> {
    state.status = CampaignStatus::Stopped;
    state.finished_at = Some(event.timestamp);
    state.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_drafted(event: Event<Drafted>, state: &mut CampaignState) -> anyhow::Result<()> {
    let data = &event.data;
    state.id = event.aggregate_id.to_owned();
    state.subject = data.subject.to_owned();
    state.html = data.html.to_owned();
    state.text = data.text.to_owned();
    state.language = data.language.to_owned();
    state.status = CampaignStatus::Draft;
    state.created_at = event.timestamp;
    state.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_edited(event: Event<Edited>, state: &mut CampaignState) -> anyhow::Result<()> {
    let data = &event.data;
    state.subject = data.subject.to_owned();
    state.html = data.html.to_owned();
    state.text = data.text.to_owned();
    state.language = data.language.to_owned();
    state.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_send_requested(
    event: Event<SendRequested>,
    state: &mut CampaignState,
) -> anyhow::Result<()> {
    state.status = CampaignStatus::Sending;
    state.sent_at = Some(event.timestamp);
    state.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_finished(event: Event<Finished>, state: &mut CampaignState) -> anyhow::Result<()> {
    state.status = CampaignStatus::Sent;
    state.finished_at = Some(event.timestamp);
    state.updated_at = event.timestamp;
    Ok(())
}

/// One campaign's mail to one subscriber, once claimed.
#[evento::projection(id = id, name = "timada-newsletter/DeliveryState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct DeliveryState {
    pub id: String,
    pub campaign_id: String,
    /// The edition of a scheduled campaign it was claimed for.
    pub edition_id: Option<String>,
    pub subscriber_id: String,
    pub failed: Option<String>,
    /// Why the mail was never sent, when the subscriber had left the list
    /// before it went out.
    pub skipped: Option<String>,
    /// Unix seconds of `Claimed` (or `Skipped`).
    pub claimed_at: u64,
}

pub fn delivery_projection<E: Executor>() -> Projection<E, DeliveryState> {
    Projection::new::<crate::aggregator::Delivery>()
        .handler(on_claimed())
        .handler(on_claimed_for_edition())
        .handler(on_failed())
        .handler(on_skipped())
        .strict()
}

#[evento::handler]
async fn on_skipped(event: Event<Skipped>, state: &mut DeliveryState) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.campaign_id = event.data.campaign_id.to_owned();
    state.edition_id = event.data.edition_id.to_owned();
    state.subscriber_id = event.data.subscriber_id.to_owned();
    state.skipped = Some(event.data.reason.to_owned());
    state.claimed_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_claimed(event: Event<Claimed>, state: &mut DeliveryState) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.campaign_id = event.data.campaign_id.to_owned();
    state.subscriber_id = event.data.subscriber_id.to_owned();
    state.claimed_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_claimed_for_edition(
    event: Event<ClaimedForEdition>,
    state: &mut DeliveryState,
) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.campaign_id = event.data.campaign_id.to_owned();
    state.edition_id = Some(event.data.edition_id.to_owned());
    state.subscriber_id = event.data.subscriber_id.to_owned();
    state.claimed_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_failed(event: Event<Failed>, state: &mut DeliveryState) -> anyhow::Result<()> {
    state.failed = Some(event.data.reason.to_owned());
    Ok(())
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Writes a new draft; its id.
    pub async fn draft_campaign(
        &self,
        actor: &str,
        draft: CampaignDraft,
        routing_key: Option<String>,
    ) -> Result<String, NewsletterError> {
        let draft = draft.check()?;
        let id = new_campaign_id();
        evento::append(&id)
            .original_version(0)
            .routing_key_opt(routing_key)
            .event(&Drafted {
                subject: draft.subject,
                html: draft.html,
                text: draft.text,
                language: draft.language,
                drafted_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(id)
    }

    /// Rewrites a draft (or a scheduled campaign, for its editions to
    /// come), whole; one sent already is [`NewsletterError::CampaignSent`].
    pub async fn edit_campaign(
        &self,
        actor: &str,
        campaign_id: &str,
        draft: CampaignDraft,
        routing_key: Option<String>,
    ) -> Result<(), NewsletterError> {
        let draft = draft.check()?;
        let state = self.require_campaign(campaign_id).await?;
        if !state.is_editable() {
            return Err(NewsletterError::CampaignSent);
        }
        state
            .write()?
            .routing_key_opt(routing_key)
            .event(&Edited {
                subject: draft.subject,
                html: draft.html,
                text: draft.text,
                language: draft.language,
                edited_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }

    /// Sends a draft: from now on it goes out to the confirmed subscribers.
    /// Sent already is [`NewsletterError::CampaignSent`].
    pub async fn send_campaign(
        &self,
        actor: &str,
        campaign_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), NewsletterError> {
        let state = self.require_campaign(campaign_id).await?;
        if state.status != CampaignStatus::Draft {
            return Err(NewsletterError::CampaignSent);
        }
        match state
            .write()?
            .routing_key_opt(routing_key)
            .event(&SendRequested {
                requested_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await
        {
            Ok(_) => Ok(()),
            // Edited or sent by another member at the same moment.
            Err(WriteError::InvalidOriginalVersion) => Err(NewsletterError::CampaignSent),
            Err(err) => Err(err.into()),
        }
    }

    /// Records that every subscriber was mailed, or that a scheduled
    /// campaign's last edition went out; finished already is a no-op.
    pub async fn finish_campaign(
        &self,
        actor: &str,
        campaign_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), NewsletterError> {
        let state = self.require_campaign(campaign_id).await?;
        if !matches!(
            state.status,
            CampaignStatus::Sending | CampaignStatus::Scheduled
        ) {
            return Ok(());
        }
        match state
            .write()?
            .routing_key_opt(routing_key)
            .event(&Finished {
                finished_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await
        {
            // Finished by another node at the same moment.
            Ok(_) | Err(WriteError::InvalidOriginalVersion) => Ok(()),
            Err(err) => Err(err.into()),
        }
    }

    /// Sets when a draft (or a scheduled campaign, for its editions to come)
    /// goes out. `today` is the shop's day: a schedule may not start before
    /// it. A campaign sent or stopped is [`NewsletterError::CampaignSent`].
    pub async fn set_schedule(
        &self,
        actor: &str,
        campaign_id: &str,
        schedule: Schedule,
        today: jiff::civil::Date,
        routing_key: Option<String>,
    ) -> Result<(), NewsletterError> {
        let mut schedule = schedule;
        schedule.weekdays.sort_unstable();
        schedule.weekdays.dedup();
        let state = self.require_campaign(campaign_id).await?;
        if !state.is_editable() {
            return Err(NewsletterError::CampaignSent);
        }
        // A running schedule keeps its start: only its days to come change.
        let today = match (state.status, &state.schedule) {
            (CampaignStatus::Scheduled, Some(current)) => today.min(current.starts_on),
            _ => today,
        };
        schedule.validate(today)?;
        if state.schedule.as_ref() == Some(&schedule) {
            return Ok(());
        }
        state
            .write()?
            .routing_key_opt(routing_key)
            .event(&ScheduleSet {
                cadence: schedule.cadence.as_str().to_owned(),
                weekdays: schedule.weekdays.clone(),
                month_day: schedule.month_day,
                starts_on: schedule.starts_on.to_string(),
                ends_on: schedule.ends_on.map(|day| day.to_string()),
                window_start: schedule.window_start,
                window_end: schedule.window_end,
                pace_minutes: schedule.pace_minutes,
                set_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }

    /// Drops a draft's schedule: sent, it goes out at once. Without one is
    /// a no-op; a campaign no longer a draft is
    /// [`NewsletterError::CampaignSent`].
    pub async fn clear_schedule(
        &self,
        actor: &str,
        campaign_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), NewsletterError> {
        let state = self.require_campaign(campaign_id).await?;
        if state.status != CampaignStatus::Draft {
            return Err(NewsletterError::CampaignSent);
        }
        if state.schedule.is_none() {
            return Ok(());
        }
        state
            .write()?
            .routing_key_opt(routing_key)
            .event(&ScheduleCleared {
                cleared_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }

    /// Starts a draft's schedule: from now on its cycles go out, and the
    /// campaign can only be edited for the editions to come. Without a
    /// schedule is [`NewsletterError::NotScheduled`]; one whose days are
    /// all past now (`today`, the shop's) is
    /// [`NewsletterError::InvalidSchedule`].
    pub async fn activate_schedule(
        &self,
        actor: &str,
        campaign_id: &str,
        today: jiff::civil::Date,
        routing_key: Option<String>,
    ) -> Result<(), NewsletterError> {
        let state = self.require_campaign(campaign_id).await?;
        if state.status != CampaignStatus::Draft {
            return Err(NewsletterError::CampaignSent);
        }
        let schedule = state
            .schedule
            .as_ref()
            .ok_or(NewsletterError::NotScheduled)?;
        schedule.validate(today)?;
        match state
            .write()?
            .routing_key_opt(routing_key)
            .event(&ScheduleActivated {
                activated_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await
        {
            Ok(_) => Ok(()),
            Err(WriteError::InvalidOriginalVersion) => Err(NewsletterError::CampaignSent),
            Err(err) => Err(err.into()),
        }
    }

    /// Stops a scheduled campaign: no edition opens any more and one going
    /// out mails nobody else. Not running is
    /// [`NewsletterError::NotScheduled`].
    pub async fn stop_schedule(
        &self,
        actor: &str,
        campaign_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), NewsletterError> {
        loop {
            let state = self.require_campaign(campaign_id).await?;
            if state.status != CampaignStatus::Scheduled {
                return Err(NewsletterError::NotScheduled);
            }
            match state
                .write()?
                .routing_key_opt(routing_key.clone())
                .event(&ScheduleStopped {
                    stopped_by: actor.to_owned(),
                })
                .tenant(&self.tenant)
                .commit(&self.executor)
                .await
            {
                Ok(_) => return Ok(()),
                // Edited meanwhile: decide again on the fresh state.
                Err(WriteError::InvalidOriginalVersion) => continue,
                Err(err) => return Err(err.into()),
            }
        }
    }

    /// Takes the mailing of a campaign to a subscriber, before it goes out.
    /// Returns whether this caller took it: `false` when it was taken
    /// already, by another node say.
    pub async fn claim_delivery(
        &self,
        actor: &str,
        campaign_id: &str,
        subscriber_id: &str,
        routing_key: Option<String>,
    ) -> Result<bool, NewsletterError> {
        let claimed = evento::append(delivery_id(campaign_id, subscriber_id))
            .original_version(0)
            .routing_key_opt(routing_key)
            .event(&Claimed {
                campaign_id: campaign_id.to_owned(),
                subscriber_id: subscriber_id.to_owned(),
                claimed_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await;
        match claimed {
            Ok(_) => Ok(true),
            Err(WriteError::InvalidOriginalVersion) => Ok(false),
            Err(err) => Err(err.into()),
        }
    }

    /// Takes the mailing of an edition to a subscriber, before it goes out.
    /// Returns whether this caller took it.
    pub async fn claim_edition_delivery(
        &self,
        actor: &str,
        campaign_id: &str,
        edition_id: &str,
        subscriber_id: &str,
        routing_key: Option<String>,
    ) -> Result<bool, NewsletterError> {
        let claimed = evento::append(delivery_id(edition_id, subscriber_id))
            .original_version(0)
            .routing_key_opt(routing_key)
            .event(&ClaimedForEdition {
                campaign_id: campaign_id.to_owned(),
                edition_id: edition_id.to_owned(),
                subscriber_id: subscriber_id.to_owned(),
                claimed_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await;
        match claimed {
            Ok(_) => Ok(true),
            Err(WriteError::InvalidOriginalVersion) => Ok(false),
            Err(err) => Err(err.into()),
        }
    }

    /// Records that a subscriber the read model owed the mail is not to get
    /// it: they left the list (or were erased) before it went out. Takes the
    /// pair's stream as a claim would, so the campaign stops owing them;
    /// `false` when the stream exists already (claimed, or skipped, by
    /// another node). `edition_id` names the edition of a scheduled
    /// campaign.
    pub async fn skip_delivery(
        &self,
        actor: &str,
        campaign_id: &str,
        edition_id: Option<&str>,
        subscriber_id: &str,
        reason: &str,
        routing_key: Option<String>,
    ) -> Result<bool, NewsletterError> {
        let skipped = evento::append(delivery_id(
            edition_id.unwrap_or(campaign_id),
            subscriber_id,
        ))
        .original_version(0)
        .routing_key_opt(routing_key)
        .event(&Skipped {
            campaign_id: campaign_id.to_owned(),
            edition_id: edition_id.map(str::to_owned),
            subscriber_id: subscriber_id.to_owned(),
            reason: reason.chars().take(200).collect(),
            skipped_by: actor.to_owned(),
        })
        .tenant(&self.tenant)
        .commit(&self.executor)
        .await;
        match skipped {
            Ok(_) => Ok(true),
            Err(WriteError::InvalidOriginalVersion) => Ok(false),
            Err(err) => Err(err.into()),
        }
    }

    /// Records that a claimed mail could not be handed over. `campaign_id`
    /// is what the mail was claimed for: the campaign, or the edition of a
    /// scheduled one.
    pub async fn fail_delivery(
        &self,
        campaign_id: &str,
        subscriber_id: &str,
        reason: &str,
        routing_key: Option<String>,
    ) -> Result<(), NewsletterError> {
        let Some(state) = delivery_projection()
            .load(delivery_id(campaign_id, subscriber_id))
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await?
        else {
            return Err(NewsletterError::NotFound);
        };
        if state.failed.is_some() {
            return Ok(());
        }
        // A reason is the mail server's words: kept short.
        let reason: String = reason.chars().take(500).collect();
        state
            .write()?
            .routing_key_opt(routing_key)
            .event(&Failed { reason })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }
}

impl<E: Executor> super::Command<E> {
    pub async fn load_campaign(&self, campaign_id: &str) -> anyhow::Result<Option<CampaignState>> {
        campaign_projection()
            .load(campaign_id)
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await
    }

    async fn require_campaign(&self, campaign_id: &str) -> Result<CampaignState, NewsletterError> {
        self.load_campaign(campaign_id)
            .await?
            .ok_or(NewsletterError::CampaignNotFound)
    }
}
