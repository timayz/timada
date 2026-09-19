//! Order bounded context: the placed order (an immutable snapshot of what was
//! bought, where it ships and how it is paid), its status, the order-history
//! read model, the anti-corruption layer that turns a cart checkout into an
//! order, and the `order-fulfillment` saga orchestrating inventory, payment
//! and shipping.

pub mod aggregator;
mod command;
mod error;
mod migration;
mod numbering;
mod process;
mod query;
mod read_model;
mod saga;
mod value_object;

pub use command::*;
pub use error::*;
pub use migration::migrations;
pub use numbering::allocate_order_number;
pub use process::*;
pub use query::*;
pub use read_model::*;
pub use saga::*;
pub use value_object::*;
