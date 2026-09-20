//! Catalog bounded context: what a product *is* — name, brand, descriptions,
//! technical specs, media and energy label — and the category tree products
//! are filed under.
//! Prices live in `timada-pricing`, stock in `timada-inventory`.

pub mod aggregator;
mod category_adoption;
mod category_list;
mod command;
mod error;
mod migration;
mod query;
mod read_model;
mod value_object;

pub use category_adoption::*;
pub use category_list::*;
pub use command::*;
pub use error::*;
pub use migration::migrations;
pub use query::*;
pub use read_model::*;
pub use value_object::*;
