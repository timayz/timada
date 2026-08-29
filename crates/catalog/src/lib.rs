//! Timada catalog.
//!
//! A [`Product`] enters the catalog by being *imported* from a supplier — the
//! catalog never invents products, it snapshots what a [`Supplier`] described
//! at import time so a later rename or delisting upstream cannot rewrite what
//! the storefront and past orders show. From there the only two business
//! actions are publishing (making it visible in the storefront) and archiving
//! (taking it back out).
//!
//! Reads never touch the aggregate: the storefront and admin pages query the
//! SQL read models maintained by the [`read_models_subscription`], while
//! [`load_product`] replays the event stream for callers that need
//! read-your-own-write consistency (the cart snapshots product data that way).
//!
//! [`Supplier`]: timada_dropship::Supplier

mod aggregate;
mod commands;
mod migrations;
mod projections;
mod routes;
mod state;
mod view;

pub use aggregate::{Product, ProductArchived, ProductImported, ProductPriceSet, ProductPublished};
pub use commands::{
    archive_product, import_product, product_id, publish_product, set_product_price,
};
pub use migrations::migrations;
pub use projections::{READ_MODELS_SUBSCRIPTION, read_models_subscription, start_subscriptions};
pub use routes::{admin_router, store_router};
pub use state::CatalogState;
pub use view::{ProductView, load_product};
