use timada_core::ServiceContext;

use crate::registry::SupplierRegistry;

/// Everything the dropship routers and subscriptions need.
#[derive(Clone)]
pub struct DropshipState {
    pub ctx: ServiceContext,
    pub registry: SupplierRegistry,
}
