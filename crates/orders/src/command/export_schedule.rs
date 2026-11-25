use evento::{Executor, Projection, ProjectionAggregate, metadata::Event};
use jiff::{
    ToSpan,
    civil::{Date, Weekday},
    tz::TimeZone,
};

use crate::aggregator::{
    ExportRunDelivered, ExportRunFailed, ExportRunStarted, ExportScheduled, ExportUnscheduled,
};
use crate::error::OrderError;
use crate::value_object::{MAX_EXPORT_REASON_LEN, export_schedule_id};

/// What a scheduled export is a file of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExportKind {
    /// The orders placed over the period.
    Orders,
    /// The shoppers who first signed in over it.
    Customers,
    /// The newsletter's subscribers who asked over it.
    Newsletter,
    /// The sales report of the period: what was sold, by what brought the
    /// shopper, by product and by category, with what the ads cost.
    Sales,
}

impl ExportKind {
    pub const ALL: [Self; 4] = [Self::Orders, Self::Customers, Self::Newsletter, Self::Sales];

    pub fn key(self) -> &'static str {
        match self {
            Self::Orders => "orders",
            Self::Customers => "customers",
            Self::Newsletter => "newsletter",
            Self::Sales => "sales",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.key() == value)
    }
}

/// How often a scheduled export goes out: once each whole day, week
/// (Monday to Sunday) or month has ended on the shop's clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportEvery {
    Daily,
    Weekly,
    Monthly,
}

impl ExportEvery {
    pub const ALL: [Self; 3] = [Self::Daily, Self::Weekly, Self::Monthly];

    pub fn key(self) -> &'static str {
        match self {
            Self::Daily => "daily",
            Self::Weekly => "weekly",
            Self::Monthly => "monthly",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|every| every.key() == value)
    }

    /// The first day of the day, week or month `day` is in.
    pub fn start_of(self, day: Date) -> Date {
        match self {
            Self::Daily => day,
            Self::Weekly => day
                .checked_sub(i64::from(day.weekday().since(Weekday::Monday)).days())
                .unwrap_or(day),
            Self::Monthly => day.first_of_month(),
        }
    }

    /// The first day of the one after.
    pub fn next_start(self, day: Date) -> Date {
        let start = self.start_of(day);
        let next = match self {
            Self::Daily => start.checked_add(1.day()),
            Self::Weekly => start.checked_add(7.days()),
            Self::Monthly => start.checked_add(1.month()),
        };
        next.unwrap_or(start)
    }

    /// Unix seconds an export may go out at for the period `day` is in:
    /// [`EXPORT_RUNS_AFTER_SECS`] after that period ended in `zone`.
    pub fn due_at(self, day: Date, zone: &TimeZone) -> u64 {
        let ended = self
            .next_start(day)
            .to_zoned(zone.clone())
            .map_or(0, |start| start.timestamp().as_second());
        ended.max(0) as u64 + EXPORT_RUNS_AFTER_SECS
    }
}

/// How long after a period ended its export goes out: what was sold in the
/// last minute of the day has its row, and its invoice, by then.
pub const EXPORT_RUNS_AFTER_SECS: u64 = 60 * 60;

/// How many times one export is tried before it is given up.
pub const MAX_EXPORT_ATTEMPTS: u32 = 5;

/// How long after a try that may do better later the next one waits, by
/// how many were made.
pub const EXPORT_RETRY_DELAYS_SECS: [u64; 4] = [5 * 60, 30 * 60, 2 * 60 * 60, 6 * 60 * 60];

/// How long a run that was started and never settled (its node stopped) is
/// left alone before another try takes it.
pub const EXPORT_RUN_STALE_SECS: u64 = 15 * 60;

/// Where a scheduled export goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportDestination {
    /// A mail to this address that says the file is ready, with a link to
    /// it in the back office: the file itself never travels by mail, and
    /// the link opens for a member signed in who may read it.
    Mail(String),
    /// The file posted to an address, signed. The address is the host's to
    /// seal before it gets here, as a scenario's webhook is (base64).
    Url { sealed: String },
}

/// A schedule as it stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportPlan {
    pub every: ExportEvery,
    pub destination: ExportDestination,
    /// The first day the first export covers.
    pub since: Date,
}

/// A run started and not settled yet, or failed and waiting for its next
/// try.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenExportRun {
    pub first_day: Date,
    pub last_day: Date,
    /// How many tries were made, this one included.
    pub attempt: u32,
    /// Unix seconds it may be tried again at.
    pub again_at: u64,
}

