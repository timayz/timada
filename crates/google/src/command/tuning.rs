//! How a shop's campaign is tuned: the thresholds its rounds decide on,
//! and where the rounds stand.
//!
//! A round is asked for ([`RoundRequested`]): by a member, or by the host
//! when the shop set its rounds to come by themselves and one is due. The
//! host then reads the campaign's last weeks, decides
//! ([`tune`](crate::tune)) and prepares proposals; it records here what the
//! round prepared, and what it looked at and left alone. Nothing is
//! changed at Google Ads until a member approves a proposal.

use evento::{Executor, Projection, ProjectionAggregate, WriteError, metadata::Event};

use crate::aggregator::{
    CampaignSetting, RoundFailed, RoundPrepared, RoundRequested, Tuning, TuningSet,
};
use crate::error::GoogleError;
use crate::value_object::{
    MAX_REASON_LEN, TUNING_FIELDS, TuningSettings, normalize_tuning_field, tuning_id,
    tuning_settings_of,
};

use super::Command;

/// How many times a write is decided again when another node wrote the
/// stream at the same moment.
const ATTEMPTS: usize = 5;

/// How many proposals, notes and flags a round keeps.
pub const MAX_ROUND_LINES: usize = 200;

/// The longest note or flag kept.
const MAX_LINE_LEN: usize = 300;

/// What the last round did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LastRound {
    /// Unix seconds.
    pub at: u64,
    /// The days it read (`2026-11-01` to `2026-11-21`); empty for one that
    /// failed.
    pub first: String,
    pub last: String,
    /// The proposals it prepared, by id.
    pub proposals: Vec<String>,
    pub notes: Vec<String>,
    pub flags: Vec<String>,
    /// Why it could not be prepared.
    pub error: Option<String>,
}

/// The write model of a shop's tuning.
#[evento::projection(id = id, name = "timada-google/TuningState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct TuningState {
    pub id: String,
    /// The shop.
    pub tenant_id: String,
    /// The thresholds set, each field with its value; the defaults for
    /// the ones never set.
    pub settings: Vec<(String, String)>,
    /// A round was asked for and is not prepared yet.
    pub round_requested: bool,
    /// Who asked.
    pub round_requested_by: String,
    /// Unix seconds of the request.
    pub round_requested_at: u64,
    pub last_round: Option<LastRound>,
}

impl TuningState {
    /// The thresholds as a round reads them.
    pub fn thresholds(&self) -> TuningSettings {
        tuning_settings_of(
            self.settings
                .iter()
                .map(|(field, value)| (field.as_str(), value.as_str())),
        )
        .unwrap_or_default()
    }

    /// Whether a round is due at `now` by the shop's own schedule: it
    /// asked for rounds every so many days, none is being prepared, and
    /// the last one is that old.
    pub fn round_due(&self, now: u64) -> bool {
        let every = self.thresholds().every_days;
        every > 0
            && !self.round_requested
            && self
                .last_round
                .as_ref()
                .is_none_or(|last| now >= last.at + every * 86_400)
    }
}

pub fn tuning_projection<E: Executor>() -> Projection<E, TuningState> {
    Projection::new::<Tuning>()
        .handler(on_tuning_set())
        .handler(on_round_requested())
        .handler(on_round_prepared())
        .handler(on_round_failed())
        .strict()
}

#[evento::handler]
async fn on_tuning_set(event: Event<TuningSet>, tuning: &mut TuningState) -> anyhow::Result<()> {
    tuning.id = event.aggregate_id.to_owned();
    tuning.tenant_id = event.data.tenant_id.to_owned();
    tuning.settings = event
        .data
        .settings
        .iter()
        .map(|setting| (setting.field.clone(), setting.value.clone()))
        .collect();
    Ok(())
}

#[evento::handler]
async fn on_round_requested(
    event: Event<RoundRequested>,
    tuning: &mut TuningState,
) -> anyhow::Result<()> {
    tuning.id = event.aggregate_id.to_owned();
    tuning.tenant_id = event.data.tenant_id.to_owned();
    tuning.round_requested = true;
    tuning.round_requested_by = event.data.requested_by.to_owned();
    tuning.round_requested_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_round_prepared(
    event: Event<RoundPrepared>,
    tuning: &mut TuningState,
) -> anyhow::Result<()> {
    tuning.round_requested = false;
    tuning.last_round = Some(LastRound {
        at: event.timestamp,
        first: event.data.first.to_owned(),
        last: event.data.last.to_owned(),
        proposals: event.data.proposals.to_owned(),
        notes: event.data.notes.to_owned(),
        flags: event.data.flags.to_owned(),
        error: None,
    });
    Ok(())
}

#[evento::handler]
async fn on_round_failed(
    event: Event<RoundFailed>,
    tuning: &mut TuningState,
) -> anyhow::Result<()> {
    tuning.round_requested = false;
    tuning.last_round = Some(LastRound {
        at: event.timestamp,
        error: Some(event.data.reason.to_owned()),
        ..LastRound::default()
    });
    Ok(())
}

/// What a round prepared, as the host hands it over.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PreparedRound {
    pub first: String,
    pub last: String,
    pub proposals: Vec<String>,
    pub notes: Vec<String>,
    pub flags: Vec<String>,
}

