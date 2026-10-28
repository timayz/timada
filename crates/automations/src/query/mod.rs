//! Read side. Listing a tenant's scenarios uses `automations_scenarios`,
//! which [`read_model_subscription`] keeps current from every tenant's
//! streams into one database (every row carries the tenant id).

mod migrations;
mod read_model;
mod scenarios;

pub use migrations::*;
pub use read_model::*;
pub use scenarios::*;
