//! The subscription that keeps `research_studies`, `research_keywords`,
//! `research_fetches`, `research_spend` and the three tables of the
//! competition current.
//!
//! The handler reloads the strict write model of the study and rewrites its
//! row, so delivery may repeat (the subscription is at-least-once) without
//! the row drifting; a deleted study loses its row and its keywords. The
//! keywords are not in the write model: the event that records a fetch
//! carries them, and its rows replace the seed's earlier ones, whole, in one
//! transaction. Events of a stream come in order, so a fetch delivered again
//! is followed by the ones after it and the seed ends on its last fetch.
//!
//! Who holds the first results is worked out from what the tables hold
//! ([`recheck`]): a study's main keywords, the results of each and its
//! owner's marks. The three change on two streams, the study's and the
//! user's marks, whose events may reach a node in either order; working it
//! out again from the tables at each of them ends on the same rows.
//!
//! A charge adds to its user's row of the day, once: the row keeps the
//! version of the day's stream it was last moved at, and a charge at or
//! under it was counted already.

use std::collections::{HashMap, HashSet};

use evento::{
    Executor,
    metadata::{Event, RawEvent},
    subscription::{Context, SubscriptionBuilder},
};
use timada_core::db::{Db, FromRow, Value, execute, params_from_iter};

use crate::aggregator::{Charged, DomainMarked, KeywordRow, ResultRow, Study, StudyEvent};
use crate::command::{StudyState, study_projection};
use crate::score::{Score, TestState, Thresholds, Verdict, competition, score};

use super::competition::{DomainRow, domains_of_study, main_keywords};
use super::keywords::DUPLICATE;

/// The cursor identity of the read-model subscription, in the store.
pub const READ_MODEL_SUBSCRIPTION: &str = "timada-research/read-models";

/// How many keywords one `INSERT` writes: a statement costs more than a
/// row, and a fetch gives up to a thousand.
const ROWS_PER_INSERT: usize = 50;

/// Builds the subscription over every routing key of the platform tenant,
/// where the studies live. Start it with `.start(&store)` and keep the
/// handle for shutdown.
pub fn read_model_subscription<E: Executor>(db: Db) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(READ_MODEL_SUBSCRIPTION)
        .data(db)
        .handler(on_study_event())
        .handler(on_charged())
        .handler(on_domain_marked())
        .any_routing_key()
        .tenant("")
}

#[evento::subscription_all]
async fn on_study_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<Study>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let Some(study) = study_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    let decoded = event.decode()?;
    // A paid fetch answers later ones of its seed whatever becomes of the
    // study: its rows stay in the stream.
    if let StudyEvent::KeywordsFetched(fetched) = &decoded {
        note_fetch(&db, &study, &fetched.seed, event.timestamp as i64).await?;
    }
    if study.deleted {
        return delete_study(&db, &study.id).await;
    }
    let fetch = match &decoded {
        StudyEvent::KeywordsFetched(fetched) => {
            Some((&fetched.seed, &fetched.rows, event.timestamp as i64))
        }
        StudyEvent::KeywordsReused(reused) => {
            Some((&reused.seed, &reused.rows, reused.fetched_at as i64))
        }
        _ => None,
    };
    if let Some((seed, rows, fetched_at)) = fetch {
        let fetch = Fetch {
            study_id: &study.id,
            seed,
            event_id: event.id.to_string(),
            fetched_at,
            rows,
        };
        replace_keywords(&db, &fetch).await?;
    }
    upsert_study(&db, &study).await?;
    // What the score reads changed: new rows, or what the owner said.
    let scores = fetch.is_some()
        || matches!(
            decoded,
            StudyEvent::InputsSet(_)
                | StudyEvent::KeywordDropped(_)
                | StudyEvent::KeywordRestored(_)
        );
    if scores {
        rescore(&db, &study).await?;
    }
    let searched = match &decoded {
        StudyEvent::ResultsFetched(fetched) => {
            note_results(
                &db,
                &study.id,
                &fetched.keyword,
                &fetched.results,
                event.timestamp as i64,
            )
            .await?;
            true
        }
        _ => false,
    };
    // What the competition reads changed: the main keywords, their first
    // results, or what the other tests say.
    if scores || searched {
        recheck(&db, &study.id, &study.owner).await?;
    }
    Ok(())
}

