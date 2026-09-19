use evento::Executor;
use timada_core::Money;

use crate::{
    aggregator::CreditNoteIssued, error::InvoiceError, query::load_credit_note,
    value_object::InvoiceStatus,
};

use super::credit_note_id;

#[derive(Debug, Clone)]
pub struct IssueCreditNote {
    /// The refund being documented; one credit note per refund.
    pub refund_id: String,
    pub invoice_id: String,
    pub amount: Money,
    pub reason: String,
}

impl<E: Executor> super::Command<'_, E> {
    /// Credits part or all of an issued invoice and returns the credit note's
    /// number. Idempotent per refund: the SQL row keyed by the refund id
    /// survives a crash between the number allocation and the event append,
    /// and the id is derived from it, so a retry gets the same note.
    pub async fn issue_credit_note(&self, cmd: IssueCreditNote) -> Result<String, InvoiceError> {
        if cmd.refund_id.trim().is_empty() {
            return Err(InvoiceError::Required("refund_id"));
        }
        if !cmd.amount.is_positive() {
            return Err(InvoiceError::InvalidCreditAmount);
        }
        let invoice = self.load_existing(&cmd.invoice_id).await?;
        let invoice_number = match (invoice.status, &invoice.invoice_number) {
            (InvoiceStatus::Issued, Some(number)) => number.clone(),
            (InvoiceStatus::Voided, _) => return Err(InvoiceError::InvoiceVoided),
            _ => return Err(InvoiceError::InvoiceNotIssued),
        };
        cmd.amount.same_currency(&invoice.total)?;

        let id = credit_note_id(&cmd.refund_id);
        if let Some(existing) = load_credit_note(self.executor, &id).await? {
            return Ok(existing.credit_note_number);
        }

        let number = self
            .allocate_credit_note_number(&cmd, invoice.total.minor)
            .await?;
        let year = timada_core::time::year_of(timada_core::time::now_unix_secs()?);
        let credit_note_number = format!("A{year}-{number:06}");

        let result = evento::append(&id)
            .event(&CreditNoteIssued {
                credit_note_number: credit_note_number.clone(),
                refund_id: cmd.refund_id,
                invoice_id: invoice.id.clone(),
                invoice_number,
                order_id: invoice.order_id.clone(),
                amount: cmd.amount,
                reason: cmd.reason,
            })
            .commit(self.executor)
            .await;
        match result {
            Ok(_) => {
                tracing::info!(invoice_id = %invoice.id, %credit_note_number, "credit note issued");
                Ok(credit_note_number)
            }
            // Lost the race with a concurrent delivery of the same refund.
            Err(evento::WriteError::InvalidOriginalVersion) => Ok(credit_note_number),
            Err(err) => Err(err.into()),
        }
    }

    /// Next number in the credit note sequence, or the one this refund already
    /// holds. The same `BEGIN IMMEDIATE` transaction checks that the invoice's
    /// credit notes never add up to more than its total.
    async fn allocate_credit_note_number(
        &self,
        cmd: &IssueCreditNote,
        invoice_total_minor: i64,
    ) -> Result<i64, InvoiceError> {
        let mut conn = self.db.acquire().await?;
        sqlx::query("BEGIN IMMEDIATE").execute(&mut *conn).await?;

        let allocated: Result<Option<i64>, sqlx::Error> = async {
            let credited: i64 = sqlx::query_scalar(
                "SELECT COALESCE(SUM(amount_minor), 0) FROM credit_note_number
                 WHERE invoice_id = ? AND refund_id != ?",
            )
            .bind(&cmd.invoice_id)
            .bind(&cmd.refund_id)
            .fetch_one(&mut *conn)
            .await?;
            if credited + cmd.amount.minor > invoice_total_minor {
                return Ok(None);
            }
            sqlx::query(
                "INSERT OR IGNORE INTO credit_note_number
                    (refund_id, number, invoice_id, amount_minor)
                 SELECT ?, COALESCE(MAX(number), 0) + 1, ?, ? FROM credit_note_number",
            )
            .bind(&cmd.refund_id)
            .bind(&cmd.invoice_id)
            .bind(cmd.amount.minor)
            .execute(&mut *conn)
            .await?;
            sqlx::query_scalar::<_, i64>(
                "SELECT number FROM credit_note_number WHERE refund_id = ?",
            )
            .bind(&cmd.refund_id)
            .fetch_one(&mut *conn)
            .await
            .map(Some)
        }
        .await;

        match allocated {
            Ok(Some(number)) => {
                sqlx::query("COMMIT").execute(&mut *conn).await?;
                Ok(number)
            }
            Ok(None) => {
                sqlx::query("ROLLBACK").execute(&mut *conn).await?;
                Err(InvoiceError::CreditExceedsInvoice)
            }
            Err(err) => {
                if let Err(rollback) = sqlx::query("ROLLBACK").execute(&mut *conn).await {
                    tracing::warn!(error = %rollback, "credit note number rollback failed");
                }
                Err(err.into())
            }
        }
    }
}
