use timada_core::Money;

use crate::value_object::PaymentMethod;

// The explicit name pins the on-disk identity: renaming the crate or the enum
// must never orphan stored events.
#[evento::aggregate(name = "timada-payment/Payment")]
pub enum Payment {
    /// A payment was requested for an order.
    PaymentRequested {
        order_id: String,
        amount: Money,
        method: PaymentMethod,
    },

    /// The PSP confirmed the capture.
    PaymentCaptured { psp_reference: String },

    /// The PSP refused the payment.
    PaymentDeclined { reason: String },

    /// Part or all of the captured amount was returned to the customer.
    PaymentRefunded { amount: Money, reason: String },
}