#[evento::subscription]
async fn on_domain_marked<E: Executor>(
    context: &Context<'_, E>,
    event: Event<DomainMarked>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let user_id = event.aggregate_id.as_str();
    db.execute(
        "INSERT INTO research_domain_marks (user_id, domain, specialist) VALUES (?1, ?2, ?3)
         ON CONFLICT (user_id, domain) DO UPDATE SET specialist = excluded.specialist",
        (
            user_id,
            event.data.domain.as_str(),
            i64::from(event.data.specialist),
        ),
    )
    .await?;
    // The mark counts on every study of the user where the site was found.
    let studies: Vec<(String,)> = db
        .fetch_all(STUDIES_WITH_DOMAIN, (user_id, event.data.domain.as_str()))
        .await?;
    for (study_id,) in studies {
        recheck(&db, &study_id, user_id).await?;
    }
    Ok(())
}

/// The studies of a user (`?1`) where a site (`?2`) was found.
pub(super) const STUDIES_WITH_DOMAIN: &str =
    "SELECT study_id FROM research_domains INDEXED BY research_domains_owner
     WHERE owner_id = ?1 AND domain = ?2";

/// The keywords of a study whose first results are held.
pub(super) const CHECKS: &str = "SELECT keyword, results FROM research_checks
     INDEXED BY sqlite_autoindex_research_checks_1 WHERE study_id = ?1";

/// What a user said of the sites they marked.
pub(super) const MARKS: &str = "SELECT domain, specialist FROM research_domain_marks
     INDEXED BY sqlite_autoindex_research_domain_marks_1 WHERE user_id = ?1";

/// Drops the sites found for a study.
pub(super) const DELETE_DOMAINS: &str = "DELETE FROM research_domains
     INDEXED BY sqlite_autoindex_research_domains_1 WHERE study_id = ?1";

/// Keeps the first results of a search for `keyword`, in place of what an
/// earlier search of it gave: one line each, its place then its site.
async fn note_results(
    db: &Db,
    study_id: &str,
    keyword: &str,
    results: &[ResultRow],
    fetched_at: i64,
) -> anyhow::Result<()> {
    let lines: Vec<String> = results
        .iter()
        .map(|result| format!("{} {}", result.position, result.domain))
        .collect();
    db.execute(
        "INSERT INTO research_checks (study_id, keyword, fetched_at, results)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (study_id, keyword) DO UPDATE SET
            fetched_at = excluded.fetched_at, results = excluded.results",
        (study_id, keyword, fetched_at, lines.join("\n")),
    )
    .await?;
    Ok(())
}

/// The sites found for a study, to write in place of the ones held.
struct Found<'a> {
    study_id: &'a str,
    owner_id: &'a str,
    sites: &'a [DomainRow],
}

