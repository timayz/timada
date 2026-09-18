use evento::{Executor, ProjectionAggregate};

use crate::{aggregator::PaymentDeclined, error::PaymentError, value_object::PaymentStatus};

impl<E: Executor> super::Command<E> {
    pub async fn decline_payment(
        &self,
        id: impl Into<String>,
        reason: String,
    ) -> Result<(), PaymentError> {
        let payment = self.load_existing(id).await?;
        if payment.status != PaymentStatus::Requested {
            return Err(PaymentError::NotRequested);
        }

        payment
            .write()?
            .event(&PaymentDeclined { reason })
            .commit(&self.0)
            .await?;
        tracing::info!(payment_id = %payment.id, "payment declined");
        Ok(())
    }
}
