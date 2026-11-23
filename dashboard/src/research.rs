//! Fetches the keywords a study's owner asked for: one paid call to the
//! keyword source per seed, made off the page that asked, and recorded on
//! the study with its rows and what it cost. Every node of the work group
//! runs it; the first to take a fetch on makes the call, the others leave
//! it.
//!
//! A failure of the source is the fetch's outcome, not the pass's: it is
//! recorded on the study with its reason, where the owner reads it and asks
//! again. Only the store failing fails the pass. A node that stops between
//! taking a fetch on and recording it leaves the seed there, and the owner
//! may ask again once the request is ten minutes old.

use evento::{
    Evento, Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use timada_research::aggregator::{FetchRequested, KeywordRow};
use timada_research::{FETCH_STALE_SECS, RESEARCH_ACTOR};

use crate::keywords::{Keyword, KeywordError, Keywords};

/// The cursor identity of the subscription, in the store.
pub const SUBSCRIPTION: &str = "timada-dashboard/keyword-fetches";

/// A request older than this is history being replayed (a node catching
/// up): left alone, as the owner may ask again from then on.
const REQUEST_WINDOW_SECS: u64 = FETCH_STALE_SECS;

/// What the owner reads when no source is set.
const NO_SOURCE: &str = "Keyword research is not set up on this platform.";

/// The store, to write with.
#[derive(Clone)]
struct Store(Evento);

pub fn subscription<E: Executor>(keywords: Keywords, store: Evento) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(SUBSCRIPTION)
        .data(keywords)
        .data(Store(store))
        .handler(on_fetch_requested())
        // A saga: it follows the node's own work group (the executor's default
        // routing key), so a work group's nodes do its work and the other
        // work groups' nodes leave it to them. The studies live in the
        // platform's tenant.
        .tenant("")
        .retry(5)
}

#[evento::subscription]
async fn on_fetch_requested<E: Executor>(
    context: &Context<'_, E>,
    event: Event<FetchRequested>,
) -> anyhow::Result<()> {
    let keywords: Keywords = context.extract();
    let store: Store = context.extract();
    if is_history(crate::auth::now(), event.timestamp) {
        return Ok(());
    }
    let studies = timada_research::Command(store.0);
    let study_id = event.aggregate_id.as_str();
    let seed = event.data.seed.as_str();
    let Some(study) = studies.load_study(study_id).await? else {
        return Ok(());
    };
    if !studies.start_fetch(RESEARCH_ACTOR, study_id, seed).await? {
        return Ok(());
    }
    let outcome = match &keywords.0 {
        Some(source) => source
            .suggestions(seed, &study.country, &study.language)
            .await
            .map_err(|err| reason_of(study_id, seed, &err)),
        None => Err(NO_SOURCE.to_owned()),
    };
    match outcome {
        Ok(suggestions) => {
            let found = suggestions.rows.len();
            let rows = suggestions.rows.into_iter().map(row_of).collect();
            studies
                .record_keywords(
                    RESEARCH_ACTOR,
                    study_id,
                    seed,
                    rows,
                    suggestions.cost_usd_micros,
                )
                .await?;
            tracing::info!(study = %study_id, %seed, keywords = found, cost_usd_micros = suggestions.cost_usd_micros, "keywords fetched");
        }
        Err(reason) => {
            studies
                .record_fetch_failure(RESEARCH_ACTOR, study_id, seed, &reason)
                .await?;
        }
    }
    Ok(())
}

/// Whether a request made at `requested_at` is history at `now` (both unix
/// seconds).
fn is_history(now: u64, requested_at: u64) -> bool {
    now.saturating_sub(requested_at) > REQUEST_WINDOW_SECS
}

/// What the owner reads about why a fetch gave nothing. A refusal is kept
/// in the source's words: it says what to put right (the balance, the
/// country). Why the source could not answer is the operator's to read.
fn reason_of(study_id: &str, seed: &str, err: &KeywordError) -> String {
    tracing::warn!(study = %study_id, %seed, error = %err, "keywords not fetched");
    match err {
        KeywordError::Unavailable(_) => {
            "The keyword source did not answer; try again in a moment.".to_owned()
        }
        KeywordError::Refused(reason) => format!("The keyword source refused: {reason}"),
    }
}

