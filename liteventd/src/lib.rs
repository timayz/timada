pub mod context;

use dyn_clone::DynClone;
use serde::{Serialize, de::DeserializeOwned};
use std::{
    collections::{HashMap, HashSet},
    sync::mpsc::Receiver,
    time::Duration,
};
use thiserror::Error;
use tokio::sync::RwLock;
use ulid::Ulid;

pub struct EventData<D, M> {
    pub details: Event,
    pub data: D,
    pub metadata: M,
}

pub struct Event {
    pub id: Ulid,
    pub aggregate_id: Ulid,
    pub aggregate_type: String,
    pub name: String,
    pub routing_key: Option<String>,
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
pub trait Aggregator: Default + Serialize + DeserializeOwned {
    async fn aggregate(&mut self, event: Event) -> anyhow::Result<()>;
    fn revision() -> &'static str;
    fn name() -> &'static str;
}

pub trait AggregatorEvent {
    fn name() -> &'static str;
}

#[async_trait::async_trait]
pub trait Executor: DynClone + Send + Sync {
    async fn test(&self) {}
}

dyn_clone::clone_trait_object!(Executor);

#[derive(Clone)]
pub struct SqlExecutor();

#[async_trait::async_trait()]
impl Executor for SqlExecutor {
    async fn test(&self) {
        todo!()
    }
}

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

#[derive(Debug, Error, PartialEq)]
pub enum SaveError {
    #[error("invalid original version")]
    InvalidOriginalVersion,
}

pub struct SaveBuilder<A: Aggregator> {
    aggregate_id: Ulid,
    aggregate_type: String,
    aggregator: A,
    routing_key: Option<String>,
    original_version: u16,
    data: Vec<(&'static str, Vec<u8>)>,
    metadata: Vec<u8>,
}

impl<A: Aggregator> SaveBuilder<A> {
    pub fn original_version(mut self, v: u16) -> Self {
        self.original_version = v;

        self
    }

    pub fn routing_key(mut self, v: String) -> Self {
        self.routing_key = Some(v);

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

    pub async fn commit<E: Executor>(&self, _executor: &E) -> Result<(), SaveError> {
        todo!()
    }
}

pub fn save_aggregator<A: Aggregator>(aggregator: A, aggregate_id: Ulid) -> SaveBuilder<A> {
    SaveBuilder {
        aggregate_id,
        aggregator,
        aggregate_type: A::name().to_owned(),
        routing_key: None,
        original_version: 0,
        data: Vec::default(),
        metadata: Vec::default(),
    }
}

pub fn save<A: Aggregator>(aggregator: LoadResult<A>, aggregate_id: Ulid) -> SaveBuilder<A> {
    save_aggregator(aggregator.item, aggregate_id).original_version(aggregator.version)
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

impl<'a, E: Executor, D, M> Context<'a, E, D, M> {
    pub fn acknowledge(&self) {
        todo!()
    }
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
