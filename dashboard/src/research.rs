//! Fetches the keywords a study's owner asked for: one paid call to the
//! keyword source per seed, made off the page that asked, and recorded on
//! the study with its rows and what it cost. Every node of the work group
//! runs it; the first to take a fetch on makes the call, the others leave
//! it.
//!
//! A call is the last resort. A seed fetched not long ago, for any study
//! of the same country and language, answers the fetch with the rows it
//! gave then, at no cost. Otherwise room is held for the call in the day's
//! spending (`timada_research`'s `SpendDay`), which every node writes: a
//! call that would pass the user's cap or the platform's is not made, and
//! the fetch fails saying which cap and when it frees.
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
use timada_research::{
    Call, Command, FETCH_STALE_SECS, RESEARCH_ACTOR, Reusable, Room, SpendCap, SpendCaps,
    StudyState, last_fetch,
};

use crate::db::Db;
use crate::keywords::{Keyword, KeywordError, Keywords};
use crate::ui::format_time;

/// The cursor identity of the subscription, in the store.
pub const SUBSCRIPTION: &str = "timada-dashboard/keyword-fetches";

/// A request older than this is history being replayed (a node catching
/// up): left alone, as the owner may ask again from then on.
const REQUEST_WINDOW_SECS: u64 = FETCH_STALE_SECS;

/// What the owner reads when no source is set.
const NO_SOURCE: &str = "Keyword research is not set up on this platform.";

/// What the fetches may spend in a day, and for how long a fetch answers
/// the next one of its seed (the configuration's `[research]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Spending {
    pub caps: SpendCaps,
    /// 0 pays for every fetch.
    pub reuse_secs: u64,
}

impl Spending {
    pub fn new(config: &crate::config::ResearchConfig) -> Self {
        Self {
            caps: config.caps(),
            reuse_secs: config.reuse_secs(),
        }
    }
}

/// The store, to write with.
#[derive(Clone)]
struct Store(Evento);

pub fn subscription<E: Executor>(
    keywords: Keywords,
    store: Evento,
    db: Db,
    spending: Spending,
) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(SUBSCRIPTION)
        .data(keywords)
        .data(Store(store))
        .data(db)
        .data(spending)
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
    let db: Db = context.extract();
    let spending: Spending = context.extract();
    let now = crate::auth::now();
    if is_history(now, event.timestamp) {
        return Ok(());
    }
    let studies = Command(store.0);
    let study_id = event.aggregate_id.as_str();
    let seed = event.data.seed.as_str();
    let Some(study) = studies.load_study(study_id).await? else {
        return Ok(());
    };
    if !studies.start_fetch(RESEARCH_ACTOR, study_id, seed).await? {
        return Ok(());
    }
    if let Some((from_study, reusable)) =
        reusable(&studies, &db, &study, seed, spending, now).await?
    {
        let found = reusable.rows.len();
        studies
            .record_reuse(RESEARCH_ACTOR, study_id, seed, reusable, &from_study)
            .await?;
        tracing::info!(study = %study_id, %seed, from = %from_study, keywords = found, "keywords reused");
        return Ok(());
    }
    let Some(source) = &keywords.0 else {
        studies
            .record_fetch_failure(RESEARCH_ACTOR, study_id, seed, NO_SOURCE)
            .await?;
        return Ok(());
    };
    // The call is paid by the platform and counted to who asked for it.
    let room = studies
        .hold_spend(
            RESEARCH_ACTOR,
            Call {
                user_id: &event.data.requested_by,
                study_id,
                seed,
                most_usd_micros: source.most_usd_micros(),
            },
            spending.caps,
        )
        .await?;
    let hold = match room {
        Room::Held(hold) => hold,
        Room::Refused { cap, frees_at } => {
            tracing::info!(study = %study_id, %seed, user = %event.data.requested_by, ?cap, "keywords not fetched: the day's cap is reached");
            studies
                .record_fetch_failure(RESEARCH_ACTOR, study_id, seed, &cap_reached(cap, frees_at))
                .await?;
            return Ok(());
        }
    };
    match source
        .suggestions(seed, &study.country, &study.language)
        .await
    {
        Ok(suggestions) => {
            // The charge first: what was paid counts whatever becomes of
            // the rows.
            studies
                .charge_spend(RESEARCH_ACTOR, &hold, suggestions.cost_usd_micros)
                .await?;
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
        Err(err) => {
            let reason = reason_of(study_id, seed, &err);
            studies.release_spend(RESEARCH_ACTOR, &hold).await?;
            studies
                .record_fetch_failure(RESEARCH_ACTOR, study_id, seed, &reason)
                .await?;
        }
    }
    Ok(())
}

