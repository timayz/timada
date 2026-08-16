//! The `Payment` aggregate.
//!
//! One payment per order attempt. The invariant it enforces is that a charge
//! is requested at most once per order (see [`crate::request_charge`]) and
//! that only a captured, not-yet-refunded payment can be refunded.

use timada_core::Money;

#[evento::aggregate]
pub enum Payment {
    ChargeRequested {
        order_id: String,
        amount: Money,
        provider: String,
    },
    ChargeCaptured {
        provider_charge_ref: String,
    },
    ChargeFailed {
        reason: String,
    },
    ChargeRefunded,
}
