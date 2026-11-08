//! The catalog's read model as a source for the quote a shopper is shown
//! ([`QuoteSource`]): the products, their combinations, what is on the
//! shelf and the shop's delivery time, read from the tables the catalog
//! subscription keeps, a moment behind the store and at no cost to it.

use timada_catalog::{DeliveryDays, Sellable};
use timada_core::db::Db;

use crate::command::QuoteSource;

/// The catalog as the read model has it, for one tenant.
pub struct CatalogReadModel<'a> {
    db: &'a Db,
    tenant_id: &'a str,
}

impl<'a> CatalogReadModel<'a> {
    pub fn new(db: &'a Db, tenant_id: &'a str) -> Self {
        Self { db, tenant_id }
    }
}

impl QuoteSource for CatalogReadModel<'_> {
    async fn sellable(&self, product_id: &str) -> anyhow::Result<Option<Sellable>> {
        timada_catalog::sellable(self.db, self.tenant_id, product_id).await
    }

    /// What is on the shelf: the holds of other checkouts are not in the
    /// read model, and the payment's exact check takes them off.
    async fn available(
        &self,
        _cart_id: &str,
        product_id: &str,
        variant_id: Option<&str>,
    ) -> anyhow::Result<Option<u32>> {
        timada_catalog::on_hand(self.db, self.tenant_id, product_id, variant_id).await
    }

    async fn default_delivery_days(&self) -> anyhow::Result<Option<DeliveryDays>> {
        timada_catalog::default_delivery_days(self.db, self.tenant_id).await
    }
}
