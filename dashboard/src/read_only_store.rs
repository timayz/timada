//! The executor a key without `store:write` is served: the tenant's own, with
//! every way of changing the tenant's store taken away.
//!
//! - Appends are refused: the client's commit fails with the reason.
//! - Snapshots the client saves or deletes are dropped. A snapshot is state
//!   the dashboard trusts when it loads the same view, so letting a read-only
//!   client write one would be a write; dropping (rather than refusing) keeps
//!   the client's own loads working, they only lose the cache.
//! - Subscriber cursors live under a name of the key's own, so a client
//!   running a subscription named like one of the tenant's cannot fence out
//!   or move the tenant's worker. The client sees its cursors as it wrote
//!   them.

use std::sync::Arc;

use evento::cursor::{Args, ReadResult, Value};
use evento::{Event, EventFilter, Evento, Executor, RoutingKey, SubscriberStatus, WriteError};
use ulid::Ulid;

/// Wraps `inner` for the key `key_id`; keeps its default routing key.
pub fn read_only(inner: Evento, key_id: &str) -> Evento {
    let routing_key = Executor::default_routing_key(&inner).map(str::to_owned);
    let executor = Evento::new(ReadOnly {
        inner,
        cursor_prefix: format!("store-api/{key_id}/"),
    });
    match routing_key {
        Some(key) => executor.default_routing_key(key),
        None => executor,
    }
}

struct ReadOnly {
    inner: Evento,
    cursor_prefix: String,
}

impl ReadOnly {
    fn cursor_key(&self, key: String) -> String {
        format!("{}{key}", self.cursor_prefix)
    }

    fn refused() -> WriteError {
        WriteError::Unknown(anyhow::anyhow!(
            "this API key may not write events (it lacks the store:write scope)"
        ))
    }
}

#[async_trait::async_trait]
impl Executor for ReadOnly {
    fn default_routing_key(&self) -> Option<&str> {
        Executor::default_routing_key(&self.inner)
    }

    async fn write(&self, _events: Vec<Event>) -> Result<(), WriteError> {
        Err(Self::refused())
    }

    async fn replicate(&self, _events: Vec<Event>) -> Result<(), WriteError> {
        Err(Self::refused())
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
        args: Args,
        to_micros: Option<u64>,
    ) -> anyhow::Result<ReadResult<Event>> {
        self.inner
            .read(aggregators, routing_key, args, to_micros)
            .await
    }

    async fn latest_timestamp(
        &self,
        aggregators: Option<Arc<[EventFilter]>>,
        routing_key: Option<RoutingKey>,
    ) -> anyhow::Result<u64> {
        self.inner.latest_timestamp(aggregators, routing_key).await
    }

    async fn get_subscriber_cursor(&self, key: String) -> anyhow::Result<Option<Value>> {
        self.inner.get_subscriber_cursor(self.cursor_key(key)).await
    }

    async fn is_subscriber_running(&self, key: String, worker_id: Ulid) -> anyhow::Result<bool> {
        self.inner
            .is_subscriber_running(self.cursor_key(key), worker_id)
            .await
    }

    async fn subscriber_status(
        &self,
        key: String,
        worker_id: Ulid,
    ) -> anyhow::Result<SubscriberStatus> {
        self.inner
            .subscriber_status(self.cursor_key(key), worker_id)
            .await
    }

    async fn latest_version(
        &self,
        aggregate_type: String,
        aggregate_id: String,
    ) -> anyhow::Result<u16> {
        self.inner
            .latest_version(aggregate_type, aggregate_id)
            .await
    }

    async fn stream_routing_key(
        &self,
        aggregate_type: String,
        aggregate_id: String,
    ) -> anyhow::Result<Option<Option<String>>> {
        self.inner
            .stream_routing_key(aggregate_type, aggregate_id)
            .await
    }

    async fn upsert_subscriber(&self, key: String, worker_id: Ulid) -> anyhow::Result<()> {
        self.inner
            .upsert_subscriber(self.cursor_key(key), worker_id)
            .await
    }

    async fn acknowledge(
        &self,
        key: String,
        worker_id: Ulid,
        cursor: Value,
        lag: u64,
    ) -> anyhow::Result<bool> {
        self.inner
            .acknowledge(self.cursor_key(key), worker_id, cursor, lag)
            .await
    }

    async fn get_snapshot(
        &self,
        aggregate_type: String,
        projection: String,
        aggregate_revision: String,
        id: String,
    ) -> anyhow::Result<Option<(Vec<u8>, Value)>> {
        self.inner
            .get_snapshot(aggregate_type, projection, aggregate_revision, id)
            .await
    }

    async fn save_snapshot(
        &self,
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
        _aggregate_type: String,
        _projection: String,
        _id: String,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}
