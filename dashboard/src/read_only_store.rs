//! The executor a key without `store:write` is served: the store, with
//! every way of changing it taken away, and only the streams a storefront
//! has business reading.
//!
//! - Reads are confined to an allow-list of aggregate types. A key holding
//!   `store:read` alone reads what a page that shows the catalog needs:
//!   products, categories, stock, published reviews, pages. One holding
//!   `store:checkout` too reads what selling needs on top: how the shop
//!   takes payment, ships and sells to businesses, discounts and offers,
//!   review links. Never a cart, an order or an invoice: the cart and order
//!   streams are every shopper's carts and every placed order with the
//!   addresses they carry, and an invoice series (`Book`) holds every
//!   invoice the shop issued with its buyer's name, address and e-mail, so a
//!   storefront is answered about the one cart or order whose id it holds,
//!   and about that order's documents, through the `/store/carts/*` and
//!   `/store/orders/*` endpoints (`crate::store_carts`), never the stream.
//!   Nothing else either: a tenant's store also holds its shoppers' accounts
//!   and sign-in challenges, its API keys, its mail settings, and a
//!   storefront's key must not be the way to those. A read naming nothing is
//!   narrowed to the list; one naming a type outside it fails with the
//!   reason.
//! - Appends are refused: the client's commit fails with the reason. What a
//!   shopper does (filling a cart, asking to pay) goes through the same
//!   endpoints, where the dashboard writes it in the key's tenant.
//! - Snapshots the client saves or deletes are dropped. A snapshot is state
//!   the dashboard trusts when it loads the same view, so letting a read-only
//!   client write one would be a write; dropping (rather than refusing) keeps
//!   the client's own loads working, they only lose the cache.
//!
//! The tenant is not this wrapper's concern: the connection is served in the
//! key's tenant by the store API's session, which pins every request to it
//! before it gets here, and refuses the subscriber calls (a storefront keeps
//! its cursors in a store of its own).

use std::sync::Arc;

use evento::cursor::{Args, ReadResult, Value};
use evento::{
    Aggregate, Event, EventFilter, Evento, Executor, RoutingKey, SubscriberStatus, TenantFilter,
    WriteError,
};
use ulid::Ulid;

/// The aggregate types a key holding `store:read` alone reads: what a page
/// that shows the catalog loads (`crates/storefront`'s catalog subscription
/// and the product, stock, review and page views).
pub fn catalog_reads() -> Vec<&'static str> {
    vec![
        timada_catalog::aggregator::Product::aggregate_type(),
        timada_catalog::aggregator::category::Category::aggregate_type(),
        timada_catalog::aggregator::stock::StockItem::aggregate_type(),
        timada_catalog::aggregator::stock::StockPointer::aggregate_type(),
        timada_reviews::aggregator::Review::aggregate_type(),
        timada_pages::aggregator::Page::aggregate_type(),
    ]
}

/// The aggregate types a key holding `store:checkout` reads: the catalog,
/// and what selling loads on top (`crates/storefront`'s checkout and
/// review-link paths through the orders, discounts and reviews gateways).
/// Not the carts, the orders or the invoices: see the module docs.
pub fn checkout_reads() -> Vec<&'static str> {
    let mut reads = catalog_reads();
    reads.extend([
        timada_orders::aggregator::PaymentMethod::aggregate_type(),
        timada_orders::aggregator::BusinessBuyers::aggregate_type(),
        timada_orders::aggregator::ShippingMethods::aggregate_type(),
        timada_discounts::aggregator::Discount::aggregate_type(),
        timada_discounts::aggregator::DiscountCode::aggregate_type(),
        timada_discounts::aggregator::Offers::aggregate_type(),
        timada_reviews::aggregator::ReviewLink::aggregate_type(),
    ]);
    reads
}

/// Wraps `inner`, refusing every append and reading the catalog alone;
/// keeps its default routing key.
pub fn read_only(inner: Evento) -> Evento {
    guarded(inner, catalog_reads())
}

/// Wraps `inner`, refusing every append and reading what selling needs (the
/// `store:checkout` scope); keeps its default routing key.
pub fn checkout_only(inner: Evento) -> Evento {
    guarded(inner, checkout_reads())
}

fn guarded(inner: Evento, reads: Vec<&'static str>) -> Evento {
    let routing_key = Executor::default_routing_key(&inner).map(str::to_owned);
    let executor = Evento::new(ReadOnly { inner, reads });
    match routing_key {
        Some(key) => executor.default_routing_key(key),
        None => executor,
    }
}

struct ReadOnly {
    inner: Evento,
    /// The aggregate types this connection reads.
    reads: Vec<&'static str>,
}

impl ReadOnly {
    /// Whether `aggregate_type` is on the connection's list.
    fn reads(&self, aggregate_type: &str) -> bool {
        self.reads.contains(&aggregate_type)
    }

    /// `Err` naming the type when the connection may not read it.
    fn check_read(&self, aggregate_type: &str) -> anyhow::Result<()> {
        if self.reads(aggregate_type) {
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "this API key may not read {aggregate_type} events (a storefront's key reads the catalog, and with store:checkout what selling needs)"
            ))
        }
    }

    /// The filters a read goes down with: the caller's, each checked, or the
    /// whole list when it named none.
    fn narrow(
        &self,
        aggregators: Option<Arc<[EventFilter]>>,
    ) -> anyhow::Result<Option<Arc<[EventFilter]>>> {
        match aggregators {
            Some(filters) => {
                for filter in filters.iter() {
                    self.check_read(&filter.aggregate_type)?;
                }
                Ok(Some(filters))
            }
            None => Ok(Some(
                self.reads
                    .iter()
                    .map(|aggregate_type| EventFilter {
                        aggregate_type: (*aggregate_type).to_owned(),
                        aggregate_id: None,
                        name: None,
                    })
                    .collect(),
            )),
        }
    }

    /// The refusal every append meets.
    fn check(&self, _events: &[Event]) -> Result<(), WriteError> {
        Err(WriteError::Unknown(anyhow::anyhow!(
            "this API key may not write events (it lacks the store:write scope)"
        )))
    }
}

