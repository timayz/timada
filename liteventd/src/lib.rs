pub mod context;
pub mod sql;

use dyn_clone::DynClone;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{collections::HashMap, time::Duration};
use thiserror::Error;
use tokio::sync::RwLock;
use ulid::Ulid;

pub struct EventData<D, M> {
    pub details: Event,
    pub data: D,
    pub metadata: M,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub aggregate_id: Ulid,
    pub aggregate_type: String,
    pub version: u16,
    pub data: Vec<u8>,
    pub created_at: u32,
    pub updated_at: Option<u32>,
}

pub struct Event {
    pub id: Ulid,
    pub aggregate_id: Ulid,
    pub aggregate_type: String,
    pub version: u16,
    pub name: String,
    pub routing_key: String,
    pub data: Vec<u8>,
    pub metadata: Vec<u8>,
    pub timestamp: u32,
}

impl Event {
    pub fn to_data<D: AggregatorEvent, M>(&self) -> anyhow::Result<Option<EventData<D, M>>> {
        if D::name() != self.name {
            return Ok(None);
        }
        todo!();
    }
}

#[async_trait::async_trait]
pub trait Aggregator: Default + Serialize + DeserializeOwned + Clone {
    async fn aggregate(&mut self, event: &Event) -> anyhow::Result<()>;
    fn revision() -> &'static str;
    fn name() -> &'static str;
}

pub trait AggregatorEvent {
    fn name() -> &'static str;
}

#[async_trait::async_trait]
pub trait Executor: DynClone + Send + Sync {
    async fn get_event_routing_key(
        &self,
        aggregate_type: String,
        aggregate_id: Ulid,
    ) -> Result<Option<String>, SaveError>;
    async fn bulk_insert(&self, events: Vec<Event>) -> Result<(), SaveError>;
    async fn save_snapshot(&self, snapshot: Snapshot) -> Result<(), SaveError>;
}

dyn_clone::clone_trait_object!(Executor);

#[derive(Debug, Error)]
pub enum LoadError {}

pub struct LoadResult<A: Aggregator> {
    pub item: A,
    pub version: u16,
}

pub async fn load<A: Aggregator, E: Executor>(
    _executor: &E,
    id: Ulid,
) -> Result<LoadResult<A>, LoadError> {
    todo!()
}

#[derive(Debug, Error)]
pub enum SaveError {
    #[error("invalid original version")]
    InvalidOriginalVersion,

