//! The campaigns timada created in a shop's Google Ads account, and since
//! when each runs.
//!
//! A campaign is asked for through a proposal a member approves; the host
//! then creates it at Google Ads and records here what Google Ads called
//! it, the settings approved and the ad groups made, one stream a shop and
//! a kind ([`campaign_stream_id`]). Nothing here says what the campaign
//! costs or brings: that is read from Google Ads every day, with every
//! other campaign of the account.
//!
//! A campaign created paused is set running by a member, here (the host
//! then tells Google Ads) or at Google Ads itself (the daily read then
//! sees it showing ads). Either way the moment it went live is recorded
//! once: what comes after counts its days from it.

use evento::{Executor, Projection, ProjectionAggregate, WriteError, metadata::Event};

use crate::aggregator::{
    Campaign, CampaignAdGroup, CampaignCreated, CampaignSetting, LiveFailed, LiveRequested,
    WentLive,
};
use crate::error::GoogleError;
use crate::value_object::{
    CAMPAIGN_KINDS, CampaignPlan, MAX_ADS_NAME_LEN, MAX_REASON_LEN, SETTLE_DAYS, campaign_fields,
    campaign_stream_id, normalize_campaign_field, normalize_customer_id, plan_of,
};

use super::Command;

/// How many times a write is decided again when another node wrote the
/// campaign's stream at the same moment.
const ATTEMPTS: usize = 5;

/// The write model of a shop's campaign of one kind.
#[evento::projection(id = id, name = "timada-google/CampaignState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct CampaignState {
    pub id: String,
    /// The shop.
    pub tenant_id: String,
    /// The Google Ads account it was created in.
    pub customer_id: String,
    pub kind: String,
    /// Google Ads' own number for it.
    pub campaign_id: String,
    pub name: String,
    /// What was approved: each field with its value.
    pub settings: Vec<(String, String)>,
    pub ad_groups: Vec<CampaignAdGroup>,
    /// The proposal it was approved through.
    pub proposal_id: String,
    /// Unix seconds.
    pub created_at: u64,
    /// A member asked for it to run and Google Ads was not told yet.
    pub live_requested: bool,
    /// Who asked.
    pub live_requested_by: String,
    /// Unix seconds since when it runs; `None` while it is paused.
    pub live_at: Option<u64>,
    /// Why Google Ads refused to set it running, the last time it was
    /// asked.
    pub live_error: Option<String>,
}

impl CampaignState {
    /// What `field` was approved at; empty for a setting left out.
    pub fn setting(&self, field: &str) -> &str {
        self.settings
            .iter()
            .find(|(named, _)| named == field)
            .map_or("", |(_, value)| value.as_str())
    }

    /// The campaign as it was asked of Google Ads.
    pub fn plan(&self) -> Result<CampaignPlan, GoogleError> {
        plan_of(
            &self.kind,
            self.settings
                .iter()
                .map(|(field, value)| (field.as_str(), value.as_str())),
        )
    }

    /// Unix seconds until when a campaign that runs is left alone; `None`
    /// while it is paused.
    pub fn settles_at(&self) -> Option<u64> {
        self.live_at.map(|at| at + SETTLE_DAYS * 86_400)
    }
}

pub fn campaign_projection<E: Executor>() -> Projection<E, CampaignState> {
    Projection::new::<Campaign>()
        .handler(on_campaign_created())
        .handler(on_live_requested())
        .handler(on_went_live())
        .handler(on_live_failed())
        .strict()
}

#[evento::handler]
async fn on_campaign_created(
    event: Event<CampaignCreated>,
    campaign: &mut CampaignState,
) -> anyhow::Result<()> {
    // Another account's campaign stands in place of the one before, whole.
    *campaign = CampaignState {
        id: event.aggregate_id.to_owned(),
        aggregate_version: campaign.aggregate_version,
        tenant_id: event.data.tenant_id.to_owned(),
        customer_id: event.data.customer_id.to_owned(),
        kind: event.data.kind.to_owned(),
        campaign_id: event.data.campaign_id.to_owned(),
        name: event.data.name.to_owned(),
        settings: event
            .data
            .settings
            .iter()
            .map(|setting| (setting.field.clone(), setting.value.clone()))
            .collect(),
        ad_groups: event.data.ad_groups.to_owned(),
        proposal_id: event.data.proposal_id.to_owned(),
        created_at: event.timestamp,
        live_at: event.data.live.then_some(event.timestamp),
        ..CampaignState::default()
    };
    Ok(())
}

