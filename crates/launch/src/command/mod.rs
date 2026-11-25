//! Write side: the command gateway and the strict write models it loads.
//!
//! The write models fold *all* events of their aggregate (`strict()`), so
//! the version they observe, and `write()` sends as `original_version`,
//! never goes stale. Neither is snapshotted: a launch is a few dozen
//! events at most.
//!
//! The launch's stream is written by the merchant and by the host (a
//! checkpoint read on every node that sees the day come), so a write that
//! lost to another is no verdict: every command loads the launch again
//! and decides again ([`WRITE_ATTEMPTS`] times).

mod notice;

use std::ops::Deref;

use evento::{Executor, Projection, ProjectionAggregate, WriteError, metadata::Event};
use timada_core::Sending;

use crate::aggregator::{
    CheckpointReached, DecisionRecorded, Figure, Launch, MailClaimed, MailDeferred, MailFailed,
    MailSent, MailUndeliverable, Notice, Owed, TargetSet, ThresholdsSet,
};
use crate::error::LaunchError;
use crate::value_object::{
    ADVICES, CHECKPOINTS, MAX_NOTE_LEN, Reading, THRESHOLDS, Thresholds, checkpoint_at, judge,
    launch_id, normalize_threshold, thresholds_of,
};

/// The command gateway. Cheap to clone when `E` is.
pub struct Command<E: Executor> {
    executor: E,
    tenant: String,
}

impl<E: Executor> Command<E> {
    /// The gateway of one tenant: every stream it writes or loads is that
    /// tenant's. `tenant` must not be empty (the default tenant holds the
    /// platform's own streams).
    pub fn new(executor: E, tenant: impl Into<String>) -> Self {
        let tenant = tenant.into();
        debug_assert!(!tenant.is_empty(), "a tenant gateway needs a tenant");
        Self { executor, tenant }
    }

    /// The tenant every call acts in.
    pub fn tenant(&self) -> &str {
        &self.tenant
    }
}

impl<E: Executor> Deref for Command<E> {
    type Target = E;

    fn deref(&self) -> &Self::Target {
        &self.executor
    }
}

/// How many times a command decides again when another write to the
/// launch came in between its load and its write.
const WRITE_ATTEMPTS: usize = 5;

/// What the shop is to reach.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Target {
    /// A month's profit once the ads are paid, in hundredths.
    pub monthly_profit_cents: u64,
    pub currency: String,
    /// `2027-06-30`.
    pub by_day: String,
    pub set_by: String,
    /// Unix seconds.
    pub set_at: u64,
}

/// What a merchant decided at a checkpoint.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Decision {
    /// One of [`ADVICES`].
    pub decision: String,
    pub note: String,
    pub decided_by: String,
    /// Unix seconds.
    pub decided_at: u64,
}

/// A checkpoint as it was read on its day.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reached {
    pub checkpoint: u32,
    /// Unix seconds the days were counted from.
    pub started_at: u64,
    /// Unix seconds it was read at.
    pub reached_at: u64,
    /// The numbers: each name with its value.
    pub figures: Vec<(String, String)>,
    /// One of [`ADVICES`].
    pub advice: String,
    pub why: String,
    pub decision: Option<Decision>,
}

/// The write model of a shop's launch.
#[evento::projection(id = id, name = "timada-launch/LaunchState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct LaunchState {
    pub id: String,
    /// Every target set, the oldest first: the last one stands.
    pub targets: Vec<Target>,
    /// The thresholds set, each name with its value; empty for the
    /// defaults.
    pub thresholds: Vec<(String, String)>,
    /// The checkpoints read so far, in the order of their days.
    pub reached: Vec<Reached>,
}

impl LaunchState {
    /// The target that stands; `None` before one was set.
    pub fn target(&self) -> Option<&Target> {
        self.targets.last()
    }

    /// The thresholds as a checkpoint is judged on them.
    pub fn thresholds(&self) -> Thresholds {
        thresholds_of(
            self.thresholds
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str())),
        )
        .unwrap_or_default()
    }

    /// The checkpoint of day `checkpoint`, once read.
    pub fn reached(&self, checkpoint: u32) -> Option<&Reached> {
        self.reached
            .iter()
            .find(|reached| reached.checkpoint == checkpoint)
    }
}

