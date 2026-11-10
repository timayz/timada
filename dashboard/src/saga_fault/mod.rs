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
//! [`Fault::of`] tells the two apart from the error a step returned. The
//! typed errors of the contexts say which of their variants are the store's
//! (`is_store_fault`); a read of the store fails with an untyped error, so
//! anything without a typed verdict is the store's too, the safe side: a
//! pass retried for nothing costs a delay, an event given up on for nothing
//! costs a fact the merchant must answer. On an event fault the handler
//! records the fact on the stream (`Order::InvoicingFailed`,
//! `Order::CaptureFailed`, a refusal of the checkout), so the merchant or
//! the shopper sees it and re-drives it, writes a row of [`given_up`] for
//! the cluster page, and acknowledges the event.
//!
//! An evento mode that did this for every subscription (dead-letter after
//! the retries, acknowledge, list) would replace the per-handler
//! classification: proposed as evento #509.

use anyhow::Result;
use evento::WriteError;
use timada_catalog::CatalogError;
use timada_core::db::FromRow;
use timada_discounts::DiscountError;
use timada_invoicing::InvoicingError;
use timada_orders::OrderError;

use crate::db::Db;

mod schema;

pub use schema::{MIGRATIONS, TABLES};

/// How long of what went wrong a stream and the table keep.
pub const REASON_LEN: usize = 200;

/// How long an event given up on stays on the cluster page.
pub const KEPT_SECS: u64 = 7 * 24 * 60 * 60;

/// What a saga does with the error of a step.
#[derive(Debug)]
pub enum Fault {
    /// The store's: a write it is not taking for the moment, a write that
    /// kept losing to another, a read that failed. The pass fails and is
    /// tried again; nothing else would progress either.
    Store(anyhow::Error),
    /// The event's own: a domain error about this one order or cart, which
    /// asking again the same way would meet again. Recorded on the stream
    /// and in the table, and the event acknowledged; what went wrong, in
    /// one line of at most [`REASON_LEN`] characters.
    Event(String),
}

impl Fault {
    /// Classifies `err`: the first typed error of a context in its chain
    /// decides (`is_store_fault`); a bare [`WriteError`] decides by
    /// [`timada_core::is_store_write_fault`]; no typed error at all is a
    /// read of the store that failed.
    pub fn of(err: anyhow::Error) -> Self {
        let verdict = err.chain().find_map(|cause| {
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
        });
        match verdict {
            Some(Some(reason)) => Self::Event(reason),
            _ => Self::Store(err),
        }
    }
}

/// Settles `outcome` of recording an event fault (a refusal, a `…Failed`
/// on the stream): the store's fault fails the pass, so the recording is
/// tried again; anything else is logged, the event acknowledged all the
/// same (it is given up on, and nothing more can be written about it).
pub fn recorded(outcome: anyhow::Result<()>, what: &str) -> anyhow::Result<()> {
    match outcome {
        Ok(()) => Ok(()),
        Err(err) => match Fault::of(err) {
            Fault::Store(err) => Err(err),
            Fault::Event(reason) => {
                tracing::error!(%reason, "{what} could not be recorded on the stream");
                Ok(())
            }
        },
    }
}

/// `text` in one line of at most [`REASON_LEN`] characters.
fn one_line(text: &str) -> String {
    let line: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match line.char_indices().nth(REASON_LEN) {
        Some((cut, _)) => format!("{}…", &line[..cut]),
        None => line,
    }
}

/// One row of `saga_given_up`: an event a saga of this node gave up on.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct GivenUp {
    /// The subscription's cursor identity (`timada-dashboard/invoicing`).
    pub subscription: String,
    pub tenant_id: String,
    pub stream_id: String,
    /// The event's name (`Paid`).
    pub event: String,
    pub reason: String,
    /// Unix seconds.
    pub given_up_at: i64,
}

