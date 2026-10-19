//! Read side. Listing a tenant's subscribers and campaigns uses
//! `newsletter_subscribers`, `newsletter_campaigns` and
//! `newsletter_deliveries`, which [`read_model_subscription`] keeps current from one tenant's event
//! stream into a shared database (every row carries the tenant id).

mod campaigns;
mod migrations;
mod read_model;
mod subscribers;

pub use campaigns::*;
pub use migrations::*;
pub use read_model::*;
pub use subscribers::*;
