//! Inventory bounded context: on-hand and reserved quantities per product and
//! location (warehouse or store), plus "alerte disponibilité" back-in-stock
//! alerts. Reservations are keyed by order id so the fulfillment saga can
//! retry safely.

pub mod aggregator;
mod command;
mod error;
mod migration;
mod query;
mod read_model;
mod stock_list;
mod value_object;

pub use command::*;
pub use error::*;
pub use migration::migrations;
pub use query::*;
pub use read_model::*;
pub use stock_list::*;
pub use value_object::*;
