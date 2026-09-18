//! Shipping bounded context: one shipment per order, plus the static
//! delivery-method catalogue (home delivery carriers and store pickup).

pub mod aggregator;
mod command;
mod error;
mod query;
mod value_object;

pub use command::*;
pub use error::*;
pub use query::*;
pub use value_object::*;
