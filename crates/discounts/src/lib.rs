//! Discounts bounded context: the codes a merchant hands out, and how often
//! each one has been used.
//!
//! A code takes a percentage or a fixed amount off the items of a whole
//! order. It may be limited in time, in the number of orders it serves and to
//! one order per shopper e-mail. Like the catalog, it lives in the tenant's
//! own event store and the crate is pure domain code: no permission checks,
//! no HTTP.
//!
//! A code is unique within the tenant through an index aggregate with a
//! deterministic id (the same create-unless-exists as a product slug).
//! Archiving a discount gives its code back.
//!
//! Uses are counted where they cannot overrun: a code with a cap keeps one
//! usage stream, and every checkout reserves its use there with an optimistic
//! write before the shopper is asked to pay; a code good once per e-mail keeps
//! one small stream per shopper the same way. A reservation the payment never
//! confirms lapses on its own (its expiry is read, never scheduled), and the
//! order the payment makes consumes it for good.

pub mod aggregator;
mod command;
mod error;
mod query;
mod value_object;

pub use command::*;
pub use error::*;
pub use query::*;
pub use value_object::*;
