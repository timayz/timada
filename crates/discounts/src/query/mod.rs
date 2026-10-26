//! Read side. A discount's own stream is read directly on its page (so an
//! edit never waits on a projection). Listing a tenant's discounts uses
//! `discounts_codes`, which [`read_model_subscription`] keeps current from every
//! tenant's streams into one database (every row carries the tenant id).

mod discounts;
mod migrations;
mod read_model;

pub use discounts::*;
pub use migrations::*;
pub use read_model::*;