/// Works out who holds the first results of a study's main keywords from
/// what the tables hold, and writes what changed: the sites found, each
/// with for how many of the keywords and at what places, and on the study's
/// row where the competition test stands and the verdict it gives with the
/// other tests. The results of a keyword that is no longer a main one stay
/// held and are not counted.
async fn recheck(db: &Db, study_id: &str, owner_id: &str) -> anyhow::Result<()> {
    let base: Vec<(String,)> = db
        .fetch_all(
            "SELECT base_verdict FROM research_studies WHERE id = ?1",
            (study_id,),
        )
        .await?;
    let Some((base,)) = base.into_iter().next() else {
        return Ok(());
    };
    let mains = main_keywords(db, study_id).await?;
    let checks: HashMap<String, String> = db
        .fetch_all::<(String, String)>(CHECKS, (study_id,))
        .await?
        .into_iter()
        .collect();
    let (mut searched, mut owed) = (0u32, 0u32);
    let mut places: HashMap<&str, Vec<u8>> = HashMap::new();
    for keyword in &mains {
        let Some(results) = checks.get(keyword) else {
            owed += 1;
            continue;
        };
        searched += 1;
        // A site with two results for the keyword counts at its best.
        let mut best: HashMap<&str, u8> = HashMap::new();
        for line in results.lines() {
            let Some((position, domain)) = line.split_once(' ') else {
                continue;
            };
            let Ok(position) = position.parse::<u8>() else {
                continue;
            };
            let held = best.entry(domain).or_insert(position);
            *held = (*held).min(position);
        }
        for (domain, position) in best {
            places.entry(domain).or_default().push(position);
        }
    }
    let mut sites: Vec<DomainRow> = places
        .into_iter()
        .map(|(domain, mut positions)| {
            positions.sort_unstable();
            DomainRow {
                domain: domain.to_owned(),
                keywords: positions.len() as i64,
                best: positions.first().copied().map_or(0, i64::from),
                positions: positions
                    .iter()
                    .map(u8::to_string)
                    .collect::<Vec<_>>()
                    .join(", "),
            }
        })
        .collect();
    sites.sort_by(|a, b| {
        (b.keywords, a.best, a.domain.as_str()).cmp(&(a.keywords, b.best, b.domain.as_str()))
    });
    let held = domains_of_study(db, study_id).await?;
    if held != sites {
        let found = Found {
            study_id,
            owner_id,
            sites: &sites,
        };
        db.write(&found, |tx, found| {
            Box::pin(async move {
                execute(tx, DELETE_DOMAINS, (found.study_id,)).await?;
                for chunk in found.sites.chunks(ROWS_PER_INSERT) {
                    let mut values = Vec::with_capacity(chunk.len() * 6);
                    let mut tuples = Vec::with_capacity(chunk.len());
                    for (at, site) in chunk.iter().enumerate() {
                        let n = at * 6;
                        tuples.push(format!(
                            "(?{}, ?{}, ?{}, ?{}, ?{}, ?{})",
                            n + 1,
                            n + 2,
                            n + 3,
                            n + 4,
                            n + 5,
                            n + 6
                        ));
                        values.extend([
                            Value::from(found.study_id),
                            Value::from(site.domain.as_str()),
                            Value::from(found.owner_id),
                            Value::from(site.keywords),
                            Value::from(site.best),
                            Value::from(site.positions.as_str()),
                        ]);
                    }
                    let statement = format!(
                        "INSERT INTO research_domains (study_id, domain, owner_id, keywords,
                            best, positions)
                         VALUES {}",
                        tuples.join(", ")
                    );
                    execute(tx, &statement, params_from_iter(values)).await?;
                }
                Ok(())
            })
        })
        .await?;
    }
    let marks: HashMap<String, i64> = db
        .fetch_all::<(String, i64)>(MARKS, (owner_id,))
        .await?
        .into_iter()
        .collect();
    let stands = competition(
        searched,
        owed,
        sites.iter().map(|site| {
            (
                site.keywords as u32,
                marks.get(&site.domain).map(|specialist| *specialist != 0),
            )
        }),
    );
    db.execute(
        "UPDATE research_studies SET competition = ?2, searched_count = ?3, owed_count = ?4,
            verdict = ?5
         WHERE id = ?1",
        (
            study_id,
            stands.state.as_str(),
            i64::from(searched),
            i64::from(owed),
            Verdict::parse(&base).with(stands.state).as_str(),
        ),
    )
    .await?;
    Ok(())
}

/// Notes where `seed` was last paid for in the study's country and
/// language; an older fetch delivered again changes nothing.
async fn note_fetch(
    db: &Db,
    study: &StudyState,
    seed: &str,
    fetched_at: i64,
) -> anyhow::Result<()> {
    db.execute(
        "INSERT INTO research_fetches (country, language, seed, study_id, fetched_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (country, language, seed) DO UPDATE SET
            study_id = excluded.study_id, fetched_at = excluded.fetched_at
         WHERE excluded.fetched_at >= research_fetches.fetched_at",
        (
            study.country.as_str(),
            study.language.as_str(),
            seed,
            study.id.as_str(),
            fetched_at,
        ),
    )
    .await?;
    Ok(())
}

#[evento::subscription]
async fn on_charged<E: Executor>(
    context: &Context<'_, E>,
    event: Event<Charged>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    count_charge(
        &db,
        &event.aggregate_id,
        &event.data.user_id,
        event.data.cost_usd_micros,
        event.version,
    )
    .await
}

/// Adds a charge to its user's row of the day, unless the row was already
/// moved at `version` of the day's stream or a later one.
pub(super) async fn count_charge(
    db: &Db,
    day: &str,
    user_id: &str,
    cost_usd_micros: u64,
    version: u16,
) -> anyhow::Result<()> {
    db.execute(
        "INSERT INTO research_spend (day, user_id, usd_micros, calls, version)
         VALUES (?1, ?2, ?3, 1, ?4)
         ON CONFLICT (day, user_id) DO UPDATE SET
            usd_micros = usd_micros + excluded.usd_micros, calls = calls + 1,
            version = excluded.version
         WHERE excluded.version > research_spend.version",
        (day, user_id, cost_usd_micros as i64, i64::from(version)),
    )
    .await?;
    Ok(())
}

/// Writes the row of a study as the write model sees it.
async fn upsert_study(db: &Db, study: &StudyState) -> anyhow::Result<()> {
    db.execute(
        "INSERT INTO research_studies (id, owner_id, name, country, language, seed_count,
            keyword_count, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT (id) DO UPDATE SET
            name = excluded.name, seed_count = excluded.seed_count,
            keyword_count = excluded.keyword_count, updated_at = excluded.updated_at",
        (
            study.id.as_str(),
            study.owner.as_str(),
            study.name.as_str(),
            study.country.as_str(),
            study.language.as_str(),
            study.seeds.len() as i64,
            study.keywords() as i64,
            study.created_at as i64,
            study.updated_at as i64,
        ),
    )
    .await?;
    Ok(())
}

