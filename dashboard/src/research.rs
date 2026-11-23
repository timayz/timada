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
//! Who holds the first results of a study's main keywords is fetched the
//! same way by a second subscription ([`results_subscription`]): one paid
//! search per keyword, inside the same caps. It is apart from the first so
//! that the searches of one study, some tens of them a few seconds each, do
//! not keep another study's keywords waiting.
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
use timada_research::aggregator::{FetchRequested, KeywordRow, ResultRow, ResultsRequested};
use timada_research::{
    Call, Command, FETCH_STALE_SECS, RESEARCH_ACTOR, Reusable, Room, SpendCap, SpendCaps,
    StudyState, last_fetch,
};

use crate::db::Db;
use crate::keywords::{Keyword, KeywordError, Keywords, SearchResult};
use crate::ui::format_time;

/// The cursor identity of the subscription, in the store.
pub const SUBSCRIPTION: &str = "timada-dashboard/keyword-fetches";

/// The cursor identity of the subscription that searches, in the store.
pub const RESULTS_SUBSCRIPTION: &str = "timada-dashboard/first-results";

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
                query: seed,
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

pub fn results_subscription<E: Executor>(
    keywords: Keywords,
    store: Evento,
    spending: Spending,
) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(RESULTS_SUBSCRIPTION)
        .data(keywords)
        .data(Store(store))
        .data(spending)
        .handler(on_results_requested())
        // A saga, as the fetches are: it follows the node's own work group.
        .tenant("")
        .retry(5)
}

