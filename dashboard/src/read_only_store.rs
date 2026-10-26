//! The executor a key without `store:write` is served: the store, with
//! every way of changing it taken away, or all but one.
//!
//! - Appends are refused: the client's commit fails with the reason. A key
//!   holding `store:checkout` may append what a shopper does (carts, placing
//!   an order: [`timada_orders::is_shopper_event`]) and nothing else; one
//!   event outside that list refuses the whole append.
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
    Event, EventFilter, Evento, Executor, RoutingKey, SubscriberStatus, TenantFilter, WriteError,
};
use ulid::Ulid;

/// What a guarded connection may append.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Appends {
    Nothing,
    ShopperEvents,
}

/// Wraps `inner`, refusing every append; keeps its default routing key.
pub fn read_only(inner: Evento) -> Evento {
    guarded(inner, Appends::Nothing)
}

/// Wraps `inner`, letting through only what a shopper writes (the
/// `store:checkout` scope); keeps its default routing key.
pub fn checkout_only(inner: Evento) -> Evento {
    guarded(inner, Appends::ShopperEvents)
}

fn guarded(inner: Evento, appends: Appends) -> Evento {
    let routing_key = Executor::default_routing_key(&inner).map(str::to_owned);
    let executor = Evento::new(ReadOnly { inner, appends });
    match routing_key {
        Some(key) => executor.default_routing_key(key),
        None => executor,
    }
}

struct ReadOnly {
    inner: Evento,
    appends: Appends,
}

impl ReadOnly {
    /// Lets an append through when every event of it is allowed.
    fn check(&self, events: &[Event]) -> Result<(), WriteError> {
        match self.appends {
            Appends::Nothing => Err(WriteError::Unknown(anyhow::anyhow!(
                "this API key may not write events (it lacks the store:write scope)"
            ))),
            Appends::ShopperEvents => match events
                .iter()
                .find(|e| !timada_orders::is_shopper_event(&e.aggregate_type, &e.name))
            {
                None => Ok(()),
                Some(event) => Err(WriteError::Unknown(anyhow::anyhow!(
                    "this API key may only fill carts and place orders (store:checkout), not write {}::{}",
                    event.aggregate_type,
                    event.name
                ))),
            },
        }
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
