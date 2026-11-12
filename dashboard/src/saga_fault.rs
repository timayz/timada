//! What a saga does with a step that failed: fail the pass, or give the
//! event up.
//!
//! A subscription's cursor serves every tenant of the region. A handler that
//! returns an error holds it for all of them: evento tries the pass again a
//! few times, the worker stops, and the supervisor (`workers.rs`) restarts
//! it at the same event with a growing delay, for ever. That is right when
//! the store itself failed (nothing else would progress either), and wrong
//! for a fault of the one event's own: a domain error on one order, a
//! supplier's permanent refusal, a cart that cannot be priced. Asked again
//! the same way, those answer the same, and every other tenant's invoices,
//! payment answers or captures stop behind them (#483).
//!
//! [`event_fault`] tells the two apart from the error a step returned. The
//! typed errors of the contexts say which of their variants are the store's
//! (`is_store_fault`); a read of the store fails with an untyped error, so
//! anything without a typed verdict is the store's too, the safe side: a
//! pass retried for nothing costs a delay, an event given up on for nothing
//! costs a fact the merchant must answer. On an event fault the handler
//! records the fact on the stream (`Order::InvoicingFailed`,
//! `Order::CaptureFailed`, a refusal of the checkout), so the merchant or
//! the shopper sees it and re-drives it, then returns the error. The saga's
//! subscription is in evento's dead-letter mode and asks [`fault`] whose
//! failure the error is: the event's own is recorded as a dead letter beside
//! the cursor and acknowledged, the store's fails the pass. The payments
//! desk, which answers off its cursor, records its own ([`dead_letter`]).
//! `/admin/cluster` lists them ([`dead_letters`]) for a week
//! ([`forget_old`]).

use std::time::Duration;

use anyhow::Result;
use evento::subscription::Fault;
use evento::{DeadLetter, Executor, MAX_DEAD_LETTER_ERROR_LEN, WriteError};
use timada_catalog::CatalogError;
use timada_discounts::DiscountError;
use timada_invoicing::InvoicingError;
use timada_orders::OrderError;
use tokio::task::JoinHandle;

use crate::db::Db;

/// How long of what went wrong a stream keeps, and the cluster page shows.
pub const REASON_LEN: usize = 200;

/// How long an event given up on stays on the cluster page.
pub const KEPT_SECS: u64 = 7 * 24 * 60 * 60;

/// How often the dead letters older than [`KEPT_SECS`] are looked for.
pub const FORGET_EVERY: Duration = Duration::from_secs(60 * 60);

/// What went wrong, in one line of at most [`REASON_LEN`] characters, when
/// `err` is the event's own fault: a domain error about this one order or
/// cart, which asking again the same way would meet again. `None` when it
/// is the store's: a write it is not taking for the moment, a write that
/// kept losing to another, a read that failed.
///
/// The first typed error of a context in the chain decides
/// (`is_store_fault`); a bare [`WriteError`] decides by
/// [`timada_core::is_store_write_fault`]; no typed error at all is a read of
/// the store that failed.
pub fn event_fault(err: &anyhow::Error) -> Option<String> {
    err.chain()
        .find_map(|cause| {
            let store = if let Some(err) = cause.downcast_ref::<OrderError>() {
                err.is_store_fault()
            } else if let Some(err) = cause.downcast_ref::<InvoicingError>() {
                err.is_store_fault()
            } else if let Some(err) = cause.downcast_ref::<CatalogError>() {
                err.is_store_fault()
            } else if let Some(err) = cause.downcast_ref::<DiscountError>() {
                err.is_store_fault()
            } else {
                timada_core::is_store_write_fault(cause.downcast_ref::<WriteError>()?)
            };
            Some((!store).then(|| one_line(&cause.to_string())))
        })
        .flatten()
}

/// Whose failure a saga handler's error is, for its subscription's
/// dead-letter mode (`SubscriptionBuilder::fault`): the event's own is
/// given up on at once, the retries would meet it again; the store's fails
/// the pass. Never `None`, which would have evento retry the event and then
/// give it up: an error without a typed verdict is the store's, and a store
/// away for a while must not have every event of the while given up on.
pub fn fault(err: &anyhow::Error) -> Option<Fault> {
    Some(match event_fault(err) {
        Some(_) => Fault::Event,
        None => Fault::Store,
    })
}

/// Settles `outcome` of recording an event fault (a refusal, a `…Failed`
/// on the stream): the store's fault fails the pass, so the recording is
/// tried again; anything else is logged, and the event given up on all
/// the same (nothing more can be written about it).
pub fn recorded(outcome: anyhow::Result<()>, what: &str) -> anyhow::Result<()> {
    match outcome {
        Ok(()) => Ok(()),
        Err(err) => match event_fault(&err) {
            None => Err(err),
            Some(reason) => {
                tracing::error!(%reason, "{what} could not be recorded on the stream");
                Ok(())
            }
        },
    }
}

