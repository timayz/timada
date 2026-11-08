#![allow(dead_code)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use evento::cursor::{Args, ReadResult, Value};
use evento::{
    Aggregate, AggregateEvent, EventFilter, Executor, RoutingKey, TenantFilter, WriteError,
};
use timada_core::db::Db;
use timada_tenancy::{Command, CreateTenant, TenantState};
use tokio::sync::Notify;
use ulid::Ulid;

/// A fresh in-memory event store with the read-model schema applied, and
/// the read-model database in it.
pub async fn executor_with_pool() -> (evento::Turso, Db) {
    let store = evento::Turso::in_memory_concurrent().await.unwrap();
    let mut conn = store.connect().unwrap();
    timada_tenancy::MIGRATIONS
        .to_latest(&mut conn)
        .await
        .map_err(|(_, err)| err)
        .unwrap();
    (store.clone(), Db::from(store))
}

pub async fn command() -> Command<evento::Turso> {
    Command(executor_with_pool().await.0)
}

pub fn create(name: &str, owner: &str) -> CreateTenant {
    CreateTenant {
        name: name.to_owned(),
        owner_user_id: owner.to_owned(),
    }
}

pub async fn create_tenant(cmd: &Command<evento::Turso>, name: &str, owner: &str) -> TenantState {
    cmd.create_tenant(create(name, owner)).await.unwrap()
}

pub fn new_id() -> String {
    ulid::Ulid::generate().to_string()
}

/// The gateway and the read-model database behind it.
pub async fn command_with_pool() -> (Command<evento::Turso>, Db) {
    let (executor, pool) = executor_with_pool().await;
    (Command(executor), pool)
}

/// A store whose writes can be refused or held, to replay what a crash or a
/// race does between two writes.
#[derive(Clone)]
pub struct Rigged {
    pub store: evento::Turso,
    refused: Vec<(&'static str, &'static str)>,
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
        Self {
            store,
            refused: Vec::new(),
            hold: None,
        }
    }

    /// Refuses every write of `D`, as if the node stopped right there.
    pub fn refuse<D: AggregateEvent>(mut self) -> Self {
        self.refused.push((D::aggregate_type(), D::event_name()));
        self
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
        if let Some(event) = events.iter().find(|event| {
            self.refused
                .iter()
                .any(|(kind, name)| event.aggregate_type == *kind && event.name == *name)
        }) {
            return Err(WriteError::Unknown(anyhow::anyhow!(
                "refused {}::{}",
                event.aggregate_type,
                event.name
            )));
        }
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

/// Grants whatever seats accepted invitations still owe, once.
pub async fn grant_owed_seats(store: &evento::Turso) {
    timada_tenancy::invitation_grants_subscription::<evento::Turso>(evento::Evento::new(
        store.clone(),
    ))
    .run_once(store)
    .await
    .unwrap();
}

/// The same, as another node getting there only now: its cursor, kept under
/// its own default routing key, starts at the beginning. The saga follows
/// the node's own region, so this stand-in for a node of the same region
/// reads every key: the events were written without one.
pub async fn grant_owed_seats_on(store: &evento::Turso, node: &str) {
    let executor = evento::Evento::new(store.clone()).default_routing_key(node);
    timada_tenancy::invitation_grants_subscription::<evento::Evento>(executor.clone())
        .any_routing_key()
        .run_once(&executor)
        .await
        .unwrap();
}
