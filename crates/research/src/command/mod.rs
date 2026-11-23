//! Write side: the command gateway and the strict write model it loads.
//!
//! The write model folds *all* events of a study (`strict()`), so the
//! version it observes, and `write()` sends as `original_version`, never
//! goes stale. It keeps where each seed's fetch stands and how many rows it
//! gave, never the rows themselves: those are the read model's. It is not
//! snapshotted: a study is a handful of seeds and of fetches.
//!
//! A study's stream is written by its owner and by the host's saga at the
//! same time (a rename while keywords arrive, two seeds fetched by two
//! nodes), so a write that lost to another is no verdict: every command
//! loads the study again and decides again ([`WRITE_ATTEMPTS`] times).

mod fetch;
mod study;

use evento::{
    Executor, Projection, ProjectionAggregate, WriteBuilder, WriteError, metadata::Event,
};

pub use study::*;

use crate::aggregator::{
    Created, Deleted, FetchFailed, FetchRequested, FetchStarted, KeywordsFetched, Renamed,
    SeedAdded, Study,
};
use crate::error::ResearchError;
use crate::value_object::{FETCH_STALE_SECS, FetchStatus};

/// The command gateway over the platform's streams. Cheap to clone when `E`
/// is (the dashboard's executor is an `Arc` wrapper).
pub struct Command<E: Executor>(pub E);

/// How many times a command decides again when another write to the study
/// came in between its load and its write.
const WRITE_ATTEMPTS: usize = 5;

/// One seed of a study, and where its fetch stands.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SeedState {
    pub seed: String,
    pub status: FetchStatus,
    /// Unix seconds of the last `FetchRequested`; 0 when never asked for.
    pub requested_at: u64,
    /// Unix seconds of the last fetch that gave its rows.
    pub fetched_at: Option<u64>,
    /// How many rows that fetch gave.
    pub keywords: u32,
    /// Why the last fetch gave nothing; cleared by the next request.
    pub last_error: Option<String>,
}

impl SeedState {
    /// Whether a fetch of the seed is on its way at `now` (unix seconds):
    /// asked for or taken on, and not so long ago that the node which took
    /// it on must have stopped.
    pub fn is_fetching(&self, now: u64) -> bool {
        matches!(self.status, FetchStatus::Requested | FetchStatus::Fetching)
            && now.saturating_sub(self.requested_at) < FETCH_STALE_SECS
    }

    /// Whether the seed still wants its keywords at `now`: never fetched,
    /// or its last fetch failed or stalled.
    pub fn is_owed(&self, now: u64) -> bool {
        self.status != FetchStatus::Fetched && !self.is_fetching(now)
    }
}

/// The write model of a study.
#[evento::projection(id = id, name = "timada-research/StudyState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct StudyState {
    pub id: String,
    /// The user who made it, and the only one who opens it.
    pub owner: String,
    pub name: String,
    pub country: String,
    pub language: String,
    /// In the order they were added.
    pub seeds: Vec<SeedState>,
    /// What its fetches cost in all, in millionths of a US dollar.
    pub cost_usd_micros: u64,
    /// Unix seconds of `Created`.
    pub created_at: u64,
    /// Unix seconds of the last event.
    pub updated_at: u64,
    pub deleted: bool,
}

impl StudyState {
    pub fn is_owner(&self, user_id: &str) -> bool {
        self.owner == user_id
    }

    pub fn seed(&self, seed: &str) -> Option<&SeedState> {
        self.seeds.iter().find(|state| state.seed == seed)
    }

    fn seed_mut(&mut self, seed: &str) -> Option<&mut SeedState> {
        self.seeds.iter_mut().find(|state| state.seed == seed)
    }

    /// How many rows its seeds' last fetches gave together.
    pub fn keywords(&self) -> u64 {
        self.seeds.iter().map(|seed| u64::from(seed.keywords)).sum()
    }
}

pub fn study_projection<E: Executor>() -> Projection<E, StudyState> {
    Projection::new::<Study>()
        .handler(on_created())
        .handler(on_renamed())
        .handler(on_seed_added())
        .handler(on_fetch_requested())
        .handler(on_fetch_started())
        .handler(on_keywords_fetched())
        .handler(on_fetch_failed())
        .handler(on_deleted())
        .strict()
}

