//! Catalog bounded context: what a product *is* — name, brand, category
//! breadcrumb, descriptions, technical specs, media and energy label.
//! Prices live in `timada-pricing`, stock in `timada-inventory`.

pub mod aggregator;
mod command;
mod error;
mod migration;
mod query;
mod read_model;
mod value_object;

pub use command::*;
pub use error::*;
pub use migration::migrations;
pub use query::*;
pub use read_model::*;
pub use value_object::*;
