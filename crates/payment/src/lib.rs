//! Timada payments.
//!
//! The write side is the [`Payment`] aggregate: one instance per order,
//! keyed deterministically off the order id so a replayed saga step cannot
//! open a second payment. Charges are attempted through the
//! [`PaymentProvider`] port — [`FakePaymentProvider`] is the demo
//! implementation; a Stripe adapter would slot in behind the same trait.
//!
//! There are two read paths. [`load_payment`] replays the event stream on
//! demand — the fulfillment saga uses it to map a payment id back to its
//! order. The `admin_payment_list` SQL projection, fed by the `payment-admin`
//! subscription and served by [`admin_router`], is eventually consistent and
//! only backs the admin page.

mod admin;
mod aggregate;
mod commands;
mod fake;
mod migrations;
mod projection;
mod provider;
mod state;
mod view;

#[cfg(test)]
mod test_support;

pub use admin::admin_router;
pub use aggregate::{ChargeCaptured, ChargeFailed, ChargeRefunded, ChargeRequested, Payment};
pub use commands::{refund, request_charge};
pub use fake::FakePaymentProvider;
pub use migrations::migrations;
pub use projection::{admin_subscription, start_subscriptions};
pub use provider::{ChargeOutcome, ChargeRequest, PaymentError, PaymentProvider};
pub use state::PaymentState;
pub use view::{PaymentStatus, PaymentView, load_payment};
