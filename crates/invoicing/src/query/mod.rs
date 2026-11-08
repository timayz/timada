//! Read side. A document is read whole from its own index stream where a
//! page draws it (the PDF); listing documents, of a tenant or of an order,
//! needs an index a relational store is good at: [`read_model_subscription`] keeps `invoicing_documents` current
//! from every tenant's streams into one database (every row carries the
//! tenant id).

mod documents;
mod migrations;
mod read_model;

pub use documents::*;
pub use migrations::*;
pub use read_model::*;
