//! Orders bounded context: the carts shoppers fill and the orders they place.
//!
//! Lives in a tenant's own event store next to the catalog. A storefront
//! writes the shopper's side (a cart, then the order it becomes) through the
//! store API; the merchant moves the order along from the dashboard (paid,
//! fulfilled, cancelled). The crate is pure domain code: no permission checks
//! and no HTTP.
//!
//! An order keeps what was sold as it was sold: each line copies the
//! product's name, the variant, its code and its price at the moment the
//! order is placed, so a later change to the catalog never rewrites an order.
//! The order of a cart has a deterministic id, so placing it twice lands on
//! the same stream. Its short reference (`K7Q-4MZP`) is unique within the
//! tenant through an index aggregate, like a product's slug.

pub mod aggregator;
mod command;
mod error;
mod query;
mod value_object;

pub use command::*;
pub use error::*;
pub use query::*;
pub use value_object::*;
