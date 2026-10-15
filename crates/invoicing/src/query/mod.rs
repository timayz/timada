//! Read side. A document is read from its own index and book streams on its
//! page; listing a tenant's documents needs an index a relational store is
//! good at: [`read_model_subscription`] keeps `invoicing_documents` current
//! from one tenant's event stream into a shared database (every row carries
//! the tenant id).

mod documents;
mod migrations;
mod read_model;

pub use documents::*;
pub use migrations::*;
pub use read_model::*;