#[evento::handler]
async fn on_live_requested(
    event: Event<LiveRequested>,
    campaign: &mut CampaignState,
) -> anyhow::Result<()> {
    campaign.live_requested = true;
    campaign.live_requested_by = event.data.requested_by.to_owned();
    campaign.live_error = None;
    Ok(())
}

#[evento::handler]
async fn on_went_live(event: Event<WentLive>, campaign: &mut CampaignState) -> anyhow::Result<()> {
    campaign.live_requested = false;
    campaign.live_error = None;
    campaign.live_at = Some(event.data.at);
    Ok(())
}

#[evento::handler]
async fn on_live_failed(
    event: Event<LiveFailed>,
    campaign: &mut CampaignState,
) -> anyhow::Result<()> {
    campaign.live_requested = false;
    campaign.live_error = Some(event.data.reason.to_owned());
    Ok(())
}

/// A campaign Google Ads took, as the host got it back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedCampaign {
    /// One of [`CAMPAIGN_KINDS`].
    pub kind: String,
    /// The Google Ads account it was created in.
    pub customer_id: String,
    /// Google Ads' own number for it.
    pub campaign_id: String,
    /// What was approved: each field with its value.
    pub settings: Vec<(String, String)>,
    pub ad_groups: Vec<CampaignAdGroup>,
    /// The proposal it was approved through.
    pub proposal_id: String,
}

/// Whether `id` is one of Google Ads' own: digits.
fn is_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 20 && id.chars().all(|c| c.is_ascii_digit())
}

impl<E: Executor> Command<E> {
    /// A shop's campaign of `kind` as it stands; `None` before one was
    /// created.
    pub async fn load_campaign(
        &self,
        tenant_id: &str,
        kind: &str,
    ) -> anyhow::Result<Option<CampaignState>> {
        campaign_projection()
            .load(campaign_stream_id(tenant_id, kind))
            .execute(&self.0)
            .await
    }

    /// The campaign whose stream is `id`; `None` for no such stream.
    pub async fn load_campaign_by_id(&self, id: &str) -> anyhow::Result<Option<CampaignState>> {
        campaign_projection().load(id).execute(&self.0).await
    }

    /// Loads the campaign, lets `decide` say what to write, and writes it;
    /// again when another write came in between. `decide` returns `false`
    /// when there is nothing to write. Whether something was written.
    async fn settle_campaign<F>(
        &self,
        tenant_id: &str,
        kind: &str,
        routing_key: Option<String>,
        decide: F,
    ) -> Result<bool, GoogleError>
    where
        F: Fn(Option<&CampaignState>, &mut evento::WriteBuilder) -> Result<bool, GoogleError>,
    {
        if !CAMPAIGN_KINDS.contains(&kind) {
            return Err(GoogleError::InvalidCampaign);
        }
        for _ in 0..ATTEMPTS {
            let loaded = self.load_campaign(tenant_id, kind).await?;
            let mut write = match &loaded {
                Some(campaign) => campaign.write()?,
                None => {
                    let mut write = evento::append(campaign_stream_id(tenant_id, kind));
                    write.original_version(0);
                    write
                }
            };
            if !decide(loaded.as_ref(), &mut write)? {
                return Ok(false);
            }
            write.routing_key_opt(routing_key.clone());
            match write.commit(&self.0).await {
                Ok(_) => return Ok(true),
                Err(WriteError::InvalidOriginalVersion) => continue,
                Err(err) => return Err(err.into()),
            }
        }
        Err(WriteError::InvalidOriginalVersion.into())
    }
}

