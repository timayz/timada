use evento::Executor;
use timada_core::{Address, Money};

use crate::{
    aggregator::OrderPlaced,
    error::OrderError,
    value_object::{DeliveryChoice, OrderLine, PaymentMode, Seller, order_total},
};

use super::order_id;

#[derive(Debug, Clone)]
pub struct PlaceOrder {
    pub cart_id: String,
    pub customer_id: String,
    pub seller: Seller,
    pub lines: Vec<OrderLine>,
    pub delivery_address: Address,
    pub billing_address: Address,
    pub delivery: DeliveryChoice,
    pub payment_mode: PaymentMode,
    pub shipping_fee: Money,
    pub handling_fee: Money,
    pub promo_code: Option<String>,
}

#[evento::command]
impl<E: Executor> super::Command<'_, E> {
    /// Places the order for a checked-out cart. The id is derived from the
    /// cart id, so placing the same cart twice is rejected atomically.
    pub async fn place_order(
        &self,
        cmd: PlaceOrder,
        routing_key: Option<String>,
    ) -> Result<String, OrderError> {
        if cmd.cart_id.trim().is_empty() {
            return Err(OrderError::Required("cart_id"));
        }
        if cmd.customer_id.trim().is_empty() {
            return Err(OrderError::Required("customer_id"));
        }
        if cmd.lines.is_empty() {
            return Err(OrderError::NoLines);
        }
        if cmd.delivery.method_code.trim().is_empty() {
            return Err(OrderError::Required("delivery.method_code"));
        }
        if cmd.shipping_fee.is_negative() || cmd.handling_fee.is_negative() {
            return Err(OrderError::Required("non-negative fees"));
        }
        cmd.delivery_address.validate()?;
        cmd.billing_address.validate()?;
        // Rejects mixed currencies and overflow before anything is written.
        order_total(&cmd.lines, &cmd.shipping_fee, &cmd.handling_fee)?;

        let id = order_id(&cmd.cart_id);
        let result = evento::append(&id)
            .routing_key_opt(routing_key)
            .event(&OrderPlaced {
                cart_id: cmd.cart_id.clone(),
                customer_id: cmd.customer_id,
                seller: cmd.seller,
                lines: cmd.lines,
                delivery_address: cmd.delivery_address,
                billing_address: cmd.billing_address,
                delivery: cmd.delivery,
                payment_mode: cmd.payment_mode,
                shipping_fee: cmd.shipping_fee,
                handling_fee: cmd.handling_fee,
                promo_code: cmd.promo_code,
            })
            .commit(self.0)
            .await;

        match result {
            Ok(id) => {
                tracing::info!(order_id = %id, cart_id = %cmd.cart_id, "order placed");
                Ok(id)
            }
            Err(evento::WriteError::InvalidOriginalVersion) => {
                Err(OrderError::AlreadyPlaced(cmd.cart_id))
            }
            Err(err) => Err(err.into()),
        }
    }
}
