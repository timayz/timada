//! The subscription that keeps `research_studies`, `research_keywords`,
//! `research_fetches` and `research_spend` current.
//!
//! The handler reloads the strict write model of the study and rewrites its
//! row, so delivery may repeat (the subscription is at-least-once) without
//! the row drifting; a deleted study loses its row and its keywords. The
//! keywords are not in the write model: the event that records a fetch
//! carries them, and its rows replace the seed's earlier ones, whole, in one
//! transaction. Events of a stream come in order, so a fetch delivered again
//! is followed by the ones after it and the seed ends on its last fetch.
//!
//! A charge adds to its user's row of the day, once: the row keeps the
//! version of the day's stream it was last moved at, and a charge at or
//! under it was counted already.

use evento::{
    Executor,
    metadata::{Event, RawEvent},
    subscription::{Context, SubscriptionBuilder},
};
use timada_core::db::{Db, Value, execute, params_from_iter};

use crate::aggregator::{Charged, KeywordRow, Study, StudyEvent};
use crate::command::{StudyState, study_projection};

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
    let (seed, rows, fetched_at) = match &decoded {
        StudyEvent::KeywordsFetched(fetched) => {
            (&fetched.seed, &fetched.rows, event.timestamp as i64)
        }
        StudyEvent::KeywordsReused(reused) => {
            (&reused.seed, &reused.rows, reused.fetched_at as i64)
        }
        _ => return upsert_study(&db, &study).await,
    };
    let fetch = Fetch {
        study_id: &study.id,
        seed,
        event_id: event.id.to_string(),
        fetched_at,
        rows,
    };
    replace_keywords(&db, &fetch).await?;
    upsert_study(&db, &study).await
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

/// Drops a deleted study's row and its keywords.
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