/// One keyword of a study, with what the last score made of it.
#[derive(FromRow)]
struct Scored {
    id: String,
    keyword: String,
    volume: Option<i64>,
    cpc_usd_cents: Option<i64>,
    difficulty: Option<i64>,
    intent: Option<String>,
    outcome: String,
    variant_of: Option<String>,
}

/// Every keyword of a study, the most searched first.
pub(super) const ALL_KEYWORDS: &str =
    "SELECT id, keyword, volume, cpc_usd_cents, difficulty, intent, outcome, variant_of
     FROM research_keywords INDEXED BY research_keywords_volume
     WHERE study_id = ?1 ORDER BY sort_volume DESC, id DESC";

/// Writes what the score made of one keyword.
pub(super) const SET_OUTCOME: &str =
    "UPDATE research_keywords INDEXED BY sqlite_autoindex_research_keywords_1
     SET outcome = ?3, variant_of = ?4 WHERE study_id = ?1 AND id = ?2";

/// How many keywords one transaction rewrites the outcome of.
const OUTCOMES_PER_WRITE: usize = 200;

/// The rows of a study whose outcome changed.
struct Outcomes<'a> {
    study_id: &'a str,
    rows: &'a [(String, &'static str, Option<String>)],
}

/// Scores the study from the keywords the read model holds and from what
/// its owner said, and writes what changed: each keyword's outcome, and on
/// the study's row what they come to and what its tests say but for the
/// competition, which [`recheck`] joins to them after. The thresholds are
/// the defaults. A keyword two seeds gave is scored on its most searched
/// row, the others being [`DUPLICATE`]s.
async fn rescore(db: &Db, study: &StudyState) -> anyhow::Result<()> {
    let stored: Vec<Scored> = db.fetch_all(ALL_KEYWORDS, (study.id.as_str(),)).await?;
    let mut first: HashSet<&str> = HashSet::with_capacity(stored.len());
    let mut scored_at: Vec<bool> = Vec::with_capacity(stored.len());
    let mut rows: Vec<KeywordRow> = Vec::with_capacity(stored.len());
    for row in &stored {
        let scored = first.insert(row.keyword.as_str());
        scored_at.push(scored);
        if scored {
            rows.push(KeywordRow {
                keyword: row.keyword.clone(),
                volume: row.volume.map(|volume| volume as u64),
                cpc_usd_cents: row.cpc_usd_cents.map(|cents| cents as u64),
                difficulty: row
                    .difficulty
                    .map(|difficulty| difficulty.clamp(0, 100) as u8),
                intent: row.intent.clone(),
            });
        }
    }
    // Scored as if the competition passed: the verdict is the other tests'.
    let score = score(
        rows,
        &study.language,
        &study.inputs,
        TestState::Passed,
        &Thresholds::default(),
    );
    let made: HashMap<&str, (&'static str, Option<&str>)> = score
        .keywords
        .iter()
        .map(|keyword| {
            let of = match &keyword.outcome {
                crate::score::Outcome::Variant { of } => Some(of.as_str()),
                _ => None,
            };
            (keyword.row.keyword.as_str(), (keyword.outcome.as_str(), of))
        })
        .collect();
    let mut changed: Vec<(String, &'static str, Option<String>)> = Vec::new();
    for (row, scored) in stored.iter().zip(scored_at) {
        let (outcome, of) = if scored {
            made.get(row.keyword.as_str())
                .copied()
                .unwrap_or((DUPLICATE, None))
        } else {
            (DUPLICATE, None)
        };
        if row.outcome != outcome || row.variant_of.as_deref() != of {
            changed.push((row.id.clone(), outcome, of.map(str::to_owned)));
        }
    }
    for rows in changed.chunks(OUTCOMES_PER_WRITE) {
        let outcomes = Outcomes {
            study_id: &study.id,
            rows,
        };
        db.write(&outcomes, |tx, outcomes| {
            Box::pin(async move {
                for (id, outcome, of) in outcomes.rows {
                    execute(
                        tx,
                        SET_OUTCOME,
                        (outcomes.study_id, id.as_str(), *outcome, of.clone()),
                    )
                    .await?;
                }
                Ok(())
            })
        })
        .await?;
    }
    set_score(db, study, &score).await
}

/// Writes what a study's keywords come to and what its other tests say on
/// its row.
async fn set_score(db: &Db, study: &StudyState, score: &Score) -> anyhow::Result<()> {
    let sale = score.sale;
    db.execute(
        "UPDATE research_studies SET base_verdict = ?2, scored_keywords = ?3, main_count = ?4,
            long_tail_count = ?5, kept_volume = ?6, avg_cpc_millis = ?7, currency = ?8,
            break_even_roas = ?9, price_per_click = ?10, needed_price_per_click = ?11
         WHERE id = ?1",
        (
            study.id.as_str(),
            score.verdict.as_str(),
            score.keywords.len() as i64,
            i64::from(score.main),
            i64::from(score.long_tail),
            score.kept_volume as i64,
            score.avg_cpc_millis.map(|millis| millis as i64),
            study.inputs.sale.currency.as_str(),
            sale.and_then(|sale| sale.margin.break_even_roas())
                .map(|roas| roas as i64),
            score.price_per_click.map(|ratio| ratio as i64),
            sale.and_then(|sale| sale.needed_price_per_click)
                .map(|ratio| ratio as i64),
        ),
    )
    .await?;
    Ok(())
}

/// Drops a deleted study's row, its keywords and what was found for them.
async fn delete_study(db: &Db, study_id: &str) -> anyhow::Result<()> {
    db.write(study_id, |tx, study_id| {
        Box::pin(async move {
            execute(
                tx,
                "DELETE FROM research_keywords INDEXED BY research_keywords_seed
                 WHERE study_id = ?1",
                (study_id,),
            )
            .await?;
            execute(
                tx,
                "DELETE FROM research_checks INDEXED BY sqlite_autoindex_research_checks_1
                 WHERE study_id = ?1",
                (study_id,),
            )
            .await?;
            execute(tx, DELETE_DOMAINS, (study_id,)).await?;
            execute(
                tx,
                "DELETE FROM research_studies WHERE id = ?1",
                (study_id,),
            )
            .await?;
            Ok(())
        })
    })
    .await
}

/// One fetch of a seed, as its event recorded it.
struct Fetch<'a> {
    study_id: &'a str,
    seed: &'a str,
    /// The event's id: with a row's place in the fetch, the row's id, the
    /// same on every node.
    event_id: String,
    fetched_at: i64,
    rows: &'a [KeywordRow],
}

/// Replaces the keywords of the fetch's seed with its rows.
async fn replace_keywords(db: &Db, fetch: &Fetch<'_>) -> anyhow::Result<()> {
    db.write(fetch, |tx, fetch| {
        Box::pin(async move {
            execute(
                tx,
                "DELETE FROM research_keywords INDEXED BY research_keywords_seed
                 WHERE study_id = ?1 AND seed = ?2",
                (fetch.study_id, fetch.seed),
            )
            .await?;
            for (chunk_at, chunk) in fetch.rows.chunks(ROWS_PER_INSERT).enumerate() {
                let mut values = Vec::with_capacity(chunk.len() * 10);
                let mut tuples = Vec::with_capacity(chunk.len());
                for (at, row) in chunk.iter().enumerate() {
                    let position = chunk_at * ROWS_PER_INSERT + at;
                    // The page reads a volume's rows by `id` downwards: the
                    // place counted from the end keeps them in the order the
                    // source gave them.
                    let id = format!(
                        "{}{:04}",
                        fetch.event_id,
                        9999usize.saturating_sub(position)
                    );
                    let n = at * 10;
                    tuples.push(format!(
                        "(?{}, ?{}, ?{}, ?{}, ?{}, ?{}, ?{}, ?{}, ?{}, ?{})",
                        n + 1,
                        n + 2,
                        n + 3,
                        n + 4,
                        n + 5,
                        n + 6,
                        n + 7,
                        n + 8,
                        n + 9,
                        n + 10
                    ));
                    values.extend([
                        Value::from(fetch.study_id),
                        Value::from(id),
                        Value::from(fetch.seed),
                        Value::from(row.keyword.as_str()),
                        Value::from(row.volume.map(|volume| volume as i64)),
                        Value::from(row.cpc_usd_cents.map(|cents| cents as i64)),
                        Value::from(row.difficulty.map(i64::from)),
                        Value::from(row.intent.clone()),
                        Value::from(row.volume.unwrap_or(0) as i64),
                        Value::from(fetch.fetched_at),
                    ]);
                }
                let statement = format!(
                    "INSERT INTO research_keywords (study_id, id, seed, keyword, volume,
                        cpc_usd_cents, difficulty, intent, sort_volume, fetched_at)
                     VALUES {}",
                    tuples.join(", ")
                );
                execute(tx, &statement, params_from_iter(values)).await?;
            }
            Ok(())
        })
    })
    .await
}
