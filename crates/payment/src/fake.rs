//! Zero-config provider used by the demo store.

use crate::provider::{ChargeOutcome, ChargeRequest, PaymentError, PaymentProvider};

/// Captures every charge, except amounts whose minor units end in `99` — those
/// decline deterministically so the order saga's compensation path can be
/// walked from the UI without any provider configuration.
#[derive(Debug, Default, Clone)]
pub struct FakePaymentProvider;

#[async_trait::async_trait]
impl PaymentProvider for FakePaymentProvider {
    fn id(&self) -> &'static str {
        "fake"
    }

    async fn charge(&self, req: ChargeRequest) -> Result<ChargeOutcome, PaymentError> {
        if req.amount.amount_cents % 100 == 99 {
            tracing::info!(order_id = %req.order_id, amount = %req.amount, "declined charge");
            return Ok(ChargeOutcome::Declined {
                reason: "card declined (demo trigger: amount ends in 99 cents)".to_owned(),
            });
        }

        let provider_charge_ref = format!("FAKE-{}", timada_core::new_id());
        tracing::info!(
            order_id = %req.order_id,
            amount = %req.amount,
            %provider_charge_ref,
            "captured charge"
        );
        Ok(ChargeOutcome::Captured {
            provider_charge_ref,
        })
    }

    async fn refund(&self, provider_charge_ref: &str) -> Result<(), PaymentError> {
        tracing::info!(%provider_charge_ref, "refunded charge");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use timada_core::{Currency, Money};

    fn request(amount_cents: i64) -> ChargeRequest {
        ChargeRequest {
            order_id: "order-1".to_owned(),
            amount: Money::new(amount_cents, Currency::Eur),
        }
    }

    #[tokio::test]
    async fn captures_ordinary_amounts_with_a_provider_reference() {
        let outcome = FakePaymentProvider.charge(request(1250)).await.unwrap();
        let ChargeOutcome::Captured {
            provider_charge_ref,
        } = outcome
        else {
            panic!("expected a capture, got {outcome:?}");
        };
        assert!(provider_charge_ref.starts_with("FAKE-"));
    }

    #[tokio::test]
    async fn declines_amounts_ending_in_99_cents() {
        let outcome = FakePaymentProvider.charge(request(1999)).await.unwrap();
        let ChargeOutcome::Declined { reason } = outcome else {
            panic!("expected a decline, got {outcome:?}");
        };
        assert!(reason.contains("declined"));
    }

    #[tokio::test]
    async fn refund_always_succeeds() {
        FakePaymentProvider.refund("FAKE-123").await.unwrap();
    }
}
