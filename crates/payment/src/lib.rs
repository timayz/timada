//! Payment bounded context: the payment attached to an order — requested with
//! a method (card or 3x installments), then captured or declined by the PSP,
//! and possibly refunded afterwards — plus the SQL list of refunds behind the
//! admin's refunds section.

pub mod aggregator;
mod command;
mod error;
mod migration;
mod query;
mod refund_list;
mod value_object;

pub use command::*;
pub use error::*;
pub use migration::migrations;
pub use query::*;
pub use refund_list::*;
pub use value_object::*;
