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

    /// Part or all of the captured amount was returned to the customer: the
    /// provider confirmed it. Since refunds are asked for first
    /// ([`RefundRequested`]), it is committed together with [`RefundSettled`].
    PaymentRefunded { amount: Money, reason: String },

    /// The shop decided to give money back; nothing has moved yet. The same
    /// `refund_id` is requested again when a failed refund is retried.
    RefundRequested {
        refund_id: String,
        amount: Money,
        reason: String,
    },

    /// Companion of [`PaymentRefunded`]: which request it settles, and the
    /// provider's own reference for the refund.
    RefundSettled {
        refund_id: String,
        psp_refund_reference: String,
    },

    /// The provider refused the refund for good; its amount is no longer held.
    RefundFailed { refund_id: String, reason: String },

    /// The cardholder contested the charge with their bank (a chargeback, or
    /// the inquiry that comes before one). `dispute_id` is the provider's own
    /// reference; `respond_by` is when the shop's evidence is due, when the
    /// provider says. While it is open no refund is handed to the provider.
    DisputeOpened {
        dispute_id: String,
        amount: Money,
        reason: String,
        respond_by: Option<u64>,
    },

    /// The bank sided with the shop (or the inquiry was closed): the money
    /// stays.
    DisputeWon { dispute_id: String },

    /// The bank sided with the cardholder: the disputed amount went back to
    /// them, and can no longer be refunded.
    DisputeLost { dispute_id: String },
}
