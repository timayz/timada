pub mod context;
pub mod sql;

use serde::{Serialize, de::DeserializeOwned};
use std::{collections::HashMap, time::Duration};
use thiserror::Error;
use tokio::sync::RwLock;
use ulid::Ulid;

pub struct EventData<D, M> {
    pub details: Event,
    pub data: D,
    pub metadata: M,
}

#[derive(Debug, Clone)]
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
    pub fn to_data<D: AggregatorEvent + DeserializeOwned, M: DeserializeOwned>(
        &self,
    ) -> Result<Option<EventData<D, M>>, ciborium::de::Error<std::io::Error>> {
        if D::name() != self.name {
            return Ok(None);
        }

        let data = ciborium::from_reader(&self.data[..])?;
        let metadata = ciborium::from_reader(&self.metadata[..])?;

        Ok(Some(EventData {
            data,
            metadata,
            details: self.clone(),
        }))
    }
}

#[async_trait::async_trait]
pub trait Aggregator: Default + Send + Sync + Serialize + DeserializeOwned + Clone {
    async fn aggregate(&mut self, event: &Event) -> anyhow::Result<()>;
    fn revision() -> &'static str;
    fn name() -> &'static str;
}

pub trait AggregatorEvent {
    fn name() -> &'static str;
}

#[async_trait::async_trait]
pub trait Executor: Send + Sync {
    async fn write<A: Aggregator>(
        &self,
        aggregator: &A,
        events: Vec<Event>,
    ) -> Result<(), WriteError>;
}

#[derive(Debug, Error)]
pub enum ReadError {}

pub struct LoadResult<A: Aggregator> {
    pub item: A,
    pub version: u16,
    pub routing_key: String,
}

pub async fn load<A: Aggregator, E: Executor>(
    _executor: &E,
    id: Ulid,
) -> Result<LoadResult<A>, ReadError> {
    todo!()
}

#[derive(Debug, Error)]
pub enum WriteError {
    #[error("invalid original version")]
    InvalidOriginalVersion,

    #[error("not data")]
    NoData,

    #[error("{0}")]
    ServerError(#[from] anyhow::Error),

    #[error("ciborium >> {0}")]
    CiboriumSer(#[from] ciborium::ser::Error<std::io::Error>),

    #[error("sqlx >> {0}")]
    Sqlx(#[from] sqlx::Error),
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

    pub async fn commit<E: Executor>(&self, executor: &E) -> Result<(), WriteError> {
        if self.data.is_empty() {
            return Err(WriteError::NoData);
        }

        let routing_key = self.routing_key.to_owned();
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

        executor.write(&aggregator, events).await?;

        Ok(())
    }
}

pub fn create<A: Aggregator>(aggregator: A, aggregate_id: Ulid) -> SaveBuilder<A> {
    SaveBuilder::new(aggregator, aggregate_id)
}

pub fn save<A: Aggregator>(aggregator: LoadResult<A>, aggregate_id: Ulid) -> SaveBuilder<A> {
    SaveBuilder::new(aggregator.item, aggregate_id)
        .original_version(aggregator.version)
        .routing_key(aggregator.routing_key)
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
