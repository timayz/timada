//! Read side. A tenant's own stream is read directly on its pages (so a
//! creation never waits on a projection). Listing a user's tenants, or every
//! tenant for the administration, needs an index a relational store is good
//! at: [`read_model_subscription`] keeps two SQLite tables current from the
//! event stream, and [`tenants`] queries them.

mod migrations;
mod read_model;
mod tenants;

pub use migrations::*;
pub use read_model::*;
pub use tenants::*;
