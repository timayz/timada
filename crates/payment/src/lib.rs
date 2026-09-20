//! Payment bounded context: the payment attached to an order — requested with
//! a method (card or 3x installments), then captured or declined by the PSP,
//! and possibly refunded afterwards — plus the SQL lists of refunds behind the
//! admin's refunds section.
//!
//! The provider itself is a [`PaymentProvider`] the host picks
//! ([`ManualProvider`] by default): [`start_payment`] opens what the shopper
//! pays on, [`apply_provider_event`] records what the provider reports, and a
//! refund is *requested* first, handed to the provider by
//! [`run_provider_refunds`], and only a `PaymentRefunded` once confirmed.

pub mod aggregator;
mod command;
mod error;
mod migration;
mod provider;
mod provider_event;
mod query;
mod refund_execution;
mod refund_list;
mod session;
mod value_object;

pub use command::*;
pub use error::*;
pub use migration::migrations;
pub use provider::*;
pub use provider_event::*;
pub use query::*;
pub use refund_execution::*;
pub use refund_list::*;
pub use session::*;
pub use value_object::*;
