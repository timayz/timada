//! Read side. A product's own stream is read directly on its page (so an edit
//! never waits on a projection). Listing a tenant's products needs an index a
//! relational store is good at: [`read_model_subscription`] keeps
//! `catalog_products` current from one tenant's event stream into a shared
//! database (every row carries the tenant id), and [`products`] queries it.

mod categories;
mod migrations;
mod products;
mod read_model;

pub use categories::*;
pub use migrations::*;
pub use products::*;
pub use read_model::*;
