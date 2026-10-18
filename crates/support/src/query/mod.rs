//! Read side. Listing a tenant's messages uses `support_messages`, which
//! [`read_model_subscription`] keeps current from one tenant's event stream
//! into a shared database (every row carries the tenant id).

mod messages;
mod migrations;
mod read_model;

pub use messages::*;
pub use migrations::*;
pub use read_model::*;
