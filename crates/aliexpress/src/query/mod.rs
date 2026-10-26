//! Read side: [`read_model_subscription`] keeps `aliexpress_products`
//! current from every tenant's streams into one database, for the list of
//! a shop's imports and the sweep that reads them again.

mod migrations;
mod read_model;

pub use migrations::*;
pub use read_model::*;