/// How the last run that ended went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndedExportRun {
    pub first_day: Date,
    pub last_day: Date,
    /// Unix seconds.
    pub at: u64,
    /// Why it was given up; `None`: delivered.
    pub failure: Option<String>,
}

/// The write model of one scheduled export of a shop.
#[evento::projection(id = id, name = "timada-orders/ExportScheduleState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct ExportScheduleState {
    pub id: String,
    /// `orders`, `customers` or `newsletter`.
    pub what: String,
    /// `None`: taken off.
    pub plan: Option<ExportPlan>,
    /// The last day an export that ended covered, delivered or given up.
    pub covered_through: Option<Date>,
    pub run: Option<OpenExportRun>,
    pub ended: Option<EndedExportRun>,
    /// Unix seconds the schedule is next looked at: what the read model
    /// keeps of it. `None` once taken off.
    pub due_at: Option<u64>,
}

impl ExportScheduleState {
    /// The days the next export covers when it may start at `now`, on a day
    /// that is `today` on the shop's clock: the run waiting for its next
    /// try once its time came, else every day since the last export up to
    /// the end of the last whole period. `None` while there is nothing to
    /// send yet.
    pub fn next_run(&self, now: u64, today: Date) -> Option<(Date, Date, u32)> {
        let plan = self.plan.as_ref()?;
        if let Some(run) = &self.run {
            return (run.again_at <= now).then_some((run.first_day, run.last_day, run.attempt + 1));
        }
        let first = match self.covered_through {
            Some(day) => day.tomorrow().ok()?,
            None => plan.since,
        };
        let last = plan.every.start_of(today).yesterday().ok()?;
        (first <= last).then_some((first, last, 1))
    }
}

fn day(text: &str) -> Date {
    text.parse().unwrap_or(Date::ZERO)
}

pub fn export_schedule_projection<E: Executor>() -> Projection<E, ExportScheduleState> {
    Projection::new::<crate::aggregator::ExportSchedule>()
        .handler(on_export_scheduled())
        .handler(on_export_unscheduled())
        .handler(on_export_run_started())
        .handler(on_export_run_delivered())
        .handler(on_export_run_failed())
        .strict()
}

#[evento::handler]
async fn on_export_scheduled(
    event: Event<ExportScheduled>,
    state: &mut ExportScheduleState,
) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.what = event.data.what.clone();
    state.plan = ExportEvery::parse(&event.data.every).map(|every| ExportPlan {
        every,
        destination: match (&event.data.mail, &event.data.url_sealed) {
            (_, Some(sealed)) => ExportDestination::Url {
                sealed: sealed.clone(),
            },
            (mail, None) => ExportDestination::Mail(mail.clone().unwrap_or_default()),
        },
        since: day(&event.data.since_day),
    });
    // A schedule set again starts over: what the one before sent, or was
    // sending, is not this one's.
    state.covered_through = None;
    state.run = None;
    state.due_at = Some(event.data.due_at);
    Ok(())
}

#[evento::handler]
async fn on_export_unscheduled(
    _event: Event<ExportUnscheduled>,
    state: &mut ExportScheduleState,
) -> anyhow::Result<()> {
    state.plan = None;
    state.run = None;
    state.due_at = None;
    Ok(())
}

#[evento::handler]
async fn on_export_run_started(
    event: Event<ExportRunStarted>,
    state: &mut ExportScheduleState,
) -> anyhow::Result<()> {
    let again_at = event.data.started_at + EXPORT_RUN_STALE_SECS;
    state.run = Some(OpenExportRun {
        first_day: day(&event.data.first_day),
        last_day: day(&event.data.last_day),
        attempt: event.data.attempt,
        again_at,
    });
    state.due_at = Some(again_at);
    Ok(())
}

#[evento::handler]
async fn on_export_run_delivered(
    event: Event<ExportRunDelivered>,
    state: &mut ExportScheduleState,
) -> anyhow::Result<()> {
    if let Some(run) = state.run.take() {
        state.covered_through = Some(run.last_day);
        state.ended = Some(EndedExportRun {
            first_day: run.first_day,
            last_day: run.last_day,
            at: event.data.delivered_at,
            failure: None,
        });
    }
    state.due_at = Some(event.data.due_at);
    Ok(())
}

