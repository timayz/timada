//! Timada returns.
//!
//! The RMA bounded context, Medusa-style: a [`Return`] tracks one delivered
//! order coming back, separate from the order itself. The order stays
//! `Delivered` throughout — its fulfillment history is what happened, and a
//! return is a new conversation about it, not a rewrite.
//!
//! Full-order returns only this pass: request → admin approves or rejects →
//! on approval the `return-flow` subscription refunds the charge through the
//! payment provider → the provider's `ChargeRefunded` event marks the return
//! refunded, which `timada-invoice`'s issuance subscription turns into a
//! credit note. Per-line returns and exchanges are named follow-ups.
//!
//! One return per order by construction ([`return_id`] derives from the order
//! id); a rejected return may be re-requested, a refunded one is terminal.
//! Requesting needs no login — the order id in the URL is the capability,
//! exactly like the public order-status page.

mod aggregate;
mod commands;
mod migrations;
mod projections;
mod routes;
mod saga;
mod state;
mod view;

pub use aggregate::{Return, ReturnApproved, ReturnRefunded, ReturnRejected, ReturnRequested};
pub use commands::{RequestReturnError, approve_return, reject_return, request_return, return_id};
pub use migrations::migrations;
pub use projections::{
    AdminReturnRow, READ_MODELS_SUBSCRIPTION, read_models_subscription, recent_returns,
    start_subscriptions,
};
pub use routes::{admin_router, store_router};
pub use saga::{RETURN_FLOW_SUBSCRIPTION, return_flow_subscription};
pub use state::{ReturnPolicy, ReturnState};
pub use view::{ReturnStatus, ReturnView, load_return};
