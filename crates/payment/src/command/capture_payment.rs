use evento::{Executor, ProjectionAggregate};

use crate::{aggregator::PaymentCaptured, error::PaymentError, value_object::PaymentStatus};

impl<E: Executor> super::Command<E> {
    pub async fn capture_payment(
        &self,
        id: impl Into<String>,
        psp_reference: String,
    ) -> Result<(), PaymentError> {
        let payment = self.load_existing(id).await?;
        if payment.status != PaymentStatus::Requested {
            return Err(PaymentError::NotRequested);
        }

        payment
            .write()?
            .event(&PaymentCaptured { psp_reference })
            .commit(&self.0)
            .await?;
        tracing::info!(payment_id = %payment.id, "payment captured");
        Ok(())
    }
}
