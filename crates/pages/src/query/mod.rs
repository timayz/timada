//! Read side. Listing a tenant's pages uses `pages`, which
//! [`read_model_subscription`] keeps current from every tenant's streams
//! into one database (every row carries the tenant id).

mod migrations;
mod pages;
mod read_model;

pub use migrations::*;
pub use pages::*;
pub use read_model::*;