/// The rows a fetch of `seed` made within the reuse window gave, for the
/// study's country and language, and the study that holds them. The index
/// of the fetches is this node's read model, a moment behind: a fetch it
/// does not know yet is paid for again.
async fn reusable(
    studies: &Command<Evento>,
    db: &Db,
    study: &StudyState,
    seed: &str,
    spending: Spending,
    now: u64,
) -> anyhow::Result<Option<(String, Reusable)>> {
    if spending.reuse_secs == 0 {
        return Ok(None);
    }
    let since = now.saturating_sub(spending.reuse_secs);
    let Some(found) = last_fetch(db, &study.country, &study.language, seed, since).await? else {
        return Ok(None);
    };
    let rows = studies.fetched_rows(&found.study_id, seed, since).await?;
    Ok(rows.map(|rows| (found.study_id, rows)))
}

/// What the owner reads when a fetch would pass `cap`, which frees at
/// `frees_at` (unix seconds).
pub fn cap_reached(cap: SpendCap, frees_at: u64) -> String {
    let who = match cap {
        SpendCap::User => "You have",
        SpendCap::Platform => "The platform has",
    };
    format!(
        "{who} reached the most keyword fetches may cost in a day. It frees on {}.",
        format_time(frees_at)
    )
}

/// An amount in millionths of a US dollar as `$0.13`; to the hundredth of
/// a cent (`$0.0126`) when it is not a whole number of cents.
pub fn format_usd(usd_micros: u64) -> String {
    if usd_micros.is_multiple_of(10_000) {
        format!("${}", timada_core::format_amount(usd_micros / 10_000))
    } else {
        let hundredths = (usd_micros + 50) / 100;
        format!("${}.{:04}", hundredths / 10_000, hundredths % 10_000)
    }
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
    use timada_research::{CreateStudy, FetchStatus, spend_day};

    use super::*;
    use crate::keywords::Intent;
    use crate::keywords::fake::{Asked, FakeKeywords, keyword};

    const ALICE: &str = "user-alice";

    const BOB: &str = "user-bob";

    /// Caps far above what a test spends, and no reuse: every fetch is a
    /// call.
    const OPEN: Spending = Spending {
        caps: SpendCaps {
            user_usd_micros: 100_000_000,
            platform_usd_micros: 100_000_000,
        },
        reuse_secs: 0,
    };

    thread_local! {
        /// The read models of the test's store: a test runs on a thread of
        /// its own and opens one store.
        static DB: std::cell::RefCell<Option<Db>> = const { std::cell::RefCell::new(None) };
    }

    /// The store, with the read models' tables in it.
    async fn store() -> Evento {
        let turso = Turso::in_memory_concurrent().await.unwrap();
        let mut conn = turso.connect().unwrap();
        timada_research::MIGRATIONS
            .to_latest(&mut conn)
            .await
            .map_err(|(_, err)| err)
            .unwrap();
        DB.set(Some(Db::from(turso.clone())));
        Evento::new(turso)
    }

    fn db() -> Db {
        DB.with_borrow(|db| db.clone().expect("store() first"))
    }

    async fn study(store: &Evento, seeds: &[&str]) -> StudyState {
        study_of(store, ALICE, seeds).await
    }

    async fn study_of(store: &Evento, owner: &str, seeds: &[&str]) -> StudyState {
        Command(store.clone())
            .create_study(
                owner,
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
        pass_within(store, source, OPEN).await;
    }

    /// One pass of the saga under `spending`, then of the read models it
    /// looks a fetch up in.
    async fn pass_within(store: &Evento, source: Option<&Arc<FakeKeywords>>, spending: Spending) {
        let keywords = Keywords(source.map(|source| source.clone() as _));
        subscription::<Evento>(keywords, store.clone(), db(), spending)
            .retry(0)
            .run_once(store)
            .await
            .unwrap();
        timada_research::read_model_subscription::<Evento>(db())
            .run_once(store)
            .await
            .unwrap();
    }

    /// What the day's spending holds for `user_id`: what was charged, and
    /// how many holds are still open over every user.
    async fn spent(store: &Evento, user_id: &str) -> (u64, usize) {
        let day = Command(store.clone())
            .load_spend_day(&spend_day(crate::auth::now()))
            .await
            .unwrap();
        (day.charged_to(user_id), day.holds.len())
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

    /// What the calls cost is counted to who asked; a call the source
    /// failed costs nothing and gives its room back.
    #[tokio::test]
    async fn a_call_is_charged_to_who_asked_for_it() {
        let store = store().await;
        let source = FakeKeywords::new();
        source.give("griffoir", vec![keyword("griffoir", 12_100, 42, 38)]);
        let study = study(&store, &["griffoir", "litière"]).await;
        let studies = Command(store.clone());
        studies.request_fetch(ALICE, &study.id, None).await.unwrap();
        pass(&store, Some(&source)).await;
        assert_eq!(spent(&store, ALICE).await, (12_120 + 12_000, 0));

        source.fail(Some(KeywordError::Unavailable("timed out".to_owned())));
        studies
            .request_fetch(ALICE, &study.id, Some("griffoir"))
            .await
            .unwrap();
        pass(&store, Some(&source)).await;
        assert_eq!(spent(&store, ALICE).await, (24_120, 0));
        assert_eq!(spent(&store, BOB).await, (0, 0));
    }

    /// A fetch that would pass the user's cap is refused before the call,
    /// and says which cap and when it frees; another user still has theirs.
    #[tokio::test]
    async fn a_fetch_past_the_users_cap_is_refused_before_the_call() {
        let store = store().await;
        let source = FakeKeywords::new();
        // Room for one call at its most (132,000), and after it (12,000
        // charged) for one more.
        let capped = Spending {
            caps: SpendCaps {
                user_usd_micros: 150_000,
                platform_usd_micros: 100_000_000,
            },
            reuse_secs: 0,
        };
        let study = study(&store, &["griffoir", "litière", "arbre à chat"]).await;
        let studies = Command(store.clone());
        studies.request_fetch(ALICE, &study.id, None).await.unwrap();
        pass_within(&store, Some(&source), capped).await;

        let after = reload(&store, &study).await;
        let statuses: Vec<_> = after.seeds.iter().map(|seed| seed.status).collect();
        assert_eq!(
            statuses,
            [
                FetchStatus::Fetched,
                FetchStatus::Fetched,
                FetchStatus::Failed
            ]
        );
        assert_eq!(source.asked().len(), 2, "the third call was never made");
        let reason = after.seeds[2].last_error.clone().unwrap();
        let frees = format_time(timada_research::spend_day_ends(crate::auth::now()));
        assert_eq!(
            reason,
            format!(
                "You have reached the most keyword fetches may cost in a day. It frees on {frees}."
            )
        );
        assert!(frees.ends_with("00:00:00 UTC"), "{frees}");
        assert_eq!(spent(&store, ALICE).await, (24_000, 0));

        let theirs = study_of(&store, BOB, &["griffoir"]).await;
        studies.request_fetch(BOB, &theirs.id, None).await.unwrap();
        pass_within(&store, Some(&source), capped).await;
        assert_eq!(
            reload(&store, &theirs).await.seeds[0].status,
            FetchStatus::Fetched
        );
        assert_eq!(source.asked().len(), 3);
    }

    /// The platform's cap is over every user's calls together.
    #[tokio::test]
    async fn a_fetch_past_the_platforms_cap_is_refused_for_everyone() {
        let store = store().await;
        let source = FakeKeywords::new();
        let capped = Spending {
            caps: SpendCaps {
                user_usd_micros: 100_000_000,
                platform_usd_micros: 140_000,
            },
            reuse_secs: 0,
        };
        let studies = Command(store.clone());
        let mine = study(&store, &["griffoir"]).await;
        studies.request_fetch(ALICE, &mine.id, None).await.unwrap();
        pass_within(&store, Some(&source), capped).await;
        let theirs = study_of(&store, BOB, &["litière"]).await;
        studies.request_fetch(BOB, &theirs.id, None).await.unwrap();
        pass_within(&store, Some(&source), capped).await;

        assert_eq!(source.asked().len(), 1);
        let refused = reload(&store, &theirs).await;
        assert_eq!(refused.seeds[0].status, FetchStatus::Failed);
        assert!(
            refused.seeds[0].last_error.as_deref().unwrap().starts_with(
                "The platform has reached the most keyword fetches may cost in a day."
            )
        );
        assert_eq!(spent(&store, BOB).await, (0, 0));
    }

    /// A seed fetched within the reuse window answers whoever asks for it
    /// again in the same country and language: no call, no cost, even past
    /// a cap.
    #[tokio::test]
    async fn a_seed_fetched_not_long_ago_is_answered_without_a_call() {
        let store = store().await;
        let source = FakeKeywords::new();
        source.give("griffoir", vec![keyword("griffoir", 12_100, 42, 38)]);
        let reusing = Spending {
            caps: SpendCaps {
                user_usd_micros: 140_000,
                platform_usd_micros: 140_000,
            },
            reuse_secs: 30 * 86_400,
        };
        let studies = Command(store.clone());
        let mine = study(&store, &["griffoir"]).await;
        studies.request_fetch(ALICE, &mine.id, None).await.unwrap();
        pass_within(&store, Some(&source), reusing).await;
        assert_eq!(source.asked().len(), 1);

        // Bob asks for the same seed; the platform's cap has no room left.
        let theirs = study_of(&store, BOB, &["griffoir", "litière"]).await;
        studies.request_fetch(BOB, &theirs.id, None).await.unwrap();
        pass_within(&store, Some(&source), reusing).await;
        let after = reload(&store, &theirs).await;
        assert_eq!(after.seeds[0].status, FetchStatus::Fetched);
        assert_eq!(after.seeds[0].keywords, 1);
        assert_eq!(
            after.seeds[0].fetched_at,
            reload(&store, &mine).await.seeds[0].fetched_at
        );
        assert_eq!(after.cost_usd_micros, 0);
        assert_eq!(after.seeds[1].status, FetchStatus::Failed);
        assert_eq!(source.asked().len(), 1, "no call for either seed");
        assert_eq!(spent(&store, BOB).await, (0, 0));

        // The same seed searched somewhere else is another question.
        let elsewhere = Command(store.clone())
            .create_study(
                ALICE,
                CreateStudy {
                    name: "Scratchers".to_owned(),
                    seeds: vec!["griffoir".to_owned()],
                    country: "US".to_owned(),
                    language: "en".to_owned(),
                },
            )
            .await
            .unwrap();
        studies
            .request_fetch(ALICE, &elsewhere.id, None)
            .await
            .unwrap();
        pass_within(&store, Some(&source), reusing).await;
        assert_eq!(
            reload(&store, &elsewhere).await.seeds[0].status,
            FetchStatus::Failed,
            "asked of the caps, which have no room"
        );
    }

    #[test]
    fn an_amount_reads_in_dollars() {
        assert_eq!(format_usd(0), "$0.00");
        assert_eq!(format_usd(1_000_000), "$1.00");
        assert_eq!(format_usd(160_000), "$0.16");
        assert_eq!(format_usd(12_600), "$0.0126");
        assert_eq!(format_usd(12_345_678), "$12.3457");
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
