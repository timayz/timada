use evento::Executor;
use timada_core::Money;

use crate::{aggregator::PaymentRequested, error::PaymentError, value_object::PaymentMethod};

use super::payment_id;

#[derive(Debug, Clone)]
pub struct RequestPayment {
    pub order_id: String,
    pub amount: Money,
    pub method: PaymentMethod,
}

#[evento::command]
impl<E: Executor> super::Command<'_, E> {
    /// Requests the payment of an order. The id is derived from the order id,
    /// so a retry (the fulfillment saga may replay) returns the existing id.
    pub async fn request_payment(
        &self,
        cmd: RequestPayment,
        routing_key: Option<String>,
    ) -> Result<String, PaymentError> {
        if !cmd.amount.is_positive() {
            return Err(PaymentError::InvalidAmount);
        }
        if let PaymentMethod::Installments { count, fee } = &cmd.method {
            if !(2..=4).contains(count) {
                return Err(PaymentError::InvalidMethod(
                    "installment count must be 2 to 4",
                ));
            }
            if fee.is_negative() {
                return Err(PaymentError::InvalidMethod(
                    "installment fee must not be negative",
                ));
            }
            cmd.amount.same_currency(fee)?;
        }

        let id = payment_id(&cmd.order_id);
        let result = evento::append(&id)
            .routing_key_opt(routing_key)
            .event(&PaymentRequested {
                order_id: cmd.order_id.clone(),
                amount: cmd.amount,
                method: cmd.method,
            })
            .commit(self.0)
            .await;

        match result {
            Ok(id) => {
                tracing::info!(payment_id = %id, order_id = %cmd.order_id, "payment requested");
                Ok(id)
            }
            Err(evento::WriteError::InvalidOriginalVersion) => Ok(id),
            Err(err) => Err(err.into()),
        }
    }
}