/// A keyword of the source as a study keeps it.
fn row_of(keyword: Keyword) -> KeywordRow {
    KeywordRow {
        keyword: keyword.keyword,
        volume: keyword.volume,
        cpc_usd_cents: keyword.cpc_usd_cents,
        difficulty: keyword.difficulty,
        intent: keyword.intent.map(|intent| intent.as_str().to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use evento::Turso;
    use timada_research::{Command, CreateStudy, FetchStatus, StudyState};

    use super::*;
    use crate::keywords::Intent;
    use crate::keywords::fake::{Asked, FakeKeywords, keyword};

    const ALICE: &str = "user-alice";

    async fn store() -> Evento {
        Evento::new(Turso::in_memory_concurrent().await.unwrap())
    }

    async fn study(store: &Evento, seeds: &[&str]) -> StudyState {
        Command(store.clone())
            .create_study(
                ALICE,
                CreateStudy {
                    name: "Arbres à chat".to_owned(),
                    seeds: seeds.iter().map(|seed| seed.to_string()).collect(),
                    country: "FR".to_owned(),
                    language: "fr".to_owned(),
                },
            )
            .await
            .unwrap()
    }

    /// One pass of the saga over what the store holds for it.
    async fn pass(store: &Evento, source: Option<&Arc<FakeKeywords>>) {
        let keywords = Keywords(source.map(|source| source.clone() as _));
        subscription::<Evento>(keywords, store.clone())
            .retry(0)
            .run_once(store)
            .await
            .unwrap();
    }

    async fn reload(store: &Evento, study: &StudyState) -> StudyState {
        Command(store.clone())
            .load_study(&study.id)
            .await
            .unwrap()
            .unwrap()
    }

    /// The rows the last fetch of each seed recorded, by seed.
    async fn recorded(store: &Evento, study: &StudyState) -> Vec<(String, Vec<KeywordRow>, u64)> {
        evento::read::<timada_research::aggregator::Study>(&study.id)
            .decode(store)
            .await
            .unwrap()
            .into_iter()
            .filter_map(|event| match event {
                timada_research::aggregator::StudyEvent::KeywordsFetched(fetched) => {
                    Some((fetched.seed, fetched.rows, fetched.cost_usd_micros))
                }
                _ => None,
            })
            .collect()
    }

    /// The crate that keeps the rows knows the intents the source names.
    #[test]
    fn a_study_keeps_every_intent_the_source_names() {
        for intent in timada_research::INTENTS {
            assert_eq!(Intent::parse(intent).map(Intent::as_str), Some(intent));
        }
    }

    /// A request is answered off the page: the source is asked once per
    /// seed, for the study's country and language, and its rows and their
    /// cost land on the study.
    #[tokio::test]
    async fn the_keywords_asked_for_are_fetched_and_recorded() {
        let store = store().await;
        let source = FakeKeywords::new();
        source.give(
            "griffoir",
            vec![
                keyword("griffoir", 12_100, 42, 38),
                Keyword {
                    keyword: "griffoir diy".to_owned(),
                    volume: Some(320),
                    cpc_usd_cents: None,
                    difficulty: None,
                    intent: Some(Intent::Informational),
                },
            ],
        );
        let study = study(&store, &["griffoir", "litière"]).await;
        Command(store.clone())
            .request_fetch(ALICE, &study.id, None)
            .await
            .unwrap();
        pass(&store, Some(&source)).await;

        let asked = |seed: &str| Asked {
            seed: seed.to_owned(),
            country: "FR".to_owned(),
            language: "fr".to_owned(),
        };
        assert_eq!(source.asked(), [asked("griffoir"), asked("litière")]);
        let after = reload(&store, &study).await;
        assert!(
            after
                .seeds
                .iter()
                .all(|seed| seed.status == FetchStatus::Fetched)
        );
        assert_eq!(after.seeds[0].keywords, 2);
        // A seed nobody searches gives no rows, and still cost its call.
        assert_eq!(after.seeds[1].keywords, 0);
        assert_eq!(after.cost_usd_micros, 12_240 + 12_000);
        let fetches = recorded(&store, &study).await;
        assert_eq!(fetches.len(), 2);
        let (seed, rows, cost) = &fetches[0];
        assert_eq!((seed.as_str(), *cost), ("griffoir", 12_240));
        assert_eq!(rows[0].intent.as_deref(), Some("commercial"));
        assert_eq!(
            rows[1],
            KeywordRow {
                keyword: "griffoir diy".to_owned(),
                volume: Some(320),
                cpc_usd_cents: None,
                difficulty: None,
                intent: Some("informational".to_owned()),
            }
        );
    }

    /// The source failing is the fetch's outcome: the pass goes on, the
    /// study says why, and the owner asks again.
    #[tokio::test]
    async fn a_failure_of_the_source_is_recorded_and_asked_again() {
        let store = store().await;
        let source = FakeKeywords::new();
        source.give("griffoir", vec![keyword("griffoir", 12_100, 42, 38)]);
        let study = study(&store, &["griffoir"]).await;
        let studies = Command(store.clone());

        source.fail(Some(KeywordError::Refused(
            "Insufficient Funds.".to_owned(),
        )));
        studies.request_fetch(ALICE, &study.id, None).await.unwrap();
        pass(&store, Some(&source)).await;
        let refused = reload(&store, &study).await;
        assert_eq!(refused.seeds[0].status, FetchStatus::Failed);
        assert_eq!(
            refused.seeds[0].last_error.as_deref(),
            Some("The keyword source refused: Insufficient Funds.")
        );

        // Why it could not answer is not the owner's to read.
        source.fail(Some(KeywordError::Unavailable(
            "connection reset by 10.0.0.7".to_owned(),
        )));
        studies.request_fetch(ALICE, &study.id, None).await.unwrap();
        pass(&store, Some(&source)).await;
        assert_eq!(
            reload(&store, &study).await.seeds[0].last_error.as_deref(),
            Some("The keyword source did not answer; try again in a moment.")
        );

        source.fail(None);
        studies
            .request_fetch(ALICE, &study.id, Some("griffoir"))
            .await
            .unwrap();
        pass(&store, Some(&source)).await;
        let fetched = reload(&store, &study).await;
        assert_eq!(fetched.seeds[0].status, FetchStatus::Fetched);
        assert_eq!(fetched.seeds[0].last_error, None);
        assert_eq!(source.asked().len(), 3);
    }

    /// Without a source the request is answered all the same, with why.
    #[tokio::test]
    async fn a_request_without_a_source_says_research_is_off() {
        let store = store().await;
        let study = study(&store, &["griffoir"]).await;
        Command(store.clone())
            .request_fetch(ALICE, &study.id, None)
            .await
            .unwrap();
        pass(&store, None).await;
        let after = reload(&store, &study).await;
        assert_eq!(after.seeds[0].status, FetchStatus::Failed);
        assert_eq!(after.seeds[0].last_error.as_deref(), Some(NO_SOURCE));
    }

    /// A fetch another node took on, a study deleted since, and a request
    /// forged in a shop's tenant cost nothing.
    #[tokio::test]
    async fn a_request_nobody_owes_an_answer_to_is_left_alone() {
        let store = store().await;
        let source = FakeKeywords::new();
        let studies = Command(store.clone());

        let taken = study(&store, &["griffoir"]).await;
        studies.request_fetch(ALICE, &taken.id, None).await.unwrap();
        studies
            .start_fetch("another node", &taken.id, "griffoir")
            .await
            .unwrap();

        let gone = study(&store, &["litière"]).await;
        studies.request_fetch(ALICE, &gone.id, None).await.unwrap();
        studies.delete_study(ALICE, &gone.id).await.unwrap();

        evento::append("forged-study")
            .original_version(0)
            .event(&timada_research::aggregator::Created {
                name: "Forged".to_owned(),
                country: "FR".to_owned(),
                language: "fr".to_owned(),
                created_by: ALICE.to_owned(),
            })
            .event(&timada_research::aggregator::SeedAdded {
                seed: "forged".to_owned(),
                added_by: ALICE.to_owned(),
            })
            .event(&FetchRequested {
                seed: "forged".to_owned(),
                requested_by: ALICE.to_owned(),
            })
            .tenant("zeta")
            .commit(&store)
            .await
            .unwrap();

        pass(&store, Some(&source)).await;
        assert!(source.asked().is_empty());
        assert_eq!(
            reload(&store, &taken).await.seeds[0].status,
            FetchStatus::Fetching
        );
    }

    /// A request read long after it was made is history: the owner may ask
    /// again from then on, and nothing is paid for it twice.
    #[test]
    fn a_request_too_old_is_history() {
        let at = 1_000_000;
        assert!(!is_history(at, at));
        assert!(!is_history(at + FETCH_STALE_SECS, at));
        assert!(is_history(at + FETCH_STALE_SECS + 1, at));
        // A clock a little behind the event's is not a reason to skip it.
        assert!(!is_history(at - 5, at));
    }
}
