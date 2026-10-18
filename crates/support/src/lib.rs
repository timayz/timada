//! Support bounded context: the messages shoppers send the shop through its
//! contact form.
//!
//! A message is what a visitor wrote, signed with their name and the address
//! to answer at; the merchant reads it in the back office and marks it
//! handled once answered (by mail, outside the platform). Messages live in
//! the tenant's own event store and the crate is pure domain code: no
//! permission checks, no HTTP, no rate limiting (whoever takes the form
//! decides how often one visitor may send). The name, address and text are
//! recorded as written, like an order's shipping details.

pub mod aggregator;
mod command;
mod error;
mod query;
mod value_object;

pub use command::*;
pub use error::*;
pub use query::*;
pub use value_object::*;
