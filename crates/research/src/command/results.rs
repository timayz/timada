//! Who holds the first results of a study's main keywords: the owner asks,
//! one node takes each search on, and what the source gave, or why it gave
//! nothing, is recorded.
//!
//! It goes as a seed's fetch does (see [`fetch`](super::fetch)): a search
//! costs money, so `ResultsStarted` is the claim that lets one node make
//! it, and a search whose node stopped may be asked again once its request
//! is [`FETCH_STALE_SECS`](crate::FETCH_STALE_SECS) old. Which keywords are
//! main is the score's to say, and the score is the read model's: the
//! caller names them, and this side keeps to the bound on how many.

use evento::Executor;
use timada_core::unix_now;

use crate::aggregator::{
    ResultRow, ResultsFailed, ResultsFetched, ResultsRequested, ResultsStarted,
};
use crate::error::ResearchError;
use crate::value_object::{
    FetchStatus, MAX_CHECKED_KEYWORDS, MAX_REASON_LEN, MAX_RESULTS, MAX_STUDY_EVENTS,
    MAX_TITLE_LEN, MAX_URL_LEN, normalize_domain, normalize_keyword,
};

use super::{Command, Step};

/// The results as an event keeps them: [`MAX_RESULTS`] at most, in their
/// order, each with its site as [`normalize_domain`] names it. A result
/// whose site does not read as one is passed over.
fn kept_results(results: Vec<ResultRow>) -> Vec<ResultRow> {
    let cut = |text: &str, len: usize| -> String {
        text.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(len)
            .collect()
    };
    results
        .into_iter()
        .filter_map(|result| {
            Some(ResultRow {
                position: result.position,
                domain: normalize_domain(&result.domain).ok()?,
                url: cut(&result.url, MAX_URL_LEN),
                title: cut(&result.title, MAX_TITLE_LEN),
            })
        })
        .take(MAX_RESULTS)
        .collect()
}

#[evento::command]
impl<E: Executor> Command<E> {
    /// Asks for the first results of `keywords`, the study's main ones as
    /// the caller read them: of those that still want theirs (never
    /// searched, or whose last search failed or stalled), or with `again`
    /// of every one that is not being searched. How many were asked for;
    /// none left to ask for is `NothingToCheck`.
    pub async fn request_results(
        &self,
        actor_user_id: &str,
        study_id: &str,
        keywords: &[String],
        again: bool,
        routing_key: Option<String>,
    ) -> Result<usize, ResearchError> {
        let mut named: Vec<String> = Vec::with_capacity(keywords.len());
        for keyword in keywords {
            let keyword = normalize_keyword(keyword)?;
            if !named.contains(&keyword) {
                named.push(keyword);
            }
        }
        if named.is_empty() {
            return Err(ResearchError::NoMainKeyword);
        }
        if named.len() > MAX_CHECKED_KEYWORDS {
            return Err(ResearchError::TooManyChecked);
        }
        self.settle_owned(actor_user_id, study_id, routing_key, |study, write| {
            let now = unix_now();
            let asked: Vec<&String> = named
                .iter()
                .filter(|keyword| match study.check(keyword) {
                    None => true,
                    Some(check) if again => !check.is_fetching(now),
                    Some(check) => check.is_owed(now),
                })
                .collect();
            if asked.is_empty() {
                return Err(ResearchError::NothingToCheck);
            }
            // Each search writes three events: asked for, taken on, answered.
            let room = usize::from(MAX_STUDY_EVENTS.saturating_sub(study.aggregate_version));
            if asked.len() * 3 > room {
                return Err(ResearchError::TooManyChanges);
            }
            for keyword in &asked {
                write.event(&ResultsRequested {
                    keyword: (*keyword).clone(),
                    requested_by: actor_user_id.to_owned(),
                });
            }
            Ok(Step::Write(asked.len()))
        })
        .await
    }

    /// Takes the search for a keyword's first results on; `false` when
    /// another node did first, or there is nothing to search any more (the
    /// study is gone, the search is done).
    pub async fn start_results(
        &self,
        actor: &str,
        study_id: &str,
        keyword: &str,
        routing_key: Option<String>,
    ) -> Result<bool, ResearchError> {
        let taken = self
            .settle(study_id, routing_key, |study, write| {
                let requested = study
                    .check(keyword)
                    .is_some_and(|check| check.status == FetchStatus::Requested);
                if !requested {
                    return Ok(Step::Leave(false));
                }
                write.event(&ResultsStarted {
                    keyword: keyword.to_owned(),
                    started_by: actor.to_owned(),
                });
                Ok(Step::Write(true))
            })
            .await?;
        Ok(taken.unwrap_or(false))
    }

    /// Records the first results of a search for `keyword`, which replace
    /// what an earlier search of it gave, and what the call cost; `false`
    /// when the study is gone meanwhile.
    pub async fn record_results(
        &self,
        actor: &str,
        study_id: &str,
        keyword: &str,
        results: Vec<ResultRow>,
        cost_usd_micros: u64,
        routing_key: Option<String>,
    ) -> Result<bool, ResearchError> {
        let results = kept_results(results);
        let recorded = self
            .settle(study_id, routing_key, |study, write| {
                if study.check(keyword).is_none() {
                    return Ok(Step::Leave(false));
                }
                write.event(&ResultsFetched {
                    keyword: keyword.to_owned(),
                    results: results.clone(),
                    cost_usd_micros,
                    fetched_by: actor.to_owned(),
                });
                Ok(Step::Write(true))
            })
            .await?;
        Ok(recorded.unwrap_or(false))
    }

    /// Records why the search for `keyword` gave nothing; `false` when no
    /// search of it is on its way any more (the study is gone, or another
    /// search answered meanwhile).
    pub async fn record_results_failure(
        &self,
        actor: &str,
        study_id: &str,
        keyword: &str,
        reason: &str,
        routing_key: Option<String>,
    ) -> Result<bool, ResearchError> {
        let reason: String = reason.chars().take(MAX_REASON_LEN).collect();
        let recorded = self
            .settle(study_id, routing_key, |study, write| {
                let pending = study.check(keyword).is_some_and(|check| {
                    matches!(check.status, FetchStatus::Requested | FetchStatus::Fetching)
                });
                if !pending {
                    return Ok(Step::Leave(false));
                }
                write.event(&ResultsFailed {
                    keyword: keyword.to_owned(),
                    reason: reason.clone(),
                    failed_by: actor.to_owned(),
                });
                Ok(Step::Write(true))
            })
            .await?;
        Ok(recorded.unwrap_or(false))
    }
}
