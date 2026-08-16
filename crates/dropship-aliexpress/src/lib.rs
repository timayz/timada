//! AliExpress [`Supplier`] adapter — stub.
//!
//! The AliExpress Open Platform dropshipping API requires an approved
//! developer account and app credentials. Until that integration lands, every
//! method returns [`SupplierError::NotImplemented`] so the adapter can be
//! registered and exercised end-to-end without panicking.

use timada_dropship::{
    Supplier, SupplierConfirmation, SupplierError, SupplierOrderRequest, SupplierProduct,
    TrackingStatus,
};

#[derive(Debug, Default)]
pub struct AliExpressSupplier;

impl AliExpressSupplier {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl Supplier for AliExpressSupplier {
    fn id(&self) -> &'static str {
        "aliexpress"
    }

    async fn search_products(&self, query: &str) -> Result<Vec<SupplierProduct>, SupplierError> {
        tracing::warn!(query, "AliExpress adapter is not implemented yet");
        Err(SupplierError::NotImplemented)
    }

    async fn place_order(
        &self,
        req: &SupplierOrderRequest,
    ) -> Result<SupplierConfirmation, SupplierError> {
        tracing::warn!(order_id = %req.order_id, "AliExpress adapter is not implemented yet");
        Err(SupplierError::NotImplemented)
    }

    async fn track_shipment(&self, external_ref: &str) -> Result<TrackingStatus, SupplierError> {
        tracing::warn!(external_ref, "AliExpress adapter is not implemented yet");
        Err(SupplierError::NotImplemented)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn every_method_reports_not_implemented() {
        let supplier = AliExpressSupplier::new();
        assert_eq!(supplier.id(), "aliexpress");
        assert!(matches!(
            supplier.search_products("lamp").await,
            Err(SupplierError::NotImplemented)
        ));
        let req = SupplierOrderRequest {
            order_id: "o1".into(),
            lines: Vec::new(),
        };
        assert!(matches!(
            supplier.place_order(&req).await,
            Err(SupplierError::NotImplemented)
        ));
        assert!(matches!(
            supplier.track_shipment("ref").await,
            Err(SupplierError::NotImplemented)
        ));
    }
}