#[evento::subscription]
async fn on_results_requested<E: Executor>(
    context: &Context<'_, E>,
    event: Event<ResultsRequested>,
) -> anyhow::Result<()> {
    let keywords: Keywords = context.extract();
    let store: Store = context.extract();
    let spending: Spending = context.extract();
    if is_history(crate::auth::now(), event.timestamp) {
        return Ok(());
    }
    let studies = Command(store.0);
    let study_id = event.aggregate_id.as_str();
    let keyword = event.data.keyword.as_str();
    let Some(study) = studies.load_study(study_id).await? else {
        return Ok(());
    };
    if !studies
        .start_results(RESEARCH_ACTOR, study_id, keyword)
        .await?
    {
        return Ok(());
    }
    let Some(source) = &keywords.0 else {
        studies
            .record_results_failure(RESEARCH_ACTOR, study_id, keyword, NO_SOURCE)
            .await?;
        return Ok(());
    };
    let room = studies
        .hold_spend(
            RESEARCH_ACTOR,
            Call {
                user_id: &event.data.requested_by,
                study_id,
                query: keyword,
                most_usd_micros: source.most_results_usd_micros(),
            },
            spending.caps,
        )
        .await?;
    let hold = match room {
        Room::Held(hold) => hold,
        Room::Refused { cap, frees_at } => {
            tracing::info!(study = %study_id, %keyword, user = %event.data.requested_by, ?cap, "first results not fetched: the day's cap is reached");
            studies
                .record_results_failure(
                    RESEARCH_ACTOR,
                    study_id,
                    keyword,
                    &cap_reached(cap, frees_at),
                )
                .await?;
            return Ok(());
        }
    };
    match source
        .first_results(keyword, &study.country, &study.language)
        .await
    {
        Ok(found) => {
            // The charge first: what was paid counts whatever becomes of
            // the results.
            studies
                .charge_spend(RESEARCH_ACTOR, &hold, found.cost_usd_micros)
                .await?;
            let results = found.results.len();
            studies
                .record_results(
                    RESEARCH_ACTOR,
                    study_id,
                    keyword,
                    found.results.into_iter().map(result_of).collect(),
                    found.cost_usd_micros,
                )
                .await?;
            tracing::info!(study = %study_id, %keyword, results, cost_usd_micros = found.cost_usd_micros, "first results fetched");
        }
        Err(err) => {
            let reason = reason_of(study_id, keyword, &err);
            studies.release_spend(RESEARCH_ACTOR, &hold).await?;
            studies
                .record_results_failure(RESEARCH_ACTOR, study_id, keyword, &reason)
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
        "{who} reached the most keyword research may cost in a day. It frees on {}.",
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

/// How long a save waits for the euro rates before going on without one.
const RATE_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// What one unit of `currency` is worth on `day` (`YYYY-MM-DD`) in
/// millionths of a US dollar, by the euro reference rates: what a study
/// reads a cost per click with. `None` for the dollar itself, which needs
/// none, and when the rates do not say.
pub async fn usd_worth(
    rates: &dyn crate::ecb::EuroRates,
    currency: &str,
    day: &str,
) -> Option<u64> {
    use crate::ecb::RateAnswer;
    if currency == timada_research::CPC_CURRENCY {
        return None;
    }
    let per_euro = async |currency: &str| match rates.rate_on(currency, day).await {
        RateAnswer::Rate(rate) => rate.per_euro_micros().filter(|micros| *micros > 0),
        RateAnswer::NoSuchCurrency | RateAnswer::Unavailable(_) => None,
    };
    let read = async {
        let dollars = per_euro(timada_research::CPC_CURRENCY).await?;
        if currency == "EUR" {
            return Some(dollars);
        }
        let units = per_euro(currency).await?;
        let worth = (u128::from(dollars) * 1_000_000 + u128::from(units) / 2) / u128::from(units);
        u64::try_from(worth).ok().filter(|worth| *worth > 0)
    };
    tokio::time::timeout(RATE_WAIT, read).await.ok().flatten()
}

/// A figure in hundredths (a return on ad spend, a price over a cost per
/// click) as `194.67`.
pub fn format_ratio(hundredths: u64) -> String {
    timada_core::format_amount(hundredths)
}

/// A cost per click in thousandths of a cent, to the tenth of a cent, in
/// `currency`: `0.308 EUR`. A click is priced finer than a cent.
pub fn format_click(millis: u64, currency: &str) -> String {
    let tenths = (millis + 50) / 100;
    format!("{}.{:03} {currency}", tenths / 1_000, tenths % 1_000)
}

/// The columns of a study's export.
pub const EXPORT_COLUMNS: [&str; 11] = [
    "keyword",
    "outcome",
    "counts_under",
    "searches_a_month",
    "difficulty",
    "cost_per_click_usd",
    "cost_per_click",
    "currency",
    "price_per_click",
    "channel",
    "intent",
];

/// How many keywords of a study one read of its export takes.
const EXPORT_PAGE: i64 = 500;

/// A study's keywords with what the score made of each, the most searched
/// first, as a CSV file: one line per keyword, a keyword two seeds gave
/// once. `judgement` is the study's, for what a click is worth against its
/// sale. The file is the owner's own report, read from the read model a
/// page at a time.
pub fn study_csv(
    db: Db,
    study: StudyState,
    judgement: timada_research::Judgement,
) -> topcoat::router::Body {
    use timada_core::page::{Cursor, PageFrom};
    use timada_research::{DUPLICATE, keywords_of_study};

    let (tx, rx) = tokio::sync::mpsc::channel::<Result<bytes::Bytes, crate::feed::Broken>>(2);
    tokio::spawn(async move {
        let mut head = String::from("\u{feff}");
        crate::export::csv_line(&mut head, &EXPORT_COLUMNS);
        if tx.send(Ok(head.into())).await.is_err() {
            return;
        }
        let currency = if judgement.cpc_converted {
            study.inputs.sale.currency.as_str()
        } else {
            timada_research::CPC_CURRENCY
        };
        let mut after: Option<Cursor> = None;
        loop {
            let from = match &after {
                Some(cursor) => PageFrom::Older(cursor),
                None => PageFrom::First,
            };
            let page = match keywords_of_study(&db, &study.id, from, EXPORT_PAGE).await {
                Ok(page) => page,
                Err(err) => {
                    tracing::error!(study = %study.id, error = %err, "a study's export stopped");
                    let _ = tx.send(Err(err.into())).await;
                    return;
                }
            };
            let mut text = String::new();
            for row in &page.rows {
                if row.outcome == DUPLICATE {
                    continue;
                }
                let click = row
                    .cpc_usd_cents
                    .map(|cents| study.inputs.cpc_millis(cents as u64));
                let kept = matches!(row.outcome.as_str(), "main" | "long_tail");
                let figure = |value: Option<i64>| value.map(|v| v.to_string()).unwrap_or_default();
                crate::export::csv_line(
                    &mut text,
                    &[
                        crate::export::typed(&row.keyword),
                        row.outcome.clone(),
                        crate::export::typed(row.variant_of.as_deref().unwrap_or_default()),
                        figure(row.volume),
                        figure(row.difficulty),
                        row.cpc_usd_cents
                            .map(|cents| timada_core::format_amount(cents as u64))
                            .unwrap_or_default(),
                        click
                            .map(|millis| {
                                let tenths = (millis + 50) / 100;
                                format!("{}.{:03}", tenths / 1_000, tenths % 1_000)
                            })
                            .unwrap_or_default(),
                        currency.to_owned(),
                        judgement
                            .sale
                            .zip(click)
                            .and_then(|(sale, click)| sale.price_per_click(click))
                            .map(format_ratio)
                            .unwrap_or_default(),
                        judgement
                            .sale
                            .filter(|_| kept)
                            .and_then(|sale| sale.channel(click))
                            .map(|channel| channel.as_str().to_owned())
                            .unwrap_or_default(),
                        row.intent.clone().unwrap_or_default(),
                    ],
                );
            }
            if !text.is_empty() && tx.send(Ok(text.into())).await.is_err() {
                return;
            }
            match page.older {
                Some(cursor) => after = Some(cursor),
                None => return,
            }
        }
    });
    topcoat::router::Body::new(crate::feed::Pages::of(rx))
}

/// Whether a request made at `requested_at` is history at `now` (both unix
/// seconds).
fn is_history(now: u64, requested_at: u64) -> bool {
    now.saturating_sub(requested_at) > REQUEST_WINDOW_SECS
}

/// What the owner reads about why a call for `query` (a seed, or a keyword
/// searched) gave nothing. A refusal is kept in the source's words: it says
/// what to put right (the balance, the country). Why the source could not
/// answer is the operator's to read.
fn reason_of(study_id: &str, query: &str, err: &KeywordError) -> String {
    tracing::warn!(study = %study_id, %query, error = %err, "the keyword source gave nothing");
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

/// A result of the source as a study keeps it.
fn result_of(result: SearchResult) -> ResultRow {
    ResultRow {
        position: result.position,
        domain: result.domain,
        url: result.url,
        title: result.title,
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

    #[tokio::test]
    async fn a_currency_is_worth_dollars_by_the_euro_rates() {
        let rates = timada_invoicing::rates::RateTable::from_ecb_xml(
            "<Envelope><Cube>
            <Cube time='2026-10-14'><Cube currency='USD' rate='1.0850'/></Cube>
            <Cube time='2026-10-13'><Cube currency='USD' rate='1.0900'/><Cube currency='GBP' rate='0.86'/></Cube>
            </Cube></Envelope>",
        )
        .unwrap();
        assert_eq!(
            usd_worth(&rates, "EUR", "2026-10-14").await,
            Some(1_085_000)
        );
        // 1.09 dollars a euro over 0.86 pounds a euro.
        assert_eq!(
            usd_worth(&rates, "GBP", "2026-10-13").await,
            Some(1_267_442)
        );
        assert_eq!(usd_worth(&rates, "USD", "2026-10-14").await, None);
        assert_eq!(usd_worth(&rates, "CHF", "2026-10-14").await, None);
        let none = timada_invoicing::rates::RateTable::default();
        assert_eq!(usd_worth(&none, "EUR", "2026-10-14").await, None);
        assert_eq!(format_click(30_821, "EUR"), "0.308 EUR");
        assert_eq!(format_click(123_456, "USD"), "1.235 USD");
    }

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
                "You have reached the most keyword research may cost in a day. It frees on {frees}."
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
                "The platform has reached the most keyword research may cost in a day."
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

    /// One pass of the saga that searches.
    async fn search_pass(store: &Evento, source: Option<&Arc<FakeKeywords>>, spending: Spending) {
        let keywords = Keywords(source.map(|source| source.clone() as _));
        results_subscription::<Evento>(keywords, store.clone(), spending)
            .retry(0)
            .run_once(store)
            .await
            .unwrap();
    }

    fn mains(keywords: &[&str]) -> Vec<String> {
        keywords.iter().map(|keyword| keyword.to_string()).collect()
    }

    /// Each main keyword is one paid search in the study's country and
    /// language, charged to who asked, its results recorded on the study.
    #[tokio::test]
    async fn the_first_results_asked_for_are_searched_and_recorded() {
        let store = store().await;
        let source = FakeKeywords::new();
        source.find("griffoir", &["www.amazon.fr", "zooplus.fr"]);
        let study = study(&store, &["griffoir"]).await;
        let studies = Command(store.clone());
        studies
            .request_results(
                ALICE,
                &study.id,
                &mains(&["griffoir", "griffoir chat"]),
                false,
            )
            .await
            .unwrap();
        search_pass(&store, Some(&source), OPEN).await;

        let asked = |keyword: &str| Asked {
            seed: keyword.to_owned(),
            country: "FR".to_owned(),
            language: "fr".to_owned(),
        };
        assert_eq!(
            source.searched(),
            [asked("griffoir"), asked("griffoir chat")]
        );
        assert!(source.asked().is_empty(), "no keywords were fetched");
        let after = reload(&store, &study).await;
        assert!(
            after
                .checks
                .iter()
                .all(|check| check.status == FetchStatus::Fetched)
        );
        assert_eq!(after.checks[0].results, 2);
        // A search nothing answers is searched all the same, and paid.
        assert_eq!(after.checks[1].results, 0);
        assert_eq!(after.cost_usd_micros, 4_000);
        assert_eq!(spent(&store, ALICE).await, (4_000, 0));

        // A second pass has nothing left to do.
        search_pass(&store, Some(&source), OPEN).await;
        assert_eq!(source.searched().len(), 2);
    }

    /// A search past the cap is not made, one the source fails costs
    /// nothing, and both say why on the keyword, for the owner to ask again.
    #[tokio::test]
    async fn a_search_past_the_cap_or_failed_says_why() {
        let store = store().await;
        let source = FakeKeywords::new();
        // Room for two searches at their most (2,000 each).
        let capped = Spending {
            caps: SpendCaps {
                user_usd_micros: 4_000,
                platform_usd_micros: 100_000_000,
            },
            reuse_secs: 0,
        };
        let study = study(&store, &["griffoir"]).await;
        let studies = Command(store.clone());
        let three = mains(&["griffoir", "griffoir chat", "griffoir mural"]);
        studies
            .request_results(ALICE, &study.id, &three, false)
            .await
            .unwrap();
        search_pass(&store, Some(&source), capped).await;
        let after = reload(&store, &study).await;
        assert_eq!(source.searched().len(), 2, "the third was never made");
        assert_eq!(after.checks[2].status, FetchStatus::Failed);
        assert!(
            after.checks[2]
                .last_error
                .as_deref()
                .unwrap()
                .starts_with("You have reached the most keyword research may cost in a day.")
        );

        // The source failing gives the room back.
        source.fail(Some(KeywordError::Unavailable("timed out".to_owned())));
        studies
            .request_results(ALICE, &study.id, &three, false)
            .await
            .unwrap();
        search_pass(&store, Some(&source), OPEN).await;
        let failed = reload(&store, &study).await;
        assert_eq!(
            failed.checks[2].last_error.as_deref(),
            Some("The keyword source did not answer; try again in a moment.")
        );
        assert_eq!(spent(&store, ALICE).await, (4_000, 0));

        // Without a source the request is answered all the same.
        studies
            .request_results(ALICE, &study.id, &three, false)
            .await
            .unwrap();
        search_pass(&store, None, OPEN).await;
        assert_eq!(
            reload(&store, &study).await.checks[2].last_error.as_deref(),
            Some(NO_SOURCE)
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
