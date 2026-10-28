//! Read side. Listing a tenant's scenarios and their runs uses
//! `automations_scenarios` and `automations_runs`, which
//! [`read_model_subscription`] keeps current from every tenant's streams
//! into one database (every row carries the tenant id). What starts runs is
//! `automations_listeners`, kept by the subscription that starts them
//! ([`listener_subscription`]).

mod checkouts;
mod listeners;
mod migrations;
mod read_model;
mod runs;
mod scenarios;

pub use checkouts::*;
pub use listeners::*;
pub use migrations::*;
pub use read_model::*;
pub use runs::*;
pub use scenarios::*;