    #[error("{0}")]
    ServerError(#[from] anyhow::Error),

    #[error("{0}")]
    CiboriumSer(#[from] ciborium::ser::Error<std::io::Error>),
}

pub struct SaveBuilder<A: Aggregator> {
    aggregate_id: Ulid,
    aggregate_type: String,
    aggregator: A,
    routing_key: String,
    original_version: u16,
    data: Vec<(&'static str, Vec<u8>)>,
    metadata: Vec<u8>,
}

impl<A: Aggregator> SaveBuilder<A> {
    pub fn new(aggregator: A, aggregate_id: Ulid) -> SaveBuilder<A> {
        SaveBuilder {
            aggregate_id,
            aggregator,
            aggregate_type: A::name().to_owned(),
            routing_key: "default".into(),
            original_version: 0,
            data: Vec::default(),
            metadata: Vec::default(),
        }
    }

    pub fn original_version(mut self, v: u16) -> Self {
        self.original_version = v;

        self
    }

    pub fn routing_key(mut self, v: String) -> Self {
        self.routing_key = v;

        self
    }

    pub fn metadata<M: Serialize>(
        mut self,
        v: &M,
    ) -> Result<Self, ciborium::ser::Error<std::io::Error>> {
        let mut metadata = Vec::new();
        ciborium::into_writer(v, &mut metadata)?;
        self.metadata = metadata;

        Ok(self)
    }

    pub fn data<D: Serialize + AggregatorEvent>(
        mut self,
        v: &D,
    ) -> Result<Self, ciborium::ser::Error<std::io::Error>> {
        let mut data = Vec::new();
        ciborium::into_writer(v, &mut data)?;
        self.data.push((D::name(), data));

        Ok(self)
    }

    pub async fn commit<E: Executor>(&self, executor: &E) -> Result<(), SaveError> {
        let routing_key = executor
            .get_event_routing_key(self.aggregate_type.to_owned(), self.aggregate_id)
            .await?
            .unwrap_or(self.routing_key.to_owned());

        let mut aggregator = self.aggregator.clone();
        let mut version = self.original_version;

        let mut events = vec![];

        for (name, data) in &self.data {
            version += 1;

            let event = Event {
                id: Ulid::new(),
                name: name.to_string(),
                data: data.to_vec(),
                metadata: self.metadata.to_vec(),
                timestamp: 0,
                aggregate_id: self.aggregate_id,
                aggregate_type: self.aggregate_type.to_owned(),
                version,
                routing_key: routing_key.to_owned(),
            };

            aggregator.aggregate(&event).await?;
            events.push(event);
        }

        executor.bulk_insert(events).await?;

        let mut data = vec![];
        ciborium::into_writer(&aggregator, &mut data)?;
        executor
            .save_snapshot(Snapshot {
                aggregate_id: self.aggregate_id,
                aggregate_type: self.aggregate_type.to_owned(),
                version,
                data,
                created_at: 0,
                updated_at: None,
            })
            .await?;

        Ok(())
    }
}

pub fn create<A: Aggregator>(aggregator: A, aggregate_id: Ulid) -> SaveBuilder<A> {
    SaveBuilder::new(aggregator, aggregate_id)
}

pub fn save<A: Aggregator>(aggregator: LoadResult<A>, aggregate_id: Ulid) -> SaveBuilder<A> {
    SaveBuilder::new(aggregator.item, aggregate_id).original_version(aggregator.version)
}

#[derive(Debug, Error)]
pub enum SubscribeError {}

#[derive(Clone)]
pub struct ContextBase<'a, E: Executor> {
    key: String,
    event: &'a Event,
    executor: &'a E,
}

impl<'a, E: Executor> ContextBase<'a, E> {
    pub fn to_context<D: AggregatorEvent, M>(
        &self,
    ) -> anyhow::Result<Option<Context<'a, E, D, M>>> {
        todo!()
    }
}

pub struct Context<'a, E, D, M> {
    key: String,
    pub executor: &'a E,
    pub event: &'a Event,
    pub data: D,
    pub metadata: M,
}

#[async_trait::async_trait]
pub trait SubscribeHandler<E: Executor> {
    async fn handle(&self, context: &ContextBase<'_, E>) -> anyhow::Result<()>;
}

pub enum SubscribeMode {
    Persistent,
    Normal,
    Live,
}

pub struct SubscribeBuilder<E: Executor> {
    id: Ulid,
    key: String,
    routing_key: Option<String>,
    mode: SubscribeMode,
    handlers: HashMap<String, Box<dyn SubscribeHandler<E>>>,
    delay: Option<Duration>,
    started: RwLock<bool>,
}

pub fn subscribe<E: Executor>(key: impl Into<String>) -> SubscribeBuilder<E> {
    SubscribeBuilder {
        id: Ulid::new(),
        key: key.into(),
        mode: SubscribeMode::Persistent,
        handlers: HashMap::default(),
        delay: None,
        routing_key: None,
        started: RwLock::new(false),
    }
}

impl<E: Executor> SubscribeBuilder<E> {
    pub fn mode(mut self, v: SubscribeMode) -> Self {
        self.mode = v;

        self
    }

    pub fn delay(mut self, v: Duration) -> Self {
        self.delay = Some(v);

        self
    }

    pub fn handler<A: Aggregator, H: SubscribeHandler<E> + 'static>(mut self, v: H) -> Self {
        self.handlers.insert(A::name().to_owned(), Box::new(v));

        self
    }

    pub async fn start(&mut self, _executor: &E) -> Result<&mut Self, SubscribeError> {
        todo!()
    }

    pub async fn wait_finish(&mut self, _executor: &E) -> Result<(), SubscribeError> {
        todo!()
    }
}

#[derive(Debug, Error)]
pub enum AcknowledgeError {}

pub async fn acknowledge<E: Executor>(
    executor: &E,
    key: impl Into<String>,
    event: &Event,
) -> Result<(), AcknowledgeError> {
    todo!()
}
