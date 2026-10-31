//! The store's write side: what must be durable before it is acknowledged
//! goes through one connection, what may be lost through another.
//!
//! Events are applied through `durable` (`synchronous = FULL`: an apply
//! must be on disk before the consensus journal, on the same connection,
//! records the transaction as applied). Subscription cursors and snapshots
//! go through `cursors` (`synchronous = NORMAL`): a cursor lost to a power
//! cut makes its subscription process a few events again, which every
//! handler allows (delivery is at-least-once), and a snapshot is a cache.
//! Twenty-odd subscriptions acknowledge every event they process; with each
//! acknowledgement an fsync on the one connection the applies and the
//! journal share, the node fell behind its own timeouts under load.
//!
//! Only `durable` wakes the subscriptions: an acknowledgement is not a new
//! event.

use std::sync::Arc;

use evento::Sqlite;
use evento::cursor::{Args, ReadResult, Value};
use evento::{
    Event, EventFilter, Executor, RoutingKey, SubscriberStatus, TenantFilter, WriteError,
};
use ulid::Ulid;

#[derive(Clone)]
pub struct StoreWriter {
    durable: Sqlite,
    cursors: Sqlite,
}

impl StoreWriter {
    pub fn new(durable: Sqlite, cursors: Sqlite) -> Self {
        Self { durable, cursors }
    }
}

#[async_trait::async_trait]
impl Executor for StoreWriter {
    fn default_routing_key(&self) -> Option<&str> {
        self.durable.default_routing_key()
    }

    async fn write(&self, events: Vec<Event>) -> Result<(), WriteError> {
        self.durable.write(events).await
    }

    async fn replicate(&self, events: Vec<Event>) -> Result<(), WriteError> {
        self.durable.replicate(events).await
    }

    fn write_watch(&self) -> Option<tokio::sync::watch::Receiver<u64>> {
        self.durable.write_watch()
    }

    async fn stable_timestamp(&self) -> anyhow::Result<Option<u64>> {
        self.durable.stable_timestamp().await
    }

    async fn read(
        &self,
        aggregators: Option<Arc<[EventFilter]>>,
        routing_key: Option<RoutingKey>,
        tenant: TenantFilter,
        args: Args,
        to_micros: Option<u64>,
    ) -> anyhow::Result<ReadResult<Event>> {
        self.durable
            .read(aggregators, routing_key, tenant, args, to_micros)
            .await
    }

    async fn latest_timestamp(
        &self,
        aggregators: Option<Arc<[EventFilter]>>,
        routing_key: Option<RoutingKey>,
        tenant: TenantFilter,
    ) -> anyhow::Result<u64> {
        self.durable
            .latest_timestamp(aggregators, routing_key, tenant)
            .await
    }

    async fn get_subscriber_cursor(&self, key: String) -> anyhow::Result<Option<Value>> {
        self.cursors.get_subscriber_cursor(key).await
    }

    async fn is_subscriber_running(&self, key: String, worker_id: Ulid) -> anyhow::Result<bool> {
        self.cursors.is_subscriber_running(key, worker_id).await
    }

    async fn subscriber_status(
        &self,
        key: String,
        worker_id: Ulid,
    ) -> anyhow::Result<SubscriberStatus> {
        self.cursors.subscriber_status(key, worker_id).await
    }

    async fn latest_version(
        &self,
        tenant: String,
        aggregate_type: String,
        aggregate_id: String,
    ) -> anyhow::Result<u16> {
        self.durable
            .latest_version(tenant, aggregate_type, aggregate_id)
            .await
    }

    async fn stream_routing_key(
        &self,
        tenant: String,
        aggregate_type: String,
        aggregate_id: String,
    ) -> anyhow::Result<Option<Option<String>>> {
        self.durable
            .stream_routing_key(tenant, aggregate_type, aggregate_id)
            .await
    }

    async fn upsert_subscriber(&self, key: String, worker_id: Ulid) -> anyhow::Result<()> {
        self.cursors.upsert_subscriber(key, worker_id).await
    }

    async fn acknowledge(
        &self,
        key: String,
        worker_id: Ulid,
        cursor: Value,
        lag: u64,
    ) -> anyhow::Result<bool> {
        self.cursors.acknowledge(key, worker_id, cursor, lag).await
    }

    async fn get_snapshot(
        &self,
        tenant: String,
        aggregate_type: String,
        projection: String,
        aggregate_revision: String,
        id: String,
    ) -> anyhow::Result<Option<(Vec<u8>, Value)>> {
        self.cursors
            .get_snapshot(tenant, aggregate_type, projection, aggregate_revision, id)
            .await
    }

    async fn save_snapshot(
        &self,
        tenant: String,
        aggregate_type: String,
        projection: String,
        aggregate_revision: String,
        id: String,
        data: Vec<u8>,
        cursor: Value,
    ) -> anyhow::Result<()> {
        self.cursors
            .save_snapshot(
                tenant,
                aggregate_type,
                projection,
                aggregate_revision,
                id,
                data,
                cursor,
            )
            .await
    }

    async fn delete_snapshot(
        &self,
        tenant: String,
        aggregate_type: String,
        projection: String,
        id: String,
    ) -> anyhow::Result<()> {
        self.cursors
            .delete_snapshot(tenant, aggregate_type, projection, id)
            .await
    }

    async fn delete_stream(
        &self,
        tenant: String,
        aggregate_type: String,
        aggregate_id: String,
    ) -> anyhow::Result<()> {
        self.durable
            .delete_stream(tenant, aggregate_type, aggregate_id)
            .await
    }
}
