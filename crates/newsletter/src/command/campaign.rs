use evento::{Executor, Projection, ProjectionAggregate, WriteError, metadata::Event};

use crate::aggregator::{Claimed, Drafted, Edited, Failed, Finished, SendRequested};
use crate::error::NewsletterError;
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
}

pub fn campaign_projection<E: Executor>() -> Projection<E, CampaignState> {
    Projection::new::<crate::aggregator::Campaign>()
        .handler(on_drafted())
        .handler(on_edited())
        .handler(on_send_requested())
        .handler(on_finished())
        .strict()
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
    pub subscriber_id: String,
    pub failed: Option<String>,
    /// Unix seconds of `Claimed`.
    pub claimed_at: u64,
}

pub fn delivery_projection<E: Executor>() -> Projection<E, DeliveryState> {
    Projection::new::<crate::aggregator::Delivery>()
        .handler(on_claimed())
        .handler(on_failed())
        .strict()
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

    /// Rewrites a draft, whole; one sent already is
    /// [`NewsletterError::CampaignSent`].
    pub async fn edit_campaign(
        &self,
        actor: &str,
        campaign_id: &str,
        draft: CampaignDraft,
        routing_key: Option<String>,
    ) -> Result<(), NewsletterError> {
        let draft = draft.check()?;
        let state = self.require_campaign(campaign_id).await?;
        if state.status != CampaignStatus::Draft {
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

    /// Records that every subscriber was mailed; finished already is a no-op.
    pub async fn finish_campaign(
        &self,
        actor: &str,
        campaign_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), NewsletterError> {
        let state = self.require_campaign(campaign_id).await?;
        if state.status != CampaignStatus::Sending {
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

    /// Records that a claimed mail could not be handed over.
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
