//! The `Product` aggregate: one sellable item, sourced from one supplier.
//!
//! The consistency boundary is a single product's lifecycle. Stock, pricing
//! rules and merchandising are deliberately outside it — this pass sells what
//! the supplier listed, at the price it listed.

use timada_core::Money;

#[evento::aggregate]
pub enum Product {
    /// Copied out of a supplier's catalog. The supplier's own reference is
    /// kept so fulfillment can order the right item later, and the descriptive
    /// fields are snapshotted rather than re-fetched.
    ProductImported {
        supplier_id: String,
        supplier_product_ref: String,
        title: String,
        description: String,
        price: Money,
        image_url: String,
    },
    /// Visible in the storefront and orderable.
    ProductPublished,
    /// Withdrawn from the storefront. Past orders keep their snapshotted lines.
    ProductArchived,
}
