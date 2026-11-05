//! Goods a supplier sells the shop, behind a trait so the payments know
//! nothing of who that is, and tests run without one.

use timada_orders::OrderLine;

/// What the payments ask of whoever supplies a shop's goods.
#[async_trait::async_trait]
pub trait SuppliedGoods: Send + Sync {
    /// Brings the catalog in step with what the supplier asks now for those of
    /// `product_ids` it sells the shop, unless they were read a moment ago.
    /// Goods it cannot read are left as last read: an unknown price is not a
    /// price that moved.
    async fn read_again(&self, tenant_id: &str, product_ids: &[String]);
    /// Who buys some of `lines` for the shop once the order is placed
    /// (`aliexpress`), if anyone: the money is then taken once they are
    /// bought. `None`: the shop holds them all, and the money is taken at
    /// once.
    async fn supplier_of(
        &self,
        tenant_id: &str,
        lines: &[OrderLine],
    ) -> anyhow::Result<Option<String>>;
}
