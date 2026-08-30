use timada_core::ServiceContext;
use timada_dropship::SupplierRegistry;

/// Everything the catalog routers and subscriptions need.
///
/// The registry is here for the admin import flow only — the storefront never
/// touches a supplier, it reads the snapshots taken at import time.
#[derive(Clone)]
pub struct CatalogState {
    pub ctx: ServiceContext,
    pub registry: SupplierRegistry,
}