/// Records that `subscription` gave `event` up for `reason`, at `now`
/// (unix seconds), and logs it; a row older than [`KEPT_SECS`] goes. The
/// same event given up on again (a replay, a retry the merchant asked for)
/// keeps one row, with the latest reason.
pub async fn give_up(
    db: &Db,
    subscription: &str,
    event: &evento::Event,
    reason: &str,
    now: u64,
) -> Result<()> {
    tracing::error!(
        subscription,
        tenant = %event.tenant,
        stream = %event.aggregate_id,
        event = %event.name,
        %reason,
        "an event the saga gave up on: recorded on its stream and acknowledged"
    );
    db.execute(
        "INSERT INTO saga_given_up (subscription, tenant_id, stream_id, event, reason, given_up_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT (subscription, tenant_id, stream_id, event)
         DO UPDATE SET reason = excluded.reason, given_up_at = excluded.given_up_at",
        (
            subscription,
            event.tenant.as_str(),
            event.aggregate_id.as_str(),
            event.name.as_str(),
            reason,
            now as i64,
        ),
    )
    .await?;
    db.execute(
        "DELETE FROM saga_given_up WHERE given_up_at < ?1",
        (now.saturating_sub(KEPT_SECS) as i64,),
    )
    .await?;
    Ok(())
}

/// How many events this node's sagas gave up on in the last week.
pub async fn given_up_count(db: &Db) -> Result<i64> {
    let (count,): (i64,) = db
        .fetch_one("SELECT count(*) FROM saga_given_up", ())
        .await?;
    Ok(count)
}

/// The events given up on, latest first, at most `limit`.
pub async fn given_up(db: &Db, limit: i64) -> Result<Vec<GivenUp>> {
    db.fetch_all(
        "SELECT subscription, tenant_id, stream_id, event, reason, given_up_at
         FROM saga_given_up
         ORDER BY given_up_at DESC, subscription, tenant_id, stream_id, event
         LIMIT ?1",
        (limit,),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_store(err: anyhow::Error) -> bool {
        matches!(Fault::of(err), Fault::Store(_))
    }

    fn reason(err: anyhow::Error) -> String {
        match Fault::of(err) {
            Fault::Event(reason) => reason,
            Fault::Store(err) => panic!("the store's: {err:#}"),
        }
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

    #[tokio::test]
    async fn given_up_events_are_listed_a_week_and_counted() {
        let store = evento::Turso::in_memory_concurrent().await.unwrap();
        crate::db::migrate_store_read_models(&store).await.unwrap();
        let db = Db::from(store);
        let event = |tenant: &str, stream: &str, name: &str| evento::Event {
            tenant: tenant.to_owned(),
            aggregate_id: stream.to_owned(),
            name: name.to_owned(),
            ..Default::default()
        };
        let now = 1_800_000_000;
        give_up(
            &db,
            "s",
            &event("t1", "o1", "Paid"),
            "no rate",
            now - KEPT_SECS - 1,
        )
        .await
        .unwrap();
        give_up(&db, "s", &event("t1", "o2", "Paid"), "no rate", now - 10)
            .await
            .unwrap();
        // The same event again: one row, the latest reason.
        give_up(&db, "s", &event("t1", "o2", "Paid"), "still no rate", now)
            .await
            .unwrap();
        give_up(
            &db,
            "s",
            &event("t2", "o3", "Refunded"),
            "too large",
            now - 5,
        )
        .await
        .unwrap();
        assert_eq!(given_up_count(&db).await.unwrap(), 2, "the old one is gone");
        let rows = given_up(&db, 10).await.unwrap();
        assert_eq!(
            rows.iter()
                .map(|row| (row.stream_id.as_str(), row.reason.as_str()))
                .collect::<Vec<_>>(),
            [("o2", "still no rate"), ("o3", "too large")]
        );
        assert_eq!(given_up(&db, 1).await.unwrap().len(), 1);
    }
}
