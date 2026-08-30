//! Runtime lookup of [`Supplier`] implementations by id.
//!
//! Orders carry a `supplier_id` snapshotted at import time; the fulfillment
//! saga and shipping's tracking refresh resolve it back to an implementation
//! here.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::supplier::{Supplier, SupplierError};

/// Immutable, cheaply cloneable set of suppliers, keyed by [`Supplier::id`].
#[derive(Clone, Default)]
pub struct SupplierRegistry(Arc<BTreeMap<&'static str, Arc<dyn Supplier>>>);

impl SupplierRegistry {
    pub fn builder() -> SupplierRegistryBuilder {
        SupplierRegistryBuilder::default()
    }

    pub fn get(&self, id: &str) -> Result<Arc<dyn Supplier>, SupplierError> {
        self.0
            .get(id)
            .cloned()
            .ok_or_else(|| SupplierError::UnknownSupplier(id.to_owned()))
    }

    /// Registered supplier ids, alphabetically — what the admin page lists.
    pub fn ids(&self) -> Vec<&'static str> {
        self.0.keys().copied().collect()
    }
}

#[derive(Default)]
pub struct SupplierRegistryBuilder(BTreeMap<&'static str, Arc<dyn Supplier>>);

impl SupplierRegistryBuilder {
    /// Registering a second supplier under an id already taken replaces the
    /// first.
    pub fn register(mut self, supplier: Arc<dyn Supplier>) -> Self {
        self.0.insert(supplier.id(), supplier);
        self
    }

    pub fn build(self) -> SupplierRegistry {
        SupplierRegistry(Arc::new(self.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::MockSupplier;

    #[test]
    fn resolves_registered_suppliers_and_rejects_unknown_ids() {
        let registry = SupplierRegistry::builder()
            .register(Arc::new(MockSupplier::default()))
            .build();

        assert_eq!(registry.ids(), vec!["mock"]);
        assert_eq!(registry.get("mock").unwrap().id(), "mock");

        match registry.get("aliexpress") {
            Err(SupplierError::UnknownSupplier(id)) => assert_eq!(id, "aliexpress"),
            Err(other) => panic!("unexpected error: {other}"),
            Ok(_) => panic!("unregistered supplier must not resolve"),
        }
    }

    #[test]
    fn empty_registry_lists_nothing() {
        assert!(SupplierRegistry::default().ids().is_empty());
    }
}
