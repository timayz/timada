//! Read side. A tenant's own stream is read directly on its pages (so a
//! creation never waits on a projection). Listing a user's tenants, every
//! tenant for the administration, or the invitations of a tenant (one stream
//! each) needs an index a relational store is good at:
//! [`read_model_subscription`] keeps the SQLite tables current from the event
//! stream, and [`tenants`] / [`invitations`] / [`api_keys`] query them.

mod api_keys;
mod companies;
mod invitations;
mod migrations;
mod read_model;
mod tenants;

pub use api_keys::*;
pub use companies::*;
pub use invitations::*;
pub use migrations::*;
pub use read_model::*;
pub use tenants::*;