/// `text` in one line of at most [`REASON_LEN`] characters.
pub fn one_line(text: &str) -> String {
    let line: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match line.char_indices().nth(REASON_LEN) {
        Some((cut, _)) => format!("{}…", &line[..cut]),
        None => line,
    }
}

/// Records that `subscription` gave `event` up for `err` at `now` (unix
/// seconds), as its dead-letter mode does with a handler's error: for the
/// payments desk, which answers a request off the subscription's cursor,
/// once the event is acknowledged. The row goes under the key of the
/// worker that handed the event over: a saga's worker follows one region,
/// under `{region}.{key}`, or the events written without one, under
/// `{key}`.
pub async fn dead_letter<E: Executor>(
    store: &E,
    subscription: &str,
    event: &evento::Event,
    err: &anyhow::Error,
    now: u64,
) -> Result<()> {
    let mut error = format!("{err:#}");
    if error.len() > MAX_DEAD_LETTER_ERROR_LEN {
        let mut end = MAX_DEAD_LETTER_ERROR_LEN;
        while !error.is_char_boundary(end) {
            end -= 1;
        }
        error.truncate(end);
    }
    tracing::error!(
        subscription,
        tenant = %event.tenant,
        stream = %event.aggregate_id,
        event = %event.name,
        %error,
        "an event the saga gave up on: recorded on its stream and as a dead letter"
    );
    store
        .save_dead_letter(DeadLetter {
            subscription: match &event.routing_key {
                Some(region) => format!("{region}.{subscription}"),
                None => subscription.to_owned(),
            },
            event_id: event.id,
            aggregate_type: event.aggregate_type.clone(),
            aggregate_id: event.aggregate_id.clone(),
            name: event.name.clone(),
            tenant: event.tenant.clone(),
            routing_key: event.routing_key.clone(),
            version: event.version,
            error,
            // Given up on at the first meeting, as a subscription gives up
            // on an event's own fault.
            attempts: 1,
            timestamp: now,
        })
        .await
}

/// The keys of the cursors this node keeps, a subscription's each (with
/// its region: `{region}.{key}`): evento keeps the dead letters beside them.
async fn cursor_keys(db: &Db) -> Result<Vec<String>> {
    let keys: Vec<(String,)> = db.fetch_all("SELECT key FROM subscriber", ()).await?;
    Ok(keys.into_iter().map(|(key,)| key).collect())
}

/// How many events this node's sagas gave up on, and the latest `limit` of
/// them, newest first.
pub async fn dead_letters<E: Executor>(
    store: &E,
    db: &Db,
    limit: usize,
) -> Result<(u64, Vec<DeadLetter>)> {
    let mut count = 0;
    let mut letters = Vec::new();
    for key in cursor_keys(db).await? {
        let kept = store.dead_letter_count(key.clone()).await?;
        if kept > 0 {
            count += kept;
            letters.extend(store.dead_letters(key, u16::MAX).await?);
        }
    }
    letters.sort_by_key(|letter| std::cmp::Reverse((letter.timestamp, letter.event_id)));
    letters.truncate(limit);
    Ok((count, letters))
}

/// Forgets the events given up on more than [`KEPT_SECS`] before `now`
/// (unix seconds); how many.
pub async fn forget_old<E: Executor>(store: &E, db: &Db, now: u64) -> Result<usize> {
    let mut forgotten = 0;
    for key in cursor_keys(db).await? {
        if store.dead_letter_count(key.clone()).await? == 0 {
            continue;
        }
        for letter in store.dead_letters(key.clone(), u16::MAX).await? {
            if letter.timestamp.saturating_add(KEPT_SECS) < now {
                store
                    .delete_dead_letter(key.clone(), letter.event_id)
                    .await?;
                forgotten += 1;
            }
        }
    }
    Ok(forgotten)
}

/// Forgets the week-old dead letters every `every`.
pub fn start_forgetting<E: Executor + Clone>(store: E, db: Db, every: Duration) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(every);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            if let Err(err) = forget_old(&store, &db, now()).await {
                tracing::error!(error = %format!("{err:#}"), "the week-old dead letters were not forgotten");
            }
        }
    })
}

