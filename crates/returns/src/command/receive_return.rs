use evento::{Executor, ProjectionAggregate};
use timada_core::Money;
use timada_order::load_order_details;
use timada_payment::{PaymentStatus, load_payment, payment_id};

use crate::{
    aggregator::ReturnReceived,
    error::ReturnError,
    value_object::{ReceivedLine, RefundMethod, ReturnStatus, refund_split},
};

#[derive(Debug, Clone)]
pub struct ReceiveReturn {
    /// What was found in the parcel; a requested line left out counts as
    /// nothing accepted.
    pub lines: Vec<ReceivedLine>,
    pub refund_method: RefundMethod,
}

impl<E: Executor> super::Command<'_, E> {
    /// Records the parcel: what is taken back, what goes into stock again and
    /// how the customer is refunded. The amounts are settled here, once —
    /// what the original payment cannot take any more (it was partly refunded
    /// already, or the order was paid with a voucher) becomes store credit —
    /// so the process manager only has to execute them.
    pub async fn receive_return(
        &self,
        id: impl Into<String>,
        cmd: ReceiveReturn,
    ) -> Result<(), ReturnError> {
        let request = self.load_existing(id).await?;
        if request.status == ReturnStatus::Received || request.status == ReturnStatus::Completed {
            return Ok(());
        }
        request.expect_status(ReturnStatus::Approved)?;

        let mut lines = Vec::with_capacity(request.lines.len());
        for requested in &request.lines {
            let found = cmd
                .lines
                .iter()
                .find(|l| l.product_id == requested.product_id);
            let accepted = found.map_or(0, |l| l.accepted);
            if accepted > requested.quantity {
                return Err(ReturnError::AcceptedExceedsRequested(
                    requested.product_id.clone(),
                ));
            }
            lines.push(ReceivedLine {
                product_id: requested.product_id.clone(),
                accepted,
                restock: accepted > 0 && found.is_some_and(|l| l.restock),
            });
        }
        if let Some(stray) = cmd
            .lines
            .iter()
            .find(|l| !request.lines.iter().any(|r| r.product_id == l.product_id))
        {
            return Err(ReturnError::UnknownLine(stray.product_id.clone()));
        }

        let order = load_order_details(self.executor, &request.order_id)
            .await?
            .ok_or(ReturnError::OrderNotFound)?;
        let accepted: Vec<(&Money, u32)> = request
            .lines
            .iter()
            .zip(&lines)
            .map(|(requested, received)| (&requested.unit_price, received.accepted))
            .collect();
        let mut split = refund_split(&order, &accepted, cmd.refund_method)?;

        // The payment can only give back what it still holds — refunds on
        // their way to the provider are as good as gone.
        let refundable = match load_payment(self.executor, payment_id(&request.order_id))
            .await?
            .filter(|p| matches!(p.status, PaymentStatus::Captured))
        {
            Some(payment) => payment.refundable()?.minor.max(0),
            None => 0,
        };
        if split.money.minor > refundable {
            let overflow = Money::new(split.money.minor - refundable, &split.money.currency);
            split.money = Money::new(refundable, &split.money.currency);
            split.credit = split.credit.checked_add(&overflow)?;
        }

        request
            .write()?
            .event(&ReturnReceived {
                lines: lines.clone(),
                refund_method: cmd.refund_method,
                money: split.money,
                credit: split.credit,
            })
            .commit(self.executor)
            .await?;

        // Units not taken back may be returned again later.
        for line in &lines {
            sqlx::query(
                "UPDATE return_claim SET quantity = ? WHERE return_id = ? AND product_id = ?",
            )
            .bind(line.accepted)
            .bind(&request.id)
            .bind(&line.product_id)
            .execute(&self.db)
            .await?;
        }
        sqlx::query("DELETE FROM return_claim WHERE return_id = ? AND quantity = 0")
            .bind(&request.id)
            .execute(&self.db)
            .await?;
        tracing::info!(return_id = %request.id, "return received");
        Ok(())
    }
}
