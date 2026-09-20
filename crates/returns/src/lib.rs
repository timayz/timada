//! Returns bounded context ("retours", RMA): the way back for an order that
//! already shipped and can no longer be cancelled.
//!
//! A customer asks to send lines of a shipped order back within the
//! [`ReturnPolicy`] window; an operator approves or refuses; once the parcel
//! is in, the operator records what was accepted, whether each line goes
//! back into stock, and how the customer is refunded. From there the
//! `return-processing` process manager restocks, refunds the original
//! payment and/or issues store credit, and completes the return — every step
//! idempotent, so a retry after a crash converges.
//!
//! The way back may come with a **prepaid label** ([`ReturnLabelProvider`],
//! or attached by hand): free when the shop is at fault
//! ([`ReturnGround::shop_at_fault`]), otherwise at the policy's flat fee,
//! taken off the refund.
//!
//! How many units of an order line may still be returned is a contended,
//! cross-aggregate counter: it lives in write-side SQL (`return_claim`),
//! like the RMA number sequence.

pub mod aggregator;
mod command;
mod error;
mod label;
mod migration;
mod process;
mod query;
mod return_list;
mod value_object;

pub use command::*;
pub use error::*;
pub use label::*;
pub use migration::migrations;
pub use process::*;
pub use query::*;
pub use return_list::*;
pub use value_object::*;
