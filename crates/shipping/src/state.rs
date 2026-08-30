use timada_core::ServiceContext;
use timada_dropship::SupplierRegistry;

/// Everything the shipping routers and subscriptions need. The registry is here
/// because tracking is a poll: refreshing a shipment resolves its `supplier_id`
/// back to a `Supplier` implementation.
#[derive(Clone)]
pub struct ShippingState {
    pub ctx: ServiceContext,
    pub registry: SupplierRegistry,
}
