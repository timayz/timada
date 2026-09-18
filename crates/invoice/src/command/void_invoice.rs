use evento::{Executor, ProjectionAggregate};

use crate::{aggregator::InvoiceVoided, error::InvoiceError, value_object::InvoiceStatus};

impl<E: Executor> super::Command<'_, E> {
    /// Voids a draft or issued invoice (the number stays consumed). A no-op
    /// when already voided so compensations can be retried.
    pub async fn void_invoice(
        &self,
        id: impl Into<String>,
        reason: impl Into<String>,
    ) -> Result<(), InvoiceError> {
        let invoice = self.load_existing(id).await?;
        if invoice.status == InvoiceStatus::Voided {
            return Ok(());
        }

        let reason = reason.into();
        invoice
            .write()?
            .event(&InvoiceVoided {
                reason: reason.clone(),
            })
            .commit(self.executor)
            .await?;
        tracing::info!(invoice_id = %invoice.id, %reason, "invoice voided");
        Ok(())
    }
}
