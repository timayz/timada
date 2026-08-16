//! The anti-corruption layer between Timada and an external dropshipping
//! supplier.
//!
//! Every supplier — mock, AliExpress, whatever comes next — is reached only
//! through [`Supplier`], so the domain never learns a vendor's vocabulary.

use timada_core::Money;

/// A product as the supplier describes it, before it is imported into the
/// catalog as a Timada `Product`.
#[derive(Debug, Clone)]
pub struct SupplierProduct {
    /// The supplier's own identifier for this product.
    pub supplier_product_ref: String,
    pub title: String,
    pub description: String,
    pub price: Money,
    pub image_url: String,
}

/// One line of an order forwarded to a supplier.
///
/// Also travels inside `SupplierOrderPlaced`, hence the bitcode derives.
#[derive(Debug, Clone, PartialEq, Eq, Default, bitcode::Encode, bitcode::Decode)]
pub struct SupplierLine {
    pub supplier_product_ref: String,
    /// Snapshotted at order time so the record stays readable if the supplier
    /// renames or delists the product.
    pub title: String,
    pub quantity: u32,
}

/// What we hand the supplier when forwarding an order.
#[derive(Debug, Clone)]
pub struct SupplierOrderRequest {
    pub order_id: String,
    pub lines: Vec<SupplierLine>,
}

/// The supplier's acknowledgement of a forwarded order.
#[derive(Debug, Clone)]
pub struct SupplierConfirmation {
    /// The supplier's own order reference; shipping polls tracking with it.
    pub external_ref: String,
}

/// Where a forwarded order is in the supplier's fulfillment pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrackingStatus {
    Pending,
    Dispatched {
        tracking_number: String,
        carrier: String,
    },
    Delivered,
}

#[derive(Debug, thiserror::Error)]
pub enum SupplierError {
    #[error("supplier does not implement this operation")]
    NotImplemented,
    #[error("unknown supplier: {0}")]
    UnknownSupplier(String),
    #[error("supplier api error: {0}")]
    Api(String),
}

#[async_trait::async_trait]
pub trait Supplier: Send + Sync {
    /// Stable identifier persisted on catalog products, cart lines and order
    /// lines — changing it orphans existing data.
    fn id(&self) -> &'static str;

    /// Browse the supplier's catalog. An empty `query` means "everything".
    async fn search_products(&self, query: &str) -> Result<Vec<SupplierProduct>, SupplierError>;

    async fn place_order(
        &self,
        req: &SupplierOrderRequest,
    ) -> Result<SupplierConfirmation, SupplierError>;

    async fn track_shipment(&self, external_ref: &str) -> Result<TrackingStatus, SupplierError>;
}
