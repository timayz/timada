//! Fetching a seed's keywords: the owner asks, one node takes the fetch on,
//! and what the source gave, or why it gave nothing, is recorded.
//!
//! The call itself is the host's: it costs money, so exactly one node makes
//! it. `FetchStarted` is the claim, written against the version the node
//! read the request at; a node that finds the seed already taken on leaves
//! it. A node that took a fetch on and stopped leaves the seed there until
//! the request is [`FETCH_STALE_SECS`](crate::FETCH_STALE_SECS) old, when
//! the owner may ask again.

use evento::Executor;
use timada_core::unix_now;

use crate::aggregator::{FetchFailed, FetchRequested, FetchStarted, KeywordRow, KeywordsFetched};
use crate::error::ResearchError;
use crate::value_object::{
    FetchStatus, INTENTS, MAX_FETCH_ROWS, MAX_KEYWORD_LEN, MAX_REASON_LEN, normalize_seed,
};

use super::{Command, Step};

/// The rows as an event keeps them: [`MAX_FETCH_ROWS`] at most, each
/// keyword on one line of [`MAX_KEYWORD_LEN`] characters at most, a
/// difficulty within 100, an intent only when it is one this crate knows.
fn kept_rows(rows: Vec<KeywordRow>) -> Vec<KeywordRow> {
    rows.into_iter()
        .filter_map(|row| {
            let keyword: String = row
                .keyword
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .take(MAX_KEYWORD_LEN)
                .collect();
            (!keyword.is_empty()).then(|| KeywordRow {
                keyword,
                volume: row.volume,
                cpc_usd_cents: row.cpc_usd_cents,
                difficulty: row.difficulty.map(|difficulty| difficulty.min(100)),
                intent: row
                    .intent
                    .filter(|intent| INTENTS.contains(&intent.as_str())),
            })
        })
        .take(MAX_FETCH_ROWS)
        .collect()
}

#[evento::command]
impl<E: Executor> Command<E> {
    /// Asks for the keywords of `seed`, or without one of every seed that
    /// still wants its own (never fetched, or whose last fetch failed or
    /// stalled); how many seeds were asked for. A seed being fetched is
    /// `Fetching`, and nothing left to ask for is `NothingToFetch`.
    pub async fn request_fetch(
        &self,
        actor_user_id: &str,
        study_id: &str,
        seed: Option<&str>,
        routing_key: Option<String>,
    ) -> Result<usize, ResearchError> {
        let seed = seed.map(normalize_seed).transpose()?;
        self.settle_owned(actor_user_id, study_id, routing_key, |study, write| {
            let now = unix_now();
            let asked: Vec<&str> = match &seed {
                Some(seed) => {
                    let state = study.seed(seed).ok_or(ResearchError::UnknownSeed)?;
                    if state.is_fetching(now) {
                        return Err(ResearchError::Fetching);
                    }
                    vec![state.seed.as_str()]
                }
                None => study
                    .seeds
                    .iter()
                    .filter(|state| state.is_owed(now))
                    .map(|state| state.seed.as_str())
                    .collect(),
            };
            if asked.is_empty() {
                return Err(ResearchError::NothingToFetch);
            }
            for seed in &asked {
                write.event(&FetchRequested {
                    seed: (*seed).to_owned(),
                    requested_by: actor_user_id.to_owned(),
                });
            }
            Ok(Step::Write(asked.len()))
        })
        .await
    }

    /// Takes the fetch of a requested seed on; `false` when another node
    /// did first, or there is nothing to fetch any more (the study is gone,
    /// the fetch is done).
    pub async fn start_fetch(
        &self,
        actor: &str,
        study_id: &str,
        seed: &str,
        routing_key: Option<String>,
    ) -> Result<bool, ResearchError> {
        let taken = self
            .settle(study_id, routing_key, |study, write| {
                let requested = study
                    .seed(seed)
                    .is_some_and(|state| state.status == FetchStatus::Requested);
                if !requested {
                    return Ok(Step::Leave(false));
                }
                write.event(&FetchStarted {
                    seed: seed.to_owned(),
                    started_by: actor.to_owned(),
                });
                Ok(Step::Write(true))
            })
            .await?;
        Ok(taken.unwrap_or(false))
    }

    /// Records what the source gave for `seed`, which replaces what an
    /// earlier fetch of it gave, and what the call cost; `false` when the
    /// study is gone meanwhile.
    pub async fn record_keywords(
        &self,
        actor: &str,
        study_id: &str,
        seed: &str,
        rows: Vec<KeywordRow>,
        cost_usd_micros: u64,
        routing_key: Option<String>,
    ) -> Result<bool, ResearchError> {
        let rows = kept_rows(rows);
        let recorded = self
            .settle(study_id, routing_key, |study, write| {
                if study.seed(seed).is_none() {
                    return Ok(Step::Leave(false));
                }
                write.event(&KeywordsFetched {
                    seed: seed.to_owned(),
                    rows: rows.clone(),
                    cost_usd_micros,
                    fetched_by: actor.to_owned(),
                });
                Ok(Step::Write(true))
            })
            .await?;
        Ok(recorded.unwrap_or(false))
    }

    /// Records why the fetch of `seed` gave nothing; `false` when no fetch
    /// of it is on its way any more (the study is gone, or another fetch
    /// answered meanwhile).
    pub async fn record_fetch_failure(
        &self,
        actor: &str,
        study_id: &str,
        seed: &str,
        reason: &str,
        routing_key: Option<String>,
    ) -> Result<bool, ResearchError> {
        let reason: String = reason.chars().take(MAX_REASON_LEN).collect();
        let recorded = self
            .settle(study_id, routing_key, |study, write| {
                let pending = study.seed(seed).is_some_and(|state| {
                    matches!(state.status, FetchStatus::Requested | FetchStatus::Fetching)
                });
                if !pending {
                    return Ok(Step::Leave(false));
                }
                write.event(&FetchFailed {
                    seed: seed.to_owned(),
                    reason: reason.clone(),
                    failed_by: actor.to_owned(),
                });
                Ok(Step::Write(true))
            })
            .await?;
        Ok(recorded.unwrap_or(false))
    }
}