/// The checkpoint to read at `now` for a campaign that started at
/// `started_at`: the first whose day came and that `launch` does not hold
/// for that start. `None` when every one that came was read.
pub fn checkpoint_due(launch: Option<&LaunchState>, started_at: u64, now: u64) -> Option<u32> {
    CHECKPOINTS.into_iter().find(|checkpoint| {
        now >= checkpoint_at(started_at, *checkpoint)
            && !launch.is_some_and(|launch| {
                launch
                    .reached(*checkpoint)
                    .is_some_and(|reached| reached.started_at == started_at)
            })
    })
}

/// The next checkpoint whose day has not come at `now`, with its moment
/// (unix seconds); `None` once the last one came.
pub fn checkpoint_ahead(started_at: u64, now: u64) -> Option<(u32, u64)> {
    CHECKPOINTS
        .into_iter()
        .map(|checkpoint| (checkpoint, checkpoint_at(started_at, checkpoint)))
        .find(|(_, at)| now < *at)
}

pub fn launch_projection<E: Executor>() -> Projection<E, LaunchState> {
    Projection::new::<Launch>()
        .handler(on_target_set())
        .handler(on_thresholds_set())
        .handler(on_checkpoint_reached())
        .handler(on_decision_recorded())
        .strict()
}

#[evento::handler]
async fn on_target_set(event: Event<TargetSet>, launch: &mut LaunchState) -> anyhow::Result<()> {
    launch.id = event.aggregate_id.to_owned();
    launch.targets.push(Target {
        monthly_profit_cents: event.data.monthly_profit_cents,
        currency: event.data.currency.to_owned(),
        by_day: event.data.by_day.to_owned(),
        set_by: event.data.set_by.to_owned(),
        set_at: event.timestamp,
    });
    Ok(())
}

#[evento::handler]
async fn on_thresholds_set(
    event: Event<ThresholdsSet>,
    launch: &mut LaunchState,
) -> anyhow::Result<()> {
    launch.id = event.aggregate_id.to_owned();
    launch.thresholds = event
        .data
        .thresholds
        .iter()
        .map(|figure| (figure.name.clone(), figure.value.clone()))
        .collect();
    Ok(())
}

#[evento::handler]
async fn on_checkpoint_reached(
    event: Event<CheckpointReached>,
    launch: &mut LaunchState,
) -> anyhow::Result<()> {
    launch.id = event.aggregate_id.to_owned();
    let reached = Reached {
        checkpoint: event.data.checkpoint,
        started_at: event.data.started_at,
        reached_at: event.timestamp,
        figures: event
            .data
            .figures
            .iter()
            .map(|figure| (figure.name.clone(), figure.value.clone()))
            .collect(),
        advice: event.data.advice.to_owned(),
        why: event.data.why.to_owned(),
        decision: None,
    };
    // A campaign started again elsewhere reads its checkpoints again: the
    // new reading stands in place of the old.
    launch
        .reached
        .retain(|held| held.checkpoint != reached.checkpoint);
    launch.reached.push(reached);
    launch.reached.sort_by_key(|reached| reached.checkpoint);
    Ok(())
}

#[evento::handler]
async fn on_decision_recorded(
    event: Event<DecisionRecorded>,
    launch: &mut LaunchState,
) -> anyhow::Result<()> {
    if let Some(reached) = launch
        .reached
        .iter_mut()
        .find(|reached| reached.checkpoint == event.data.checkpoint)
    {
        reached.decision = Some(Decision {
            decision: event.data.decision.to_owned(),
            note: event.data.note.to_owned(),
            decided_by: event.data.decided_by.to_owned(),
            decided_at: event.timestamp,
        });
    }
    Ok(())
}

/// Where the notice of a checkpoint stands. No stream is no claim yet.
#[evento::projection(id = id, name = "timada-launch/NoticeState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct NoticeState {
    pub id: String,
    pub checkpoint: u32,
    /// Unix seconds: no retry is made past it.
    pub expires_at: u64,
    pub sending: Sending,
    /// Unix seconds of the last event.
    pub updated_at: u64,
}

