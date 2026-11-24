#![allow(dead_code)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use evento::cursor::{Args, ReadResult, Value};
use evento::{Aggregate, EventFilter, Executor, RoutingKey, TenantFilter, WriteError};
use tokio::sync::Notify;
use ulid::Ulid;

/// A store whose first write to one aggregate type can be held, to replay
/// what a race between two writes does.
#[derive(Clone)]
pub struct Rigged {
    pub store: evento::Turso,
    hold: Option<Arc<Hold>>,
}

/// The first write to one aggregate type waits until the next write to it
/// has landed; later writes pass.
struct Hold {
    aggregate_type: &'static str,
    came: AtomicBool,
    parked: Notify,
    passed: Notify,
}

impl Rigged {
    pub fn new(store: evento::Turso) -> Self {
        Self { store, hold: None }
    }

    /// Holds the first write to an `A` stream until another write to one
    /// has landed: the two race, and the second wins.
    pub fn hold_first<A: Aggregate>(mut self) -> Self {
        self.hold = Some(Arc::new(Hold {
            aggregate_type: A::aggregate_type(),
            came: AtomicBool::new(false),
            parked: Notify::new(),
            passed: Notify::new(),
        }));
        self
    }

    /// Resolves once the write [`Self::hold_first`] names is held.
    pub async fn parked(&self) {
        if let Some(hold) = &self.hold {
            hold.parked.notified().await;
        }
    }
}

#[async_trait::async_trait]
impl Executor for Rigged {
    async fn write(&self, events: Vec<evento::Event>) -> Result<(), WriteError> {
        let Some(hold) = self.hold.as_ref().filter(|hold| {
            events
                .iter()
                .any(|event| event.aggregate_type == hold.aggregate_type)
        }) else {
            return self.store.write(events).await;
        };
        if !hold.came.swap(true, Ordering::SeqCst) {
            hold.parked.notify_one();
            hold.passed.notified().await;
            return self.store.write(events).await;
        }
        let written = self.store.write(events).await;
        hold.passed.notify_one();
        written
    }

    async fn read(
        &self,
        aggregators: Option<Arc<[EventFilter]>>,
        routing_key: Option<RoutingKey>,
        tenant: TenantFilter,
        args: Args,
        to_micros: Option<u64>,
    ) -> anyhow::Result<ReadResult<evento::Event>> {
        self.store
            .read(aggregators, routing_key, tenant, args, to_micros)
            .await
    }

    async fn latest_timestamp(
        &self,
        aggregators: Option<Arc<[EventFilter]>>,
        routing_key: Option<RoutingKey>,
        tenant: TenantFilter,
    ) -> anyhow::Result<u64> {
        self.store
            .latest_timestamp(aggregators, routing_key, tenant)
            .await
    }

    async fn get_subscriber_cursor(&self, key: String) -> anyhow::Result<Option<Value>> {
        self.store.get_subscriber_cursor(key).await
    }

    async fn is_subscriber_running(&self, key: String, worker_id: Ulid) -> anyhow::Result<bool> {
        self.store.is_subscriber_running(key, worker_id).await
    }

    async fn upsert_subscriber(&self, key: String, worker_id: Ulid) -> anyhow::Result<()> {
        self.store.upsert_subscriber(key, worker_id).await
    }

    async fn acknowledge(
        &self,
        key: String,
        worker_id: Ulid,
        cursor: Value,
        lag: u64,
    ) -> anyhow::Result<bool> {
        self.store.acknowledge(key, worker_id, cursor, lag).await
    }

    async fn get_snapshot(
        &self,
        tenant: String,
        aggregate_type: String,
        projection: String,
        aggregate_revision: String,
        id: String,
    ) -> anyhow::Result<Option<(Vec<u8>, Value)>> {
        self.store
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
        self.store
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
        self.store
            .delete_snapshot(tenant, aggregate_type, projection, id)
            .await
    }
}