#[evento::handler]
async fn on_export_run_failed(
    event: Event<ExportRunFailed>,
    state: &mut ExportScheduleState,
) -> anyhow::Result<()> {
    match (event.data.retry_at, &mut state.run) {
        (Some(retry_at), Some(run)) => {
            run.again_at = retry_at;
            state.due_at = Some(retry_at);
        }
        _ => {
            if let Some(run) = state.run.take() {
                state.covered_through = Some(run.last_day);
                state.ended = Some(EndedExportRun {
                    first_day: run.first_day,
                    last_day: run.last_day,
                    at: event.data.failed_at,
                    failure: Some(event.data.reason.clone()),
                });
            }
            state.due_at = Some(event.data.due_at);
        }
    }
    Ok(())
}

/// A run this node started: what to send, and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportRun {
    pub kind: ExportKind,
    pub every: ExportEvery,
    pub destination: ExportDestination,
    /// The days covered, both included.
    pub first_day: Date,
    pub last_day: Date,
    pub attempt: u32,
}

/// How sending a run went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportOutcome {
    Delivered,
    /// Not taken, and later may do better (no answer, a 5xx).
    Again(String),
    /// Not taken, and asking again would meet the same (a 4xx, an address
    /// that is not public).
    Refused(String),
}

/// The moment a schedule is looked at, on the shop's clock.
#[derive(Debug, Clone)]
pub struct ExportClock {
    /// Unix seconds.
    pub now: u64,
    /// Where the shop lives.
    pub zone: TimeZone,
}

impl ExportClock {
    pub fn new(now: u64, zone: TimeZone) -> Self {
        Self { now, zone }
    }

    /// The day it is there.
    pub fn today(&self) -> Date {
        jiff::Timestamp::from_second(self.now as i64)
            .map(|at| at.to_zoned(self.zone.clone()).date())
            .unwrap_or(Date::ZERO)
    }
}

