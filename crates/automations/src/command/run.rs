use evento::{Executor, Projection, ProjectionAggregate, WriteError, metadata::Event};

use crate::aggregator::run::{
    Branched, Cancelled, Finished, Started, StepClaimed, StepFailed, StepSkipped, StepSucceeded,
    Waiting,
};
use crate::error::AutomationError;
use crate::value_object::{CLAIM_LEASE_SECS, RunStatus, run_id};

/// What became of a step a run reached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// A wait began, until then.
    Waiting {
        until: u64,
    },
    /// A condition sent the run down this lane.
    Branched(u32),
    /// Taken to act, its outcome not known yet.
    Claimed {
        attempt: u32,
    },
    Succeeded {
        output: String,
    },
    Skipped {
        reason: String,
    },
    Failed {
        attempt: u32,
        reason: String,
        retry_at: Option<u64>,
    },
}

/// One line of a run's log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepRecord {
    pub step: String,
    pub outcome: Outcome,
    /// Unix seconds.
    pub at: u64,
}

/// The write model of a run: what started it and its log.
#[evento::projection(id = id, name = "timada-automations/RunState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct RunState {
    pub id: String,
    pub scenario_id: String,
    pub scenario_version: u32,
    pub trigger: String,
    pub trigger_event_id: String,
    pub subject_id: String,
    pub status: RunStatus,
    /// What each step reached did, in order; a step tried again appears
    /// again.
    pub log: Vec<StepRecord>,
    /// Why it failed or was cancelled.
    pub reason: Option<String>,
    /// Unix seconds of `Started`.
    pub started_at: u64,
    /// Unix seconds of the last event.
    pub updated_at: u64,
    /// Unix seconds of its end.
    pub ended_at: Option<u64>,
}

impl RunState {
    /// The last line of a step's log.
    pub fn latest(&self, step: &str) -> Option<&StepRecord> {
        self.log.iter().rev().find(|record| record.step == step)
    }

    /// How many times a step was taken to act.
    pub fn attempts(&self, step: &str) -> u32 {
        self.log
            .iter()
            .filter(|record| {
                record.step == step && matches!(record.outcome, Outcome::Claimed { .. })
            })
            .count() as u32
    }

    /// When the run is to be looked at again; `None` once it ended.
    pub fn next_at(&self) -> Option<u64> {
        if self.status.is_done() {
            return None;
        }
        Some(
            match self.log.last().map(|record| (&record.outcome, record.at)) {
                Some((Outcome::Waiting { until }, _)) => *until,
                Some((Outcome::Claimed { .. }, at)) => at + CLAIM_LEASE_SECS,
                Some((
                    Outcome::Failed {
                        retry_at: Some(at), ..
                    },
                    _,
                )) => *at,
                _ => self.updated_at,
            },
        )
    }

    fn record(&mut self, step: &str, outcome: Outcome, at: u64) {
        self.log.push(StepRecord {
            step: step.to_owned(),
            outcome,
            at,
        });
        self.updated_at = at;
    }
}

pub fn run_projection<E: Executor>() -> Projection<E, RunState> {
    Projection::new::<crate::aggregator::run::Run>()
        .handler(on_started())
        .handler(on_waiting())
        .handler(on_branched())
        .handler(on_step_claimed())
        .handler(on_step_succeeded())
        .handler(on_step_skipped())
        .handler(on_step_failed())
        .handler(on_finished())
        .handler(on_cancelled())
        .strict()
}

