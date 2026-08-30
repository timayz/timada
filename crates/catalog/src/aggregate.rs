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
    /// The price for one currency was set or replaced. The import price stays
    /// the base; each `ProductPriceSet` overrides the price in its own
    /// currency, latest event winning — how a product becomes sellable in a
    /// region whose currency the supplier does not quote.
    ProductPriceSet { price: Money },
    /// Withdrawn from the storefront. Past orders keep their snapshotted lines.
    ProductArchived,
}
