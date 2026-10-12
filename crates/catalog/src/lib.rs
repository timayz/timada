//! Catalog bounded context: the products a merchant sells.
//!
//! The first context that lives in a tenant's own event store rather than the
//! platform's: every product stream is written through the tenant's executor,
//! so a tenant's catalog is isolated by construction and the storefront that
//! holds the tenant's API key sees nothing else. The crate is pure domain code:
//! no permission checks (the host decides who may edit), no HTTP.
//!
//! A product's URL handle (`slug`) is unique within the tenant through an index
//! aggregate with a deterministic id, like a tenant's slug in the tenancy
//! context: appending its first event at version 0 is an atomic
//! create-unless-exists. Archiving a product gives its slug back.

pub mod aggregator;
mod command;
mod error;
mod query;
mod value_object;

pub use command::*;
pub use error::*;
pub use query::*;
pub use value_object::*;