#[evento::command]
impl<E: Executor> Command<E> {
    /// Records the campaign Google Ads took for the shop. Whether it was
    /// written: the same campaign recorded already (by another node that
    /// saw the same approval) is left as it is.
    pub async fn record_campaign(
        &self,
        actor: &str,
        tenant_id: &str,
        created: CreatedCampaign,
        routing_key: Option<String>,
    ) -> Result<bool, GoogleError> {
        let customer_id = normalize_customer_id(&created.customer_id)?;
        if !is_id(&created.campaign_id)
            || created
                .ad_groups
                .iter()
                .any(|group| !is_id(&group.ad_group_id))
        {
            return Err(GoogleError::InvalidCampaign);
        }
        // Kept as a proposal keeps them, so what was approved reads back
        // the same; a setting no campaign of the kind takes is refused.
        let mut settings = Vec::with_capacity(created.settings.len());
        for field in campaign_fields(&created.kind) {
            let value = created
                .settings
                .iter()
                .find(|(named, _)| named == field)
                .map_or("", |(_, value)| value.as_str());
            let value = normalize_campaign_field(&created.kind, field, value)?;
            settings.push(((*field).to_owned(), value));
        }
        let plan = plan_of(
            &created.kind,
            settings
                .iter()
                .map(|(field, value)| (field.as_str(), value.as_str())),
        )?;
        let event = CampaignCreated {
            tenant_id: tenant_id.to_owned(),
            customer_id,
            kind: created.kind.clone(),
            campaign_id: created.campaign_id.clone(),
            name: plan.name.clone(),
            settings: settings
                .into_iter()
                .map(|(field, value)| CampaignSetting { field, value })
                .collect(),
            ad_groups: created
                .ad_groups
                .iter()
                .map(|group| CampaignAdGroup {
                    ad_group_id: group.ad_group_id.clone(),
                    category_id: group.category_id.clone(),
                    name: group.name.trim().chars().take(MAX_ADS_NAME_LEN).collect(),
                })
                .collect(),
            live: plan.live,
            proposal_id: created.proposal_id.clone(),
            created_by: actor.to_owned(),
        };
        self.settle_campaign(tenant_id, &created.kind, routing_key, |campaign, write| {
            let recorded = campaign.is_some_and(|campaign| {
                campaign.customer_id == event.customer_id
                    && campaign.campaign_id == event.campaign_id
            });
            if recorded {
                return Ok(false);
            }
            write.event(&event);
            Ok(true)
        })
        .await
    }

    /// Asks for the shop's paused campaign of `kind` to run: the host then
    /// tells Google Ads. Asked again while the first is not answered is a
    /// no-op.
    pub async fn request_live(
        &self,
        actor_user_id: &str,
        tenant_id: &str,
        kind: &str,
        routing_key: Option<String>,
    ) -> Result<(), GoogleError> {
        self.settle_campaign(tenant_id, kind, routing_key, |campaign, write| {
            let campaign = campaign.ok_or(GoogleError::NoCampaign)?;
            if campaign.live_at.is_some() {
                return Err(GoogleError::CampaignLive);
            }
            if campaign.live_requested {
                return Ok(false);
            }
            write.event(&LiveRequested {
                requested_by: actor_user_id.to_owned(),
            });
            Ok(true)
        })
        .await?;
        Ok(())
    }

    /// Records that the shop's campaign of `kind`, number `campaign_id` at
    /// Google Ads, runs since `at` (unix seconds). Whether it was written:
    /// one that runs already, or another campaign than the one recorded,
    /// is left alone.
    pub async fn record_live(
        &self,
        actor: &str,
        tenant_id: &str,
        kind: &str,
        campaign_id: &str,
        at: u64,
        routing_key: Option<String>,
    ) -> Result<bool, GoogleError> {
        self.settle_campaign(tenant_id, kind, routing_key, |campaign, write| {
            let Some(campaign) = campaign.filter(|campaign| campaign.campaign_id == campaign_id)
            else {
                return Ok(false);
            };
            if campaign.live_at.is_some() {
                return Ok(false);
            }
            write.event(&WentLive {
                // Never before it existed: a clock that disagrees.
                at: at.max(campaign.created_at),
                noticed_by: actor.to_owned(),
            });
            Ok(true)
        })
        .await
    }

    /// Records that Google Ads refused to set the shop's campaign of
    /// `kind` running, and why. One nobody asks to run any more is left
    /// alone.
    pub async fn record_live_failure(
        &self,
        actor: &str,
        tenant_id: &str,
        kind: &str,
        reason: &str,
        routing_key: Option<String>,
    ) -> Result<(), GoogleError> {
        let reason: String = reason.trim().chars().take(MAX_REASON_LEN).collect();
        self.settle_campaign(tenant_id, kind, routing_key, |campaign, write| {
            if !campaign.is_some_and(|campaign| campaign.live_requested) {
                return Ok(false);
            }
            write.event(&LiveFailed {
                reason: reason.clone(),
                failed_by: actor.to_owned(),
            });
            Ok(true)
        })
        .await?;
        Ok(())
    }
}
