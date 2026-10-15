//! The headless storefront library: what a merchant's own topcoat app depends
//! on to sell from its timada store.
//!
//! [`Store::connect`] opens the connection to the store API with the tenant's
//! key, keeps a local copy of the published catalog current in the app's own
//! SQLite database, and [`RouterBuilderStoreExt::store`] puts it in the app
//! context. Pages then read the catalog with [`products`] and
//! [`require_product`] inside their own markup; the one component shipped,
//! [`price`], renders a formatted price and nothing else: every class and
//! attribute comes from the merchant.
//!
//! Events stay on the store; only the subscription's cursor, the projection
//! snapshots and the product rows live in the local database (one per replica
//! of the app), so replicas never fence each other out and nothing accumulates
//! on the store.
//!
//! The library declares no page of its own: a library's routes must be added
//! by the host (`#[page]` discovery is the host's), so the only HTTP endpoint,
//! `GET {prefix}/status`, is registered explicitly under the prefix the host
//! chooses.

mod cart;
mod components;
mod config;
mod context;
mod invoice;
mod migrations;
mod query;
mod read_model;
mod router;
mod store;

pub use cart::*;
pub use components::*;
pub use config::*;
pub use context::*;
pub use invoice::*;
pub use migrations::*;
pub use query::*;
pub use read_model::*;
pub use router::*;
pub use store::*;
