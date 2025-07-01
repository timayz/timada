use std::{collections::HashSet, sync::mpsc::Receiver, time::Duration};

use serde::{Serialize, de::DeserializeOwned};
use thiserror::Error;
use ulid::Ulid;

pub struct EventDetail<D, M> {
    pub detail: Event,
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
    pub fn to_detail<D: AggregatorEvent, M>(&self) -> anyhow::Result<Option<EventDetail<D, M>>> {
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

pub trait Executor: Clone + Send + Sync {
    fn test(&self) {}
}

#[derive(Clone)]
pub struct SqlExecutor();

impl Executor for SqlExecutor {
    fn test(&self) {
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

#[async_trait::async_trait]
pub trait SubscribeHandler {
    async fn handle<E: Executor>(&self, executor: &E, event: &Event) -> anyhow::Result<()>;
}

pub enum SubscribeMode {
    Persistent,
    Normal,
    Live,
}

pub struct SubscribeBuilder {
    id: Ulid,
    key: String,
    routing_key: Option<String>,
    mode: SubscribeMode,
    aggregators: HashSet<String>,
    delay: Option<Duration>,
}

pub fn subscribe(key: impl Into<String>) -> SubscribeBuilder {
    SubscribeBuilder {
        id: Ulid::new(),
        key: key.into(),
        mode: SubscribeMode::Persistent,
        aggregators: HashSet::default(),
        delay: None,
        routing_key: None,
    }
}

impl SubscribeBuilder {
    pub fn mode(mut self, v: SubscribeMode) -> Self {
        self.mode = v;

        self
    }

    pub fn delay(mut self, v: Duration) -> Self {
        self.delay = Some(v);

        self
    }

    pub fn aggregator<A: Aggregator>(mut self) -> Self {
        self.aggregators.insert(A::name().to_owned());

        self
    }

    pub async fn start<E: Executor>(
        &mut self,
        _executor: &E,
    ) -> Result<Receiver<Event>, SubscribeError> {
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
