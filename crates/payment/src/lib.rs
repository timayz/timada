//! Payment bounded context: the payment attached to an order — requested with
//! a method (card or 3x installments), then captured or declined by the PSP,
//! and possibly refunded afterwards.

pub mod aggregator;
mod command;
mod error;
mod query;
mod value_object;

pub use command::*;
pub use error::*;
pub use query::*;
pub use value_object::*;
