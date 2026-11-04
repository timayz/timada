//! Goods a supplier sells the shop, behind a trait so the payments know
//! nothing of who that is, and tests run without one.

/// What the payments ask of whoever supplies a shop's goods.
#[async_trait::async_trait]
pub trait SuppliedGoods: Send + Sync {
    /// Brings the catalog in step with what the supplier asks now for those of
    /// `product_ids` it sells the shop, unless they were read a moment ago.
    /// Goods it cannot read are left as last read: an unknown price is not a
    /// price that moved.
    async fn read_again(&self, tenant_id: &str, product_ids: &[String]);
}