impl<E: Executor> super::Command<E> {
    /// The shop's scheduled export of `kind`; `None` before one was ever
    /// set.
    pub async fn load_export_schedule(
        &self,
        kind: ExportKind,
    ) -> anyhow::Result<Option<ExportScheduleState>> {
        export_schedule_projection()
            .load(export_schedule_id(kind))
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await
    }
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Schedules the shop's export of `kind`, or changes it: from the next
    /// time a whole day, week or month has ended on the shop's clock, the
    /// file of that period goes to `destination`. The first covers the
    /// period `clock` is in, whole. A schedule set again as it
    /// stands writes nothing; one changed starts over.
    ///
    /// The caller answers for who may do this (an owner of the shop): a
    /// file of customers sent on its own leaves the shop's hands.
    pub async fn schedule_export(
        &self,
        actor: &str,
        kind: ExportKind,
        every: ExportEvery,
        destination: ExportDestination,
        clock: &ExportClock,
        routing_key: Option<String>,
    ) -> Result<(), OrderError> {
        let destination = match destination {
            ExportDestination::Mail(address) => ExportDestination::Mail(
                timada_core::normalize_email(&address).ok_or(OrderError::InvalidExportAddress)?,
            ),
            ExportDestination::Url { sealed } if sealed.is_empty() => {
                return Err(OrderError::InvalidExportAddress);
            }
            url => url,
        };
        let state = self.load_export_schedule(kind).await?;
        let stands = state
            .as_ref()
            .and_then(|state| state.plan.as_ref())
            .is_some_and(|plan| plan.every == every && plan.destination == destination);
        if stands {
            return Ok(());
        }
        let today = clock.today();
        let (mail, url_sealed) = match destination {
            ExportDestination::Mail(address) => (Some(address), None),
            ExportDestination::Url { sealed } => (None, Some(sealed)),
        };
        let mut write = match &state {
            Some(state) => state.write()?,
            None => {
                let mut write = evento::append(export_schedule_id(kind));
                write.original_version(0);
                write
            }
        };
        write
            .routing_key_opt(routing_key)
            .event(&ExportScheduled {
                what: kind.key().to_owned(),
                every: every.key().to_owned(),
                mail,
                url_sealed,
                since_day: every.start_of(today).to_string(),
                due_at: every.due_at(today, &clock.zone),
                scheduled_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }

    /// Takes the shop's scheduled export of `kind` off. Already off is a
    /// no-op.
    pub async fn unschedule_export(
        &self,
        actor: &str,
        kind: ExportKind,
        routing_key: Option<String>,
    ) -> Result<(), OrderError> {
        let Some(state) = self.load_export_schedule(kind).await? else {
            return Ok(());
        };
        if state.plan.is_none() {
            return Ok(());
        }
        state
            .write()?
            .routing_key_opt(routing_key)
            .event(&ExportUnscheduled {
                unscheduled_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }

    /// Starts the export of `kind` that is due on the shop's clock, if one
    /// is: the claim on it, written at the version read,
    /// so of the nodes that find it due one starts it and the others are
    /// told there is none (`None`, as when nothing is due). The caller
    /// sends it, then says how that went
    /// ([`settle_export_run`](Self::settle_export_run)); a run never
    /// settled is started again after [`EXPORT_RUN_STALE_SECS`].
    pub async fn start_export_run(
        &self,
        actor: &str,
        kind: ExportKind,
        clock: &ExportClock,
        routing_key: Option<String>,
    ) -> Result<Option<ExportRun>, OrderError> {
        let Some(state) = self.load_export_schedule(kind).await? else {
            return Ok(None);
        };
        let Some(plan) = state.plan.clone() else {
            return Ok(None);
        };
        let Some((first_day, last_day, attempt)) = state.next_run(clock.now, clock.today()) else {
            return Ok(None);
        };
        let written = state
            .write()?
            .routing_key_opt(routing_key)
            .event(&ExportRunStarted {
                first_day: first_day.to_string(),
                last_day: last_day.to_string(),
                attempt,
                started_at: clock.now,
                started_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await;
        match written {
            Ok(_) => Ok(Some(ExportRun {
                kind,
                every: plan.every,
                destination: plan.destination,
                first_day,
                last_day,
                attempt,
            })),
            // Another node started it.
            Err(evento::WriteError::InvalidOriginalVersion) => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    /// Records how the run of `kind` started as try `attempt` went, at
    /// `clock`. Delivered, or refused for good, or failed once too often
    /// ([`MAX_EXPORT_ATTEMPTS`]): the schedule moves on to the next period.
    /// Otherwise the same days are tried again later
    /// ([`EXPORT_RETRY_DELAYS_SECS`]). A run settled already, or started
    /// again since, is left as it is.
    pub async fn settle_export_run(
        &self,
        actor: &str,
        kind: ExportKind,
        attempt: u32,
        outcome: ExportOutcome,
        clock: &ExportClock,
        routing_key: Option<String>,
    ) -> Result<(), OrderError> {
        let Some(state) = self.load_export_schedule(kind).await? else {
            return Ok(());
        };
        let (Some(plan), Some(run)) = (&state.plan, &state.run) else {
            return Ok(());
        };
        if run.attempt != attempt {
            return Ok(());
        }
        // The period now running ends the next export's days.
        let now = clock.now;
        let due_at = plan.every.due_at(clock.today(), &clock.zone);
        let cut = |reason: String| -> String {
            reason
                .chars()
                .map(|c| if c.is_control() { ' ' } else { c })
                .take(MAX_EXPORT_REASON_LEN)
                .collect()
        };
        let mut write = state.write()?;
        write.routing_key_opt(routing_key);
        match outcome {
            ExportOutcome::Delivered => write.event(&ExportRunDelivered {
                delivered_at: now,
                due_at,
                delivered_by: actor.to_owned(),
            }),
            ExportOutcome::Again(reason) if attempt < MAX_EXPORT_ATTEMPTS => {
                let delay = EXPORT_RETRY_DELAYS_SECS
                    [(attempt as usize - 1).min(EXPORT_RETRY_DELAYS_SECS.len() - 1)];
                write.event(&ExportRunFailed {
                    reason: cut(reason),
                    failed_at: now,
                    retry_at: Some(now + delay),
                    due_at,
                    failed_by: actor.to_owned(),
                })
            }
            ExportOutcome::Again(reason) | ExportOutcome::Refused(reason) => {
                write.event(&ExportRunFailed {
                    reason: cut(reason),
                    failed_at: now,
                    retry_at: None,
                    due_at,
                    failed_by: actor.to_owned(),
                })
            }
        };
        match write.tenant(&self.tenant).commit(&self.executor).await {
            // The schedule changed meanwhile: its run went with it.
            Ok(_) | Err(evento::WriteError::InvalidOriginalVersion) => Ok(()),
            Err(err) => Err(err.into()),
        }
    }
}
