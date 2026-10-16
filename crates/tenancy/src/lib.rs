//! Tenancy bounded context: the tenants (merchants) of the platform and who
//! belongs to them.
//!
//! A tenant is a catalog entry in the platform's event store; its own data
//! lives in a database of its own that the host opens on demand (the host's
//! concern, not this crate's). The crate is pure domain code: it knows
//! nothing about files, consensus groups or HTTP.
//!
//! The URL handle of a tenant (`slug`) is unique through an index aggregate
//! with a deterministic id, as the identity context does for e-mail
//! addresses: appending its first event at version 0 is an atomic
//! create-unless-exists.

pub mod aggregator;
mod command;
mod error;
mod mail_template;
mod query;
mod value_object;

pub use command::*;
pub use error::*;
pub use mail_template::*;
pub use query::*;
pub use value_object::*;