pub fn notice_projection<E: Executor>() -> Projection<E, NoticeState> {
    Projection::new::<Notice>()
        .handler(on_notice_owed())
        .handler(on_notice_claimed())
        .handler(on_notice_sent())
        .handler(on_notice_undeliverable())
        .handler(on_notice_deferred())
        .handler(on_notice_failed())
        .strict()
}

#[evento::handler]
async fn on_notice_owed(event: Event<Owed>, state: &mut NoticeState) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.checkpoint = event.data.checkpoint;
    state.expires_at = event.data.expires_at;
    state.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_notice_claimed(
    event: Event<MailClaimed>,
    state: &mut NoticeState,
) -> anyhow::Result<()> {
    state.sending = state.sending.claimed(&event.data.by, event.data.until);
    state.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_notice_sent(event: Event<MailSent>, state: &mut NoticeState) -> anyhow::Result<()> {
    state.sending = Sending::Sent;
    state.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_notice_undeliverable(
    event: Event<MailUndeliverable>,
    state: &mut NoticeState,
) -> anyhow::Result<()> {
    state.sending = Sending::Undeliverable {
        reason: event.data.reason.clone(),
    };
    state.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_notice_deferred(
    event: Event<MailDeferred>,
    state: &mut NoticeState,
) -> anyhow::Result<()> {
    state.sending = Sending::Deferred {
        retry_at: event.data.retry_at,
        attempts: event.data.attempts,
        reason: event.data.reason.clone(),
    };
    state.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_notice_failed(event: Event<MailFailed>, state: &mut NoticeState) -> anyhow::Result<()> {
    state.sending = Sending::Failed {
        reason: event.data.reason.clone(),
        attempts: event.data.attempts,
    };
    state.updated_at = event.timestamp;
    Ok(())
}

impl<E: Executor> Command<E> {
    /// The shop's launch; `None` before anything was set or read.
    pub async fn load_launch(&self) -> anyhow::Result<Option<LaunchState>> {
        launch_projection()
            .load(launch_id())
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await
    }

    /// Loads the launch, lets `decide` say what to write, and writes it;
    /// again when another write came in between. Whether something was
    /// written.
    async fn settle<F>(&self, routing_key: Option<String>, decide: F) -> Result<bool, LaunchError>
    where
        F: Fn(Option<&LaunchState>, &mut evento::WriteBuilder) -> Result<bool, LaunchError>,
    {
        for _ in 0..WRITE_ATTEMPTS {
            let loaded = self.load_launch().await?;
            let mut write = match &loaded {
                Some(launch) => launch.write()?,
                None => {
                    let mut write = evento::append(launch_id());
                    write.original_version(0);
                    write
                }
            };
            if !decide(loaded.as_ref(), &mut write)? {
                return Ok(false);
            }
            write.routing_key_opt(routing_key.clone());
            match write.tenant(&self.tenant).commit(&self.executor).await {
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
    /// Sets what the shop is to reach: `monthly_profit_cents` of profit
    /// after ads a month, in `currency`, by `by_day` (`2027-06-30`, after
    /// `now`). The one before stays in the stream; the same again is a
    /// no-op.
    pub async fn set_target(
        &self,
        actor_user_id: &str,
        monthly_profit_cents: u64,
        currency: &str,
        by_day: &str,
        now: u64,
        routing_key: Option<String>,
    ) -> Result<(), LaunchError> {
        let currency =
            timada_core::normalize_currency(currency).ok_or(LaunchError::InvalidTarget)?;
        if monthly_profit_cents == 0 || monthly_profit_cents > 100_000_000_000 {
            return Err(LaunchError::InvalidTarget);
        }
        let by_day = by_day.trim();
        let day = timada_core::civil::parse_date(by_day)
            .filter(|_| by_day.len() == 10)
            .ok_or(LaunchError::InvalidTargetDay)?;
        if day <= now {
            return Err(LaunchError::InvalidTargetDay);
        }
        self.settle(routing_key, |launch, write| {
            let same = launch.and_then(LaunchState::target).is_some_and(|target| {
                target.monthly_profit_cents == monthly_profit_cents
                    && target.currency == currency
                    && target.by_day == by_day
            });
            if same {
                return Ok(false);
            }
            write.event(&TargetSet {
                monthly_profit_cents,
                currency: currency.clone(),
                by_day: by_day.to_owned(),
                set_by: actor_user_id.to_owned(),
            });
            Ok(true)
        })
        .await?;
        Ok(())
    }

    /// Sets the thresholds the checkpoints are judged on: each of
    /// [`THRESHOLDS`] with its value, the default for one left out. The
    /// same again is a no-op. What was read already stays as it was read.
    pub async fn set_thresholds(
        &self,
        actor_user_id: &str,
        thresholds: Vec<(String, String)>,
        routing_key: Option<String>,
    ) -> Result<(), LaunchError> {
        let mut kept = Vec::with_capacity(THRESHOLDS.len());
        for (name, default) in THRESHOLDS {
            let value = thresholds
                .iter()
                .find(|(named, _)| named == name)
                .map_or(default, |(_, value)| value.as_str());
            kept.push((name.to_owned(), normalize_threshold(name, value)?));
        }
        // Together: the bar to go on is never under the bar to stop.
        thresholds_of(
            kept.iter()
                .map(|(name, value)| (name.as_str(), value.as_str())),
        )?;
        self.settle(routing_key, |launch, write| {
            if launch.is_some_and(|launch| launch.thresholds == kept) {
                return Ok(false);
            }
            write.event(&ThresholdsSet {
                thresholds: kept
                    .iter()
                    .map(|(name, value)| Figure {
                        name: name.clone(),
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

    /// Records the checkpoint `reading` is of, for a campaign that started
    /// at `started_at`: its numbers, and what they say against the shop's
    /// thresholds. Whether it was written: one read already for that start
    /// (by another node) is left as it is.
    pub async fn record_checkpoint(
        &self,
        actor: &str,
        started_at: u64,
        reading: Reading,
        routing_key: Option<String>,
    ) -> Result<bool, LaunchError> {
        let checkpoint = reading.checkpoint();
        self.settle(routing_key, |launch, write| {
            let read = launch.is_some_and(|launch| {
                launch
                    .reached(checkpoint)
                    .is_some_and(|reached| reached.started_at == started_at)
            });
            if read {
                return Ok(false);
            }
            let thresholds = launch.map(LaunchState::thresholds).unwrap_or_default();
            let (advice, why) = judge(&thresholds, &reading);
            write.event(&CheckpointReached {
                checkpoint,
                started_at,
                figures: reading
                    .figures()
                    .into_iter()
                    .map(|(name, value)| Figure { name, value })
                    .collect(),
                advice: advice.to_owned(),
                why,
                reached_by: actor.to_owned(),
            });
            Ok(true)
        })
        .await
    }

    /// Records what the merchant decided at `checkpoint`, and why. Said
    /// again, the last word stands; the same again is a no-op.
    pub async fn record_decision(
        &self,
        actor_user_id: &str,
        checkpoint: u32,
        decision: &str,
        note: &str,
        routing_key: Option<String>,
    ) -> Result<(), LaunchError> {
        if !CHECKPOINTS.contains(&checkpoint) {
            return Err(LaunchError::UnknownCheckpoint);
        }
        if !ADVICES.contains(&decision) {
            return Err(LaunchError::InvalidDecision);
        }
        let note = note.trim();
        if note.chars().count() > MAX_NOTE_LEN {
            return Err(LaunchError::NoteTooLong);
        }
        self.settle(routing_key, |launch, write| {
            let reached = launch
                .and_then(|launch| launch.reached(checkpoint))
                .ok_or(LaunchError::NotReached)?;
            let same = reached
                .decision
                .as_ref()
                .is_some_and(|held| held.decision == decision && held.note == note);
            if same {
                return Ok(false);
            }
            write.event(&DecisionRecorded {
                checkpoint,
                decision: decision.to_owned(),
                note: note.to_owned(),
                decided_by: actor_user_id.to_owned(),
            });
            Ok(true)
        })
        .await?;
        Ok(())
    }
}
