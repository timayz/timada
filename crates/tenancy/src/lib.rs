//! Tenancy bounded context: the tenants (merchants) of the platform and who
//! belongs to them.
//!
//! A tenant is a catalog entry in the platform's own streams (the default
//! tenant of the store); its own data lives in the same store under its
//! tenant id, which the host names on every call (the host's concern, not
//! this crate's). The crate is pure domain code: it knows nothing about
//! files or HTTP.
//!
//! The URL handle of a tenant (`slug`) is unique through an index aggregate
//! with a deterministic id, as the identity context does for e-mail
//! addresses: appending its first event at version 0 is an atomic
//! create-unless-exists.

pub mod aggregator;
mod command;
mod error;
mod languages;
mod mail_template;
mod query;
mod value_object;

pub use command::*;
pub use error::*;
pub use languages::*;
pub use mail_template::*;
pub use query::*;
pub use value_object::*;