#[evento::handler]
async fn on_started(event: Event<Started>, state: &mut RunState) -> anyhow::Result<()> {
    let data = &event.data;
    state.id = event.aggregate_id.to_owned();
    state.scenario_id = data.scenario_id.to_owned();
    state.scenario_version = data.scenario_version;
    state.trigger = data.trigger.to_owned();
    state.trigger_event_id = data.trigger_event_id.to_owned();
    state.subject_id = data.subject_id.to_owned();
    state.status = RunStatus::Running;
    state.started_at = event.timestamp;
    state.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_waiting(event: Event<Waiting>, state: &mut RunState) -> anyhow::Result<()> {
    let until = event.data.until;
    state.record(
        &event.data.step,
        Outcome::Waiting { until },
        event.timestamp,
    );
    state.status = RunStatus::Waiting;
    Ok(())
}

#[evento::handler]
async fn on_branched(event: Event<Branched>, state: &mut RunState) -> anyhow::Result<()> {
    let branch = event.data.branch;
    state.record(&event.data.step, Outcome::Branched(branch), event.timestamp);
    state.status = RunStatus::Running;
    Ok(())
}

#[evento::handler]
async fn on_step_claimed(event: Event<StepClaimed>, state: &mut RunState) -> anyhow::Result<()> {
    let attempt = event.data.attempt;
    state.record(
        &event.data.step,
        Outcome::Claimed { attempt },
        event.timestamp,
    );
    state.status = RunStatus::Running;
    Ok(())
}

#[evento::handler]
async fn on_step_succeeded(
    event: Event<StepSucceeded>,
    state: &mut RunState,
) -> anyhow::Result<()> {
    let output = event.data.output.to_owned();
    state.record(
        &event.data.step,
        Outcome::Succeeded { output },
        event.timestamp,
    );
    state.status = RunStatus::Running;
    Ok(())
}

#[evento::handler]
async fn on_step_skipped(event: Event<StepSkipped>, state: &mut RunState) -> anyhow::Result<()> {
    let reason = event.data.reason.to_owned();
    state.record(
        &event.data.step,
        Outcome::Skipped { reason },
        event.timestamp,
    );
    state.status = RunStatus::Running;
    Ok(())
}

#[evento::handler]
async fn on_step_failed(event: Event<StepFailed>, state: &mut RunState) -> anyhow::Result<()> {
    let data = &event.data;
    state.record(
        &data.step,
        Outcome::Failed {
            attempt: data.attempt,
            reason: data.reason.to_owned(),
            retry_at: data.retry_at,
        },
        event.timestamp,
    );
    if data.retry_at.is_none() {
        state.status = RunStatus::Failed;
        state.reason = Some(data.reason.to_owned());
        state.ended_at = Some(event.timestamp);
    } else {
        state.status = RunStatus::Running;
    }
    Ok(())
}

#[evento::handler]
async fn on_finished(event: Event<Finished>, state: &mut RunState) -> anyhow::Result<()> {
    state.status = RunStatus::Finished;
    state.updated_at = event.timestamp;
    state.ended_at = Some(event.timestamp);
    Ok(())
}

#[evento::handler]
async fn on_cancelled(event: Event<Cancelled>, state: &mut RunState) -> anyhow::Result<()> {
    state.status = RunStatus::Cancelled;
    state.reason = Some(event.data.reason.to_owned());
    state.updated_at = event.timestamp;
    state.ended_at = Some(event.timestamp);
    Ok(())
}

/// What starts a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartRun {
    pub scenario_id: String,
    pub scenario_version: u32,
    pub trigger: String,
    /// The id of the event that started it (unique in the store).
    pub trigger_event_id: String,
    pub subject_id: String,
}

/// One move of a run, written by [`Command::advance_run`](super::Command::advance_run).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Transition {
    Wait {
        step: String,
        until: u64,
    },
    Branch {
        step: String,
        branch: u32,
    },
    Claim {
        step: String,
        attempt: u32,
    },
    Succeed {
        step: String,
        output: String,
    },
    Skip {
        step: String,
        reason: String,
    },
    Fail {
        step: String,
        attempt: u32,
        reason: String,
        retry_at: Option<u64>,
    },
    Finish,
    Cancel {
        reason: String,
    },
}

