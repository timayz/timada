//! Cart bounded context: the shopping cart (lines with a price snapshot taken
//! when added, promo code entry, saved carts) and the checkout fact that the
//! order context consumes.

pub mod aggregator;
mod command;
mod error;
mod query;
mod value_object;

pub use command::*;
pub use error::*;
pub use query::*;
pub use value_object::*;
