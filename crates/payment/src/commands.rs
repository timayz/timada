//! Payment commands.
//!
//! Both commands are dispatched by the order fulfillment saga, which may
//! replay them, so both are idempotent.

use std::sync::Arc;

use evento::{AggregateExt as _, ProjectionAggregate as _};
use timada_core::{Executor, Money};

use crate::aggregate::{ChargeCaptured, ChargeFailed, ChargeRefunded, ChargeRequested};
use crate::provider::{ChargeOutcome, ChargeRequest, PaymentProvider};
use crate::view::{PaymentStatus, load_payment};

/// Derive the payment aggregate id from the order it pays for.
///
/// Deterministic so a replayed `OrderPlaced` lands on the same aggregate
/// instead of opening a second payment.
fn payment_id_for_order(order_id: &str) -> String {
    evento::hash_ids(vec![order_id, "payment"])
}

/// Request a charge for an order and record the provider's answer.
///
/// Returns the payment aggregate id. Calling this twice for the same order is
/// a no-op: the second call sees the existing `ChargeRequested` and returns
/// without touching the provider.
///
/// Note the ordering: `ChargeRequested` is committed *before* the provider is
/// called, so a crash mid-charge leaves the payment stuck in `requested`
/// rather than silently double-charging. Recovering those needs a reconcile
/// job against the provider — out of scope for the fake provider.
#[tracing::instrument(skip(executor, provider))]
pub async fn request_charge(
    executor: &Executor,
    provider: &Arc<dyn PaymentProvider>,
    order_id: &str,
    amount: Money,
) -> anyhow::Result<String> {
    let payment_id = payment_id_for_order(order_id);

    if executor.has_event::<ChargeRequested>(&payment_id).await? {
        tracing::info!(%payment_id, "charge already requested for this order");
        return Ok(payment_id);
    }

    evento::append(&payment_id)
        .original_version(0)
        .event(&ChargeRequested {
            order_id: order_id.to_owned(),
            amount,
            provider: provider.id().to_owned(),
        })
        .commit(executor)
        .await?;

    let outcome = provider
        .charge(ChargeRequest {
            order_id: order_id.to_owned(),
            amount,
        })
        .await;

    let mut write = evento::append(&payment_id);
    write.original_version(1);
    match outcome {
        Ok(ChargeOutcome::Captured {
            provider_charge_ref,
        }) => {
            tracing::info!(%payment_id, %provider_charge_ref, "charge captured");
            write.event(&ChargeCaptured {
                provider_charge_ref,
            });
        }
        Ok(ChargeOutcome::Declined { reason }) => {
            tracing::warn!(%payment_id, %reason, "charge declined");
            write.event(&ChargeFailed { reason });
        }
        Err(source) => {
            tracing::error!(%payment_id, error = ?source, "charge failed at the provider");
            write.event(&ChargeFailed {
                reason: source.to_string(),
            });
        }
    }
    write.commit(executor).await?;

    Ok(payment_id)
}