/// Unix seconds now.
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_store(err: anyhow::Error) -> bool {
        event_fault(&err).is_none() && fault(&err) == Some(Fault::Store)
    }

    fn reason(err: anyhow::Error) -> String {
        assert_eq!(fault(&err), Some(Fault::Event), "{err:#}");
        event_fault(&err).unwrap()
    }

    #[test]
    fn a_domain_error_is_the_events_own() {
        assert_eq!(
            reason(OrderError::OrderCancelled.into()),
            "a cancelled order cannot be fulfilled"
        );
        assert_eq!(
            reason(
                InvoicingError::EuroRateMissing {
                    currency: "USD".into()
                }
                .into()
            ),
            "no euro rate for USD: the sale cannot be counted against the EU threshold"
        );
        assert!(!is_store(CatalogError::NotFound.into()));
        assert!(!is_store(DiscountError::Exhausted.into()));
        // Through a context layer: the chain is what counts.
        let err = anyhow::Error::from(InvoicingError::TotalMismatch).context("order o1");
        assert_eq!(reason(err), "the lines do not add up to what was paid");
    }

    #[test]
    fn the_stores_faults_fail_the_pass() {
        let paused = || WriteError::Unavailable(anyhow::anyhow!("backlog at cap"));
        assert!(is_store(OrderError::Write(paused()).into()));
        assert!(is_store(
            InvoicingError::Write(WriteError::InvalidOriginalVersion).into()
        ));
        assert!(is_store(InvoicingError::Busy.into()));
        assert!(is_store(
            OrderError::Other(anyhow::anyhow!("the connection is gone")).into()
        ));
        assert!(is_store(paused().into()));
        // A read of the store fails untyped: the store's.
        assert!(is_store(anyhow::anyhow!("database is locked")));
    }

    #[test]
    fn a_write_that_cannot_be_right_is_the_events_own() {
        assert_eq!(
            reason(WriteError::VersionOverflow.into()),
            "aggregate version overflow"
        );
        assert!(!is_store(OrderError::Write(WriteError::MissingData).into()));
    }

    #[test]
    fn recording_a_fault_fails_the_pass_only_for_the_store() {
        assert!(recorded(Ok(()), "x").is_ok());
        assert!(recorded(Err(OrderError::OrderNotFound.into()), "x").is_ok());
        assert!(
            recorded(
                Err(OrderError::Write(WriteError::Unavailable(anyhow::anyhow!("paused"))).into()),
                "x"
            )
            .is_err()
        );
    }

    #[test]
    fn a_reason_is_one_short_line() {
        let long = one_line(&"x".repeat(500));
        assert_eq!(long.chars().count(), REASON_LEN + 1);
        assert!(long.ends_with('…'));
        assert_eq!(one_line("two\n  lines"), "two lines");
    }

    /// The desk's dead letters go under the key of the worker that handed
    /// the event over, are listed newest first across the cursors, counted,
    /// and forgotten once a week old.
    #[tokio::test]
    async fn dead_letters_are_listed_a_week_and_counted() {
        let store = evento::Turso::in_memory_concurrent().await.unwrap();
        let db = Db::from(store.clone());
        for key in ["s", "eu.s", "eu.t"] {
            store
                .upsert_subscriber(key.to_owned(), ulid::Ulid::generate())
                .await
                .unwrap();
        }
        let event = |tenant: &str, stream: &str, region: Option<&str>| evento::Event {
            id: ulid::Ulid::generate(),
            tenant: tenant.to_owned(),
            aggregate_type: "timada-orders/Order".to_owned(),
            aggregate_id: stream.to_owned(),
            name: "Paid".to_owned(),
            routing_key: region.map(str::to_owned),
            ..Default::default()
        };
        let refused = anyhow::Error::from(OrderError::OrderCancelled).context("order o1");
        let now = 1_800_000_000;
        let old = event("t1", "o1", None);
        dead_letter(&store, "s", &old, &refused, now - KEPT_SECS - 1)
            .await
            .unwrap();
        let first = event("t1", "o2", Some("eu"));
        dead_letter(&store, "s", &first, &refused, now - 10)
            .await
            .unwrap();
        // The same event again: one row, the latest time.
        dead_letter(&store, "s", &first, &refused, now - 5)
            .await
            .unwrap();
        let last = event("t2", "o3", Some("eu"));
        dead_letter(&store, "t", &last, &refused, now - 1)
            .await
            .unwrap();

        let (count, letters) = dead_letters(&store, &db, 10).await.unwrap();
        assert_eq!(count, 3);
        assert_eq!(
            letters
                .iter()
                .map(|letter| (letter.subscription.as_str(), letter.aggregate_id.as_str()))
                .collect::<Vec<_>>(),
            [("eu.t", "o3"), ("eu.s", "o2"), ("s", "o1")],
            "newest first, under the region's key"
        );
        assert_eq!(letters[1].timestamp, now - 5);
        assert_eq!(letters[1].attempts, 1);
        assert_eq!(
            letters[1].error,
            "order o1: a cancelled order cannot be fulfilled"
        );
        assert_eq!(dead_letters(&store, &db, 1).await.unwrap().1.len(), 1);

        assert_eq!(forget_old(&store, &db, now).await.unwrap(), 1);
        let (count, letters) = dead_letters(&store, &db, 10).await.unwrap();
        assert_eq!(count, 2, "the week-old one is gone");
        assert!(letters.iter().all(|letter| letter.aggregate_id != "o1"));
        assert_eq!(forget_old(&store, &db, now).await.unwrap(), 0);
    }
}
