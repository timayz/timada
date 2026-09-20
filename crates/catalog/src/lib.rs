//! Catalog bounded context: what a product *is* — name, brand, descriptions,
//! technical specs, media and energy label — and the category tree products
//! are filed under. It also keeps what the storefront lists
//! ([`search_listing`]): the one place where the catalog reads the price, the
//! stock and the rating of its products from the contexts that own them.
//! Prices live in `timada-pricing`, stock in `timada-inventory`.

pub mod aggregator;
mod category_adoption;
mod category_list;
mod command;
mod error;
mod listing;
mod migration;
mod query;
mod read_model;
mod value_object;

pub use category_adoption::*;
pub use category_list::*;
pub use command::*;
pub use error::*;
pub use listing::*;
pub use migration::migrations;
pub use query::*;
pub use read_model::*;
pub use value_object::*;
