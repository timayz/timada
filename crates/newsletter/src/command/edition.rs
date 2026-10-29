use evento::{Executor, Projection, ProjectionAggregate, WriteError, metadata::Event};

use crate::aggregator::{EditionClosed, EditionOpened, EditionRolled};
use crate::error::NewsletterError;
use crate::value_object::edition_id;

use super::CampaignState;

/// The write model of one cycle of a scheduled campaign.
#[evento::projection(id = id, name = "timada-newsletter/EditionState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct EditionState {
    pub id: String,
    pub campaign_id: String,
    /// The first day of the cycle it goes out in now, `YYYY-MM-DD`.
    pub cycle: String,
    /// The first day of the cycle it opened in.
    pub first_cycle: String,
    pub subject: String,
    pub html: String,
    pub text: Option<String>,
    pub language: Option<String>,
    /// Unix seconds it opened, or rolled into its current cycle: no slot of
    /// that cycle before it counts.
    pub opened_at: u64,
    /// Unix seconds of `EditionClosed`.
    pub closed_at: Option<u64>,
    /// Unix seconds of the last event.
    pub updated_at: u64,
}

impl EditionState {
    pub fn is_open(&self) -> bool {
        self.closed_at.is_none()
    }
}

pub fn edition_projection<E: Executor>() -> Projection<E, EditionState> {
    Projection::new::<crate::aggregator::Edition>()
        .handler(on_edition_opened())
        .handler(on_edition_rolled())
        .handler(on_edition_closed())
        .strict()
}

#[evento::handler]
async fn on_edition_opened(
    event: Event<EditionOpened>,
    state: &mut EditionState,
) -> anyhow::Result<()> {
    let data = &event.data;
    state.id = event.aggregate_id.to_owned();
    state.campaign_id = data.campaign_id.to_owned();
    state.cycle = data.cycle.to_owned();
    state.first_cycle = data.cycle.to_owned();
    state.subject = data.subject.to_owned();
    state.html = data.html.to_owned();
    state.text = data.text.to_owned();
    state.language = data.language.to_owned();
    state.opened_at = event.timestamp;
    state.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_edition_rolled(
    event: Event<EditionRolled>,
    state: &mut EditionState,
) -> anyhow::Result<()> {
    state.cycle = event.data.cycle.to_owned();
    state.opened_at = event.timestamp;
    state.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_edition_closed(
    event: Event<EditionClosed>,
    state: &mut EditionState,
) -> anyhow::Result<()> {
    state.closed_at = Some(event.timestamp);
    state.updated_at = event.timestamp;
    Ok(())
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Opens the edition of `campaign` for `cycle` (`YYYY-MM-DD`), with the
    /// campaign's copy as it stands. Returns its id; opened already (by
    /// another node, say) is fine.
    pub async fn open_edition(
        &self,
        actor: &str,
        campaign: &CampaignState,
        cycle: &str,
        routing_key: Option<String>,
    ) -> Result<String, NewsletterError> {
        let id = edition_id(&campaign.id, cycle);
        let opened = evento::append(&id)
            .original_version(0)
            .routing_key_opt(routing_key)
            .event(&EditionOpened {
                campaign_id: campaign.id.clone(),
                cycle: cycle.to_owned(),
                subject: campaign.subject.clone(),
                html: campaign.html.clone(),
                text: campaign.text.clone(),
                language: campaign.language.clone(),
                opened_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await;
        match opened {
            Ok(_) | Err(WriteError::InvalidOriginalVersion) => Ok(id),
            Err(err) => Err(err.into()),
        }
    }

    /// Moves an open edition on to `cycle`, for the mails it still owes.
    /// Closed, or there already, is a no-op.
    pub async fn roll_edition(
        &self,
        actor: &str,
        edition_id: &str,
        cycle: &str,
        routing_key: Option<String>,
    ) -> Result<(), NewsletterError> {
        let state = self.require_edition(edition_id).await?;
        if !state.is_open() || state.cycle == cycle {
            return Ok(());
        }
        match state
            .write()?
            .routing_key_opt(routing_key)
            .event(&EditionRolled {
                cycle: cycle.to_owned(),
                rolled_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await
        {
            Ok(_) | Err(WriteError::InvalidOriginalVersion) => Ok(()),
            Err(err) => Err(err.into()),
        }
    }

    /// Closes an edition: nobody else gets it. Closed already is a no-op.
    pub async fn close_edition(
        &self,
        actor: &str,
        edition_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), NewsletterError> {
        let state = self.require_edition(edition_id).await?;
        if !state.is_open() {
            return Ok(());
        }
        match state
            .write()?
            .routing_key_opt(routing_key)
            .event(&EditionClosed {
                closed_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await
        {
            Ok(_) | Err(WriteError::InvalidOriginalVersion) => Ok(()),
            Err(err) => Err(err.into()),
        }
    }
}

impl<E: Executor> super::Command<E> {
    pub async fn load_edition(&self, edition_id: &str) -> anyhow::Result<Option<EditionState>> {
        edition_projection()
            .load(edition_id)
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await
    }

    async fn require_edition(&self, edition_id: &str) -> Result<EditionState, NewsletterError> {
        self.load_edition(edition_id)
            .await?
            .ok_or(NewsletterError::CampaignNotFound)
    }
}