/// A reason as stored: short.
fn short(reason: &str) -> String {
    reason.chars().take(500).collect()
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Starts the run an event makes of a scenario; whether this call
    /// started it (`false`: started already, by another node say).
    pub async fn start_run(
        &self,
        actor: &str,
        start: &StartRun,
        routing_key: Option<String>,
    ) -> Result<bool, AutomationError> {
        let id = run_id(&start.scenario_id, &start.trigger_event_id);
        // Read first: most of the time another node started it already, and
        // a refused write costs a round of consensus.
        if self.load_run(&id).await?.is_some() {
            return Ok(false);
        }
        let started = evento::append(&id)
            .original_version(0)
            .routing_key_opt(routing_key)
            .event(&Started {
                scenario_id: start.scenario_id.clone(),
                scenario_version: start.scenario_version,
                trigger: start.trigger.clone(),
                trigger_event_id: start.trigger_event_id.clone(),
                subject_id: start.subject_id.clone(),
                started_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await;
        match started {
            Ok(_) => Ok(true),
            Err(WriteError::InvalidOriginalVersion) => Ok(false),
            Err(err) => Err(err.into()),
        }
    }

    /// Writes moves of a run, in one commit, against the version `run` was
    /// loaded at: whether they were written (`false`: another node moved
    /// the run meanwhile, and nothing was).
    pub async fn advance_run(
        &self,
        actor: &str,
        run: &RunState,
        transitions: &[Transition],
        routing_key: Option<String>,
    ) -> Result<bool, AutomationError> {
        let mut write = run.write()?;
        write.routing_key_opt(routing_key);
        let by = actor.to_owned();
        for transition in transitions {
            match transition {
                Transition::Wait { step, until } => write.event(&Waiting {
                    step: step.clone(),
                    until: *until,
                    waited_by: by.clone(),
                }),
                Transition::Branch { step, branch } => write.event(&Branched {
                    step: step.clone(),
                    branch: *branch,
                    branched_by: by.clone(),
                }),
                Transition::Claim { step, attempt } => write.event(&StepClaimed {
                    step: step.clone(),
                    attempt: *attempt,
                    claimed_by: by.clone(),
                }),
                Transition::Succeed { step, output } => write.event(&StepSucceeded {
                    step: step.clone(),
                    output: output.clone(),
                    recorded_by: by.clone(),
                }),
                Transition::Skip { step, reason } => write.event(&StepSkipped {
                    step: step.clone(),
                    reason: short(reason),
                    recorded_by: by.clone(),
                }),
                Transition::Fail {
                    step,
                    attempt,
                    reason,
                    retry_at,
                } => write.event(&StepFailed {
                    step: step.clone(),
                    attempt: *attempt,
                    reason: short(reason),
                    retry_at: *retry_at,
                    recorded_by: by.clone(),
                }),
                Transition::Finish => write.event(&Finished {
                    finished_by: by.clone(),
                }),
                Transition::Cancel { reason } => write.event(&Cancelled {
                    reason: short(reason),
                    cancelled_by: by.clone(),
                }),
            };
        }
        match write.tenant(&self.tenant).commit(&self.executor).await {
            Ok(_) => Ok(true),
            Err(WriteError::InvalidOriginalVersion) => Ok(false),
            Err(err) => Err(err.into()),
        }
    }

    /// Stops a run before its end; ended already is a no-op (`false`).
    pub async fn cancel_run(
        &self,
        actor: &str,
        run_id: &str,
        reason: &str,
        routing_key: Option<String>,
    ) -> Result<bool, AutomationError> {
        let Some(run) = self.load_run(run_id).await? else {
            return Err(AutomationError::RunNotFound);
        };
        if run.status.is_done() {
            return Ok(false);
        }
        let cancelled = self
            .advance_run_opt(
                actor,
                &run,
                &[Transition::Cancel {
                    reason: reason.to_owned(),
                }],
                routing_key,
            )
            .await?;
        if !cancelled {
            return Err(AutomationError::RunChanged);
        }
        Ok(true)
    }
}

impl<E: Executor> super::Command<E> {
    pub async fn load_run(&self, run_id: &str) -> anyhow::Result<Option<RunState>> {
        run_projection()
            .load(run_id)
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await
    }
}
