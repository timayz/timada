use evento::Executor;
use timada_core::{Address, Money};

use crate::{
    aggregator::{OrderDiscountApplied, OrderNumberAssigned, OrderPlaced},
    error::OrderError,
    value_object::{DeliveryChoice, OrderDiscount, OrderLine, PaymentMode, Seller, order_total},
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
    /// The code typed in the cart, honoured or not.
    pub promo_code: Option<String>,
    /// What the promotion context granted for that code, already redeemed.
    pub discount: Option<OrderDiscount>,
    /// The number shown to the customer, from [`crate::allocate_order_number`];
    /// without one the order goes by its id.
    pub order_number: Option<String>,
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
        let totals = order_total(&cmd.lines, &cmd.shipping_fee, &cmd.handling_fee)?;
        if let Some(discount) = &cmd.discount {
            let max = totals.max_discount();
            discount.amount.same_currency(&max)?;
            if !discount.amount.is_positive() || discount.amount.minor > max.minor {
                return Err(OrderError::InvalidDiscount { max });
            }
        }

        let id = order_id(&cmd.cart_id);
        let mut write = evento::append(&id);
        write.routing_key_opt(routing_key).event(&OrderPlaced {
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
        });
        if let Some(order_number) = cmd.order_number.filter(|n| !n.trim().is_empty()) {
            write.event(&OrderNumberAssigned { order_number });
        }
        if let Some(discount) = cmd.discount {
            write.event(&OrderDiscountApplied {
                code: discount.code,
                kind: discount.kind,
                amount: discount.amount,
            });
        }
        let result = write.commit(self.0).await;

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