#[async_trait::async_trait]
impl Executor for ReadOnly {
    fn default_routing_key(&self) -> Option<&str> {
        Executor::default_routing_key(&self.inner)
    }

    async fn write(&self, events: Vec<Event>) -> Result<(), WriteError> {
        self.check(&events)?;
        self.inner.write(events).await
    }

    async fn replicate(&self, events: Vec<Event>) -> Result<(), WriteError> {
        self.check(&events)?;
        self.inner.replicate(events).await
    }

    fn write_watch(&self) -> Option<tokio::sync::watch::Receiver<u64>> {
        self.inner.write_watch()
    }

    async fn stable_timestamp(&self) -> anyhow::Result<Option<u64>> {
        self.inner.stable_timestamp().await
    }

    async fn read(
        &self,
        aggregators: Option<Arc<[EventFilter]>>,
        routing_key: Option<RoutingKey>,
        tenant: TenantFilter,
        args: Args,
        to_micros: Option<u64>,
    ) -> anyhow::Result<ReadResult<Event>> {
        let aggregators = self.narrow(aggregators)?;
        self.inner
            .read(aggregators, routing_key, tenant, args, to_micros)
            .await
    }

    async fn latest_timestamp(
        &self,
        aggregators: Option<Arc<[EventFilter]>>,
        routing_key: Option<RoutingKey>,
        tenant: TenantFilter,
    ) -> anyhow::Result<u64> {
        let aggregators = self.narrow(aggregators)?;
        self.inner
            .latest_timestamp(aggregators, routing_key, tenant)
            .await
    }

    async fn get_subscriber_cursor(&self, key: String) -> anyhow::Result<Option<Value>> {
        self.inner.get_subscriber_cursor(key).await
    }

    async fn is_subscriber_running(&self, key: String, worker_id: Ulid) -> anyhow::Result<bool> {
        self.inner.is_subscriber_running(key, worker_id).await
    }

    async fn subscriber_status(
        &self,
        key: String,
        worker_id: Ulid,
    ) -> anyhow::Result<SubscriberStatus> {
        self.inner.subscriber_status(key, worker_id).await
    }

    async fn latest_version(
        &self,
        tenant: String,
        aggregate_type: String,
        aggregate_id: String,
    ) -> anyhow::Result<u16> {
        self.check_read(&aggregate_type)?;
        self.inner
            .latest_version(tenant, aggregate_type, aggregate_id)
            .await
    }

    async fn stream_routing_key(
        &self,
        tenant: String,
        aggregate_type: String,
        aggregate_id: String,
    ) -> anyhow::Result<Option<Option<String>>> {
        self.check_read(&aggregate_type)?;
        self.inner
            .stream_routing_key(tenant, aggregate_type, aggregate_id)
            .await
    }

    async fn upsert_subscriber(&self, key: String, worker_id: Ulid) -> anyhow::Result<()> {
        self.inner.upsert_subscriber(key, worker_id).await
    }

    async fn acknowledge(
        &self,
        key: String,
        worker_id: Ulid,
        cursor: Value,
        lag: u64,
    ) -> anyhow::Result<bool> {
        self.inner.acknowledge(key, worker_id, cursor, lag).await
    }

    async fn get_snapshot(
        &self,
        tenant: String,
        aggregate_type: String,
        projection: String,
        aggregate_revision: String,
        id: String,
    ) -> anyhow::Result<Option<(Vec<u8>, Value)>> {
        self.check_read(&aggregate_type)?;
        self.inner
            .get_snapshot(tenant, aggregate_type, projection, aggregate_revision, id)
            .await
    }

    async fn save_snapshot(
        &self,
        _tenant: String,
        _aggregate_type: String,
        _projection: String,
        _aggregate_revision: String,
        _id: String,
        _data: Vec<u8>,
        _cursor: Value,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn delete_snapshot(
        &self,
        _tenant: String,
        _aggregate_type: String,
        _projection: String,
        _id: String,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A checkout key reads how the shop sells, never a cart, an order or
    /// an invoice series: those are answered one at a time by the
    /// `/store/carts/*` and `/store/orders/*` endpoints.
    #[test]
    fn a_checkout_key_reads_neither_carts_nor_orders() {
        let reads = checkout_reads();
        let cart = timada_orders::aggregator::Cart::aggregate_type();
        let order = timada_orders::aggregator::Order::aggregate_type();
        let book = timada_invoicing::aggregator::Book::aggregate_type();
        let document = timada_invoicing::aggregator::Document::aggregate_type();
        assert!(!reads.contains(&cart), "{reads:?}");
        assert!(!reads.contains(&order), "{reads:?}");
        assert!(!reads.contains(&book), "{reads:?}");
        assert!(!reads.contains(&document), "{reads:?}");
        assert!(reads.contains(&timada_orders::aggregator::PaymentMethod::aggregate_type()));
        for read in catalog_reads() {
            assert!(reads.contains(&read), "{read} is catalog");
        }
        assert!(!catalog_reads().contains(&cart));
    }
}
