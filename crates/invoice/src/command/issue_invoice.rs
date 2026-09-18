use evento::{Executor, ProjectionAggregate};

use crate::{aggregator::InvoiceIssued, error::InvoiceError, value_object::InvoiceStatus};

impl<E: Executor> super::Command<'_, E> {
    /// Numbers the invoice. Idempotent: an issued invoice returns its number,
    /// and the SQL row keyed by order id survives a crash between the number
    /// allocation and the event append, so a retry gets the same number.
    pub async fn issue_invoice(&self, id: impl Into<String>) -> Result<String, InvoiceError> {
        let invoice = self.load_existing(id).await?;
        match invoice.status {
            InvoiceStatus::Issued => {
                return invoice
                    .invoice_number
                    .ok_or_else(|| anyhow::anyhow!("issued invoice without a number").into());
            }
            InvoiceStatus::Voided => return Err(InvoiceError::InvoiceVoided),
            InvoiceStatus::Draft => {}
        }

        let number = self.allocate_number(&invoice.order_id).await?;
        let year = timada_core::time::year_of(timada_core::time::now_unix_secs()?);
        let invoice_number = format!("F{year}-{number:06}");

        invoice
            .write()?
            .event(&InvoiceIssued {
                invoice_number: invoice_number.clone(),
            })
            .commit(self.executor)
            .await?;
        tracing::info!(invoice_id = %invoice.id, %invoice_number, "invoice issued");
        Ok(invoice_number)
    }

    /// Next number in the sequence, or the one already given to this order.
    /// `BEGIN IMMEDIATE` serialises allocators; `INSERT OR IGNORE` keeps a
    /// retry from taking a second number.
    async fn allocate_number(&self, order_id: &str) -> Result<i64, InvoiceError> {
        let mut conn = self.db.acquire().await?;
        sqlx::query("BEGIN IMMEDIATE").execute(&mut *conn).await?;

        let allocated: Result<i64, sqlx::Error> = async {
            sqlx::query(
                "INSERT OR IGNORE INTO invoice_number (order_id, number)
                 SELECT ?, COALESCE(MAX(number), 0) + 1 FROM invoice_number",
            )
            .bind(order_id)
            .execute(&mut *conn)
            .await?;
            sqlx::query_scalar::<_, i64>("SELECT number FROM invoice_number WHERE order_id = ?")
                .bind(order_id)
                .fetch_one(&mut *conn)
                .await
        }
        .await;

        match allocated {
            Ok(number) => {
                sqlx::query("COMMIT").execute(&mut *conn).await?;
                Ok(number)
            }
            Err(err) => {
                if let Err(rollback) = sqlx::query("ROLLBACK").execute(&mut *conn).await {
                    tracing::warn!(error = %rollback, "invoice number rollback failed");
                }
                Err(err.into())
            }
        }
    }
}