/// Lines short enough and few enough to keep.
fn lines(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .map(|line| line.trim().chars().take(MAX_LINE_LEN).collect::<String>())
        .filter(|line| !line.is_empty())
        .take(MAX_ROUND_LINES)
        .collect()
}

impl<E: Executor> Command<E> {
    /// How a shop's campaign is tuned; `None` before a threshold was set
    /// or a round asked for: the defaults then.
    pub async fn load_tuning(&self, tenant_id: &str) -> anyhow::Result<Option<TuningState>> {
        tuning_projection()
            .load(tuning_id(tenant_id))
            .execute(&self.0)
            .await
    }

    /// Loads the tuning, lets `decide` say what to write, and writes it;
    /// again when another write came in between. Whether something was
    /// written.
    async fn settle_tuning<F>(
        &self,
        tenant_id: &str,
        routing_key: Option<String>,
        decide: F,
    ) -> Result<bool, GoogleError>
    where
        F: Fn(Option<&TuningState>, &mut evento::WriteBuilder) -> Result<bool, GoogleError>,
    {
        for _ in 0..ATTEMPTS {
            let loaded = self.load_tuning(tenant_id).await?;
            let mut write = match &loaded {
                Some(tuning) => tuning.write()?,
                None => {
                    let mut write = evento::append(tuning_id(tenant_id));
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
    /// Sets the thresholds the shop's rounds decide on: each of
    /// [`TUNING_FIELDS`] with its value, the default for one left out. The
    /// same again is a no-op.
    pub async fn set_tuning(
        &self,
        actor_user_id: &str,
        tenant_id: &str,
        settings: Vec<(String, String)>,
        routing_key: Option<String>,
    ) -> Result<(), GoogleError> {
        let mut kept = Vec::with_capacity(TUNING_FIELDS.len());
        for (field, default) in TUNING_FIELDS {
            let value = settings
                .iter()
                .find(|(named, _)| named == field)
                .map_or(default, |(_, value)| value.as_str());
            kept.push((field.to_owned(), normalize_tuning_field(field, value)?));
        }
        self.settle_tuning(tenant_id, routing_key, |tuning, write| {
            if tuning.is_some_and(|tuning| tuning.settings == kept) {
                return Ok(false);
            }
            write.event(&TuningSet {
                tenant_id: tenant_id.to_owned(),
                settings: kept
                    .iter()
                    .map(|(field, value)| CampaignSetting {
                        field: field.clone(),
                        value: value.clone(),
                    })
                    .collect(),
                set_by: actor_user_id.to_owned(),
            });
            Ok(true)
        })
        .await?;
        Ok(())
    }

    /// Asks for a round. Whether it was written: one asked for and not
    /// prepared yet is not asked for twice.
    pub async fn request_round(
        &self,
        actor: &str,
        tenant_id: &str,
        routing_key: Option<String>,
    ) -> Result<bool, GoogleError> {
        self.settle_tuning(tenant_id, routing_key, |tuning, write| {
            if tuning.is_some_and(|tuning| tuning.round_requested) {
                return Ok(false);
            }
            write.event(&RoundRequested {
                tenant_id: tenant_id.to_owned(),
                requested_by: actor.to_owned(),
            });
            Ok(true)
        })
        .await
    }

    /// Records what the round asked for prepared. Whether it was written:
    /// with no round asked for (another node recorded it), nothing is.
    pub async fn record_round(
        &self,
        actor: &str,
        tenant_id: &str,
        round: PreparedRound,
        routing_key: Option<String>,
    ) -> Result<bool, GoogleError> {
        let event = RoundPrepared {
            tenant_id: tenant_id.to_owned(),
            first: round.first.clone(),
            last: round.last.clone(),
            proposals: round
                .proposals
                .iter()
                .take(MAX_ROUND_LINES)
                .cloned()
                .collect(),
            notes: lines(&round.notes),
            flags: lines(&round.flags),
            prepared_by: actor.to_owned(),
        };
        self.settle_tuning(tenant_id, routing_key, |tuning, write| {
            if !tuning.is_some_and(|tuning| tuning.round_requested) {
                return Ok(false);
            }
            write.event(&event);
            Ok(true)
        })
        .await
    }

    /// Records that the round asked for could not be prepared, and why.
    pub async fn record_round_failure(
        &self,
        actor: &str,
        tenant_id: &str,
        reason: &str,
        routing_key: Option<String>,
    ) -> Result<bool, GoogleError> {
        let reason: String = reason.trim().chars().take(MAX_REASON_LEN).collect();
        self.settle_tuning(tenant_id, routing_key, |tuning, write| {
            if !tuning.is_some_and(|tuning| tuning.round_requested) {
                return Ok(false);
            }
            write.event(&RoundFailed {
                tenant_id: tenant_id.to_owned(),
                reason: reason.clone(),
                failed_by: actor.to_owned(),
            });
            Ok(true)
        })
        .await
    }
}
