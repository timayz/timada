//! Read side. A shopper's own stream is read directly when they sign in.
//! Listing a tenant's shoppers uses `shoppers_shoppers`, which
//! [`read_model_subscription`] keeps current from one tenant's event stream
//! into a shared database (every row carries the tenant id).

mod migrations;
mod read_model;
mod shoppers;

pub use migrations::*;
pub use read_model::*;
pub use shoppers::*;
