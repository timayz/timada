//! Read side. Listing a tenant's subscribers uses `newsletter_subscribers`,
//! which [`read_model_subscription`] keeps current from one tenant's event
//! stream into a shared database (every row carries the tenant id).

mod migrations;
mod read_model;
mod subscribers;

pub use migrations::*;
pub use read_model::*;
pub use subscribers::*;