/// Refund a captured payment.
///
/// A payment that was never captured, or that is already refunded, is left
/// alone — the saga calls this as a compensation and may retry it.
#[tracing::instrument(skip(executor, provider))]
pub async fn refund(
    executor: &Executor,
    provider: &Arc<dyn PaymentProvider>,
    payment_id: &str,
) -> anyhow::Result<()> {
    let Some(payment) = load_payment(executor, payment_id).await? else {
        tracing::warn!("refund skipped: no such payment");
        return Ok(());
    };

    match payment.status {
        PaymentStatus::Captured => {}
        PaymentStatus::Refunded => {
            tracing::warn!("refund skipped: payment already refunded");
            return Ok(());
        }
        PaymentStatus::Requested | PaymentStatus::Failed => {
            tracing::warn!(status = ?payment.status, "refund skipped: nothing was captured");
            return Ok(());
        }
    }

    let Some(provider_charge_ref) = payment.provider_charge_ref.clone() else {
        tracing::warn!("refund skipped: captured payment has no provider reference");
        return Ok(());
    };

    provider.refund(&provider_charge_ref).await?;

    payment
        .write()?
        .event(&ChargeRefunded)
        .commit(executor)
        .await?;

    tracing::info!(%provider_charge_ref, "payment refunded");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FakePaymentProvider;
    use crate::aggregate::Payment;
    use evento::cursor::Args;
    use evento::{Aggregate as _, EventFilter, Executor as _};
    use timada_core::Currency;

    fn fake() -> Arc<dyn PaymentProvider> {
        Arc::new(FakePaymentProvider)
    }

    /// Event names committed to a payment, oldest first.
    async fn event_names(executor: &Executor, payment_id: &str) -> Vec<String> {
        let events = executor
            .read(
                Some(vec![EventFilter::by_id(
                    Payment::aggregate_type(),
                    payment_id,
                )]),
                None,
                Args::forward(20, None),
            )
            .await
            .unwrap();

        events.edges.into_iter().map(|e| e.node.name).collect()
    }

    #[tokio::test]
    async fn captures_a_charge_and_stays_idempotent_on_replay() {
        let db = crate::test_support::temp_db().await;
        let amount = Money::new(4200, Currency::Eur);

        let payment_id = request_charge(&db.ctx.executor, &fake(), "order-1", amount)
            .await
            .unwrap();
        assert_eq!(
            event_names(&db.ctx.executor, &payment_id).await,
            ["ChargeRequested", "ChargeCaptured"]
        );

        let replayed = request_charge(&db.ctx.executor, &fake(), "order-1", amount)
            .await
            .unwrap();
        assert_eq!(
            replayed, payment_id,
            "payment id must be derived from the order"
        );
        assert_eq!(
            event_names(&db.ctx.executor, &payment_id).await,
            ["ChargeRequested", "ChargeCaptured"],
            "a replayed command must not charge again"
        );
    }

    #[tokio::test]
    async fn records_a_decline_as_a_failed_charge() {
        let db = crate::test_support::temp_db().await;

        let payment_id = request_charge(
            &db.ctx.executor,
            &fake(),
            "order-2",
            Money::new(1999, Currency::Eur),
        )
        .await
        .unwrap();

        assert_eq!(
            event_names(&db.ctx.executor, &payment_id).await,
            ["ChargeRequested", "ChargeFailed"]
        );
    }

    #[tokio::test]
    async fn refunds_a_captured_payment_once() {
        let db = crate::test_support::temp_db().await;

        let payment_id = request_charge(
            &db.ctx.executor,
            &fake(),
            "order-3",
            Money::new(99_900, Currency::Eur),
        )
        .await
        .unwrap();

        refund(&db.ctx.executor, &fake(), &payment_id)
            .await
            .unwrap();
        assert_eq!(
            event_names(&db.ctx.executor, &payment_id).await,
            ["ChargeRequested", "ChargeCaptured", "ChargeRefunded"]
        );

        refund(&db.ctx.executor, &fake(), &payment_id)
            .await
            .unwrap();
        assert_eq!(
            event_names(&db.ctx.executor, &payment_id).await,
            ["ChargeRequested", "ChargeCaptured", "ChargeRefunded"],
            "refunding twice must not emit a second refund"
        );
    }

    #[tokio::test]
    async fn load_payment_maps_a_payment_back_to_its_order() {
        let db = crate::test_support::temp_db().await;
        let amount = Money::new(7350, Currency::Eur);

        let payment_id = request_charge(&db.ctx.executor, &fake(), "order-5", amount)
            .await
            .unwrap();

        let captured = load_payment(&db.ctx.executor, &payment_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(captured.id, payment_id);
        assert_eq!(captured.order_id, "order-5");
        assert_eq!(captured.amount, amount);
        assert_eq!(captured.provider, "fake");
        assert_eq!(captured.status, PaymentStatus::Captured);
        assert_eq!(captured.reason, None);
        assert!(
            captured
                .provider_charge_ref
                .as_deref()
                .unwrap_or_default()
                .starts_with("FAKE-")
        );

        refund(&db.ctx.executor, &fake(), &payment_id)
            .await
            .unwrap();

        let refunded = load_payment(&db.ctx.executor, &payment_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(refunded.status, PaymentStatus::Refunded);
        assert_eq!(refunded.order_id, "order-5");
        assert_eq!(refunded.amount, amount);
        assert_eq!(
            refunded.provider_charge_ref, captured.provider_charge_ref,
            "the charge reference must survive a refund"
        );

        assert!(
            load_payment(&db.ctx.executor, "no-such-payment")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn load_payment_reports_a_decline_with_its_reason() {
        let db = crate::test_support::temp_db().await;

        let payment_id = request_charge(
            &db.ctx.executor,
            &fake(),
            "order-6",
            Money::new(4999, Currency::Eur),
        )
        .await
        .unwrap();

        let payment = load_payment(&db.ctx.executor, &payment_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(payment.status, PaymentStatus::Failed);
        assert_eq!(payment.order_id, "order-6");
        assert_eq!(payment.provider_charge_ref, None);
        assert!(payment.reason.unwrap_or_default().contains("declined"));
    }

    #[tokio::test]
    async fn refund_is_a_no_op_for_a_failed_or_unknown_payment() {
        let db = crate::test_support::temp_db().await;

        let payment_id = request_charge(
            &db.ctx.executor,
            &fake(),
            "order-4",
            Money::new(1099, Currency::Eur),
        )
        .await
        .unwrap();

        refund(&db.ctx.executor, &fake(), &payment_id)
            .await
            .unwrap();
        assert_eq!(
            event_names(&db.ctx.executor, &payment_id).await,
            ["ChargeRequested", "ChargeFailed"]
        );

        refund(&db.ctx.executor, &fake(), "no-such-payment")
            .await
            .unwrap();
    }
}
