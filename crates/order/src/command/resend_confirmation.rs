use evento::{Executor, ProjectionAggregate};

use crate::{aggregator::OrderConfirmationResent, error::OrderError, value_object::OrderStatus};

impl<E: Executor> super::Command<'_, E> {
    /// "Renvoyer le mail de confirmation": records the request; the mailer
    /// reacts to the event.
    pub async fn resend_confirmation(&self, id: impl Into<String>) -> Result<(), OrderError> {
        let order = self.load_existing(id).await?;
        if order.status == OrderStatus::Cancelled {
            return Err(OrderError::Cancelled);
        }

        order
            .write()?
            .event(&OrderConfirmationResent)
            .commit(self.0)
            .await?;
        Ok(())
    }
}
