//! Read side: [`read_model_subscription`] keeps `aliexpress_products`
//! current from every tenant's streams into one database, for the list of
//! a shop's imports and the sweep that reads them again; (in
//! [`read_times`]) when this node last read each of them, which no event
//! records; (in [`deliveries`]) what delivering each last cost, which a
//! routine read reuses; and (in [`unrecorded`]) what this node placed on
//! AliExpress and could not record yet.

pub mod deliveries;
mod migrations;
mod read_model;
pub mod read_times;
mod unrecorded;

pub use deliveries::*;
pub use migrations::*;
pub use read_model::*;
pub use read_times::*;
pub use unrecorded::*;