#[evento::handler]
async fn on_created(event: Event<Created>, study: &mut StudyState) -> anyhow::Result<()> {
    study.id = event.aggregate_id.to_owned();
    study.owner = event.data.created_by.to_owned();
    study.name = event.data.name.to_owned();
    study.country = event.data.country.to_owned();
    study.language = event.data.language.to_owned();
    study.created_at = event.timestamp;
    study.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_renamed(event: Event<Renamed>, study: &mut StudyState) -> anyhow::Result<()> {
    study.name = event.data.name.to_owned();
    study.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_seed_added(event: Event<SeedAdded>, study: &mut StudyState) -> anyhow::Result<()> {
    if study.seed(&event.data.seed).is_none() {
        study.seeds.push(SeedState {
            seed: event.data.seed.to_owned(),
            ..SeedState::default()
        });
    }
    study.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_fetch_requested(
    event: Event<FetchRequested>,
    study: &mut StudyState,
) -> anyhow::Result<()> {
    if let Some(seed) = study.seed_mut(&event.data.seed) {
        seed.status = FetchStatus::Requested;
        seed.requested_at = event.timestamp;
        seed.last_error = None;
    }
    study.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_fetch_started(
    event: Event<FetchStarted>,
    study: &mut StudyState,
) -> anyhow::Result<()> {
    if let Some(seed) = study.seed_mut(&event.data.seed) {
        seed.status = FetchStatus::Fetching;
    }
    study.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_keywords_fetched(
    event: Event<KeywordsFetched>,
    study: &mut StudyState,
) -> anyhow::Result<()> {
    if let Some(seed) = study.seed_mut(&event.data.seed) {
        seed.status = FetchStatus::Fetched;
        seed.fetched_at = Some(event.timestamp);
        seed.keywords = event.data.rows.len() as u32;
        seed.last_error = None;
    }
    study.cost_usd_micros = study
        .cost_usd_micros
        .saturating_add(event.data.cost_usd_micros);
    study.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_fetch_failed(event: Event<FetchFailed>, study: &mut StudyState) -> anyhow::Result<()> {
    if let Some(seed) = study.seed_mut(&event.data.seed) {
        seed.status = FetchStatus::Failed;
        seed.last_error = Some(event.data.reason.to_owned());
    }
    study.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_deleted(event: Event<Deleted>, study: &mut StudyState) -> anyhow::Result<()> {
    study.deleted = true;
    study.updated_at = event.timestamp;
    Ok(())
}

/// What a command made of the study as it stands.
enum Step<T> {
    /// It added its events to the write, which is to commit.
    Write(T),
    /// Nothing to write: the study already is as the command wants it.
    Leave(T),
}

impl<E: Executor> Command<E> {
    /// The study, unless it was deleted.
    pub async fn load_study(&self, study_id: &str) -> anyhow::Result<Option<StudyState>> {
        Ok(study_projection()
            .load(study_id)
            .execute(&self.0)
            .await?
            .filter(|study| !study.deleted))
    }

    /// The study `user_id` owns. Another user's reads as one that does not
    /// exist: a study is nobody else's business.
    pub async fn require_owned(
        &self,
        study_id: &str,
        user_id: &str,
    ) -> Result<StudyState, ResearchError> {
        self.load_study(study_id)
            .await?
            .filter(|study| study.is_owner(user_id))
            .ok_or(ResearchError::NotFound)
    }

    /// Loads the study, lets `decide` add its events to a write of it, and
    /// commits; decided again from a fresh load when another write came in
    /// between. `None` when there is no such study (or it was deleted).
    async fn settle<T>(
        &self,
        study_id: &str,
        routing_key: Option<String>,
        mut decide: impl FnMut(&StudyState, &mut WriteBuilder) -> Result<Step<T>, ResearchError> + Send,
    ) -> Result<Option<T>, ResearchError> {
        for _ in 0..WRITE_ATTEMPTS {
            let Some(study) = self.load_study(study_id).await? else {
                return Ok(None);
            };
            let mut write = study.write()?;
            write.routing_key_opt(routing_key.clone());
            let value = match decide(&study, &mut write)? {
                Step::Leave(value) => return Ok(Some(value)),
                Step::Write(value) => value,
            };
            match write.commit(&self.0).await {
                Ok(_) => return Ok(Some(value)),
                Err(WriteError::InvalidOriginalVersion) => continue,
                Err(err) => return Err(err.into()),
            }
        }
        Err(WriteError::InvalidOriginalVersion.into())
    }

    /// [`settle`](Self::settle) for what the owner does: a study that is
    /// not `actor_user_id`'s is `NotFound`.
    async fn settle_owned<T>(
        &self,
        actor_user_id: &str,
        study_id: &str,
        routing_key: Option<String>,
        mut decide: impl FnMut(&StudyState, &mut WriteBuilder) -> Result<Step<T>, ResearchError> + Send,
    ) -> Result<T, ResearchError> {
        self.settle(study_id, routing_key, |study, write| {
            if !study.is_owner(actor_user_id) {
                return Err(ResearchError::NotFound);
            }
            decide(study, write)
        })
        .await?
        .ok_or(ResearchError::NotFound)
    }
}
