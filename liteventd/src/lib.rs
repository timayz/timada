pub mod context;
pub mod cursor;
pub mod sql;

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    collections::HashMap,
    fmt::Debug,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use thiserror::Error;
use tokio::sync::RwLock;
use ulid::Ulid;

use crate::cursor::{Args, Cursor, ReadResult, Value};

pub struct EventData<D, M> {
    pub details: Event,
    pub data: D,
    pub metadata: M,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct EventCursor {
    pub i: Ulid,
    pub v: u16,
    pub t: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub id: Ulid,
    pub aggregate_id: Ulid,
    pub aggregate_type: String,
    pub version: u16,
    pub name: String,
    pub routing_key: String,
    pub data: Vec<u8>,
    pub metadata: Vec<u8>,
    pub timestamp: u64,
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

impl Cursor for Event {
    type T = EventCursor;

    fn serialize(&self) -> Self::T {
        EventCursor {
            i: self.id,
            v: self.version,
            t: self.timestamp,
        }
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
    async fn write(&self, events: Vec<Event>) -> Result<(), WriteError>;

    async fn get_event<A: Aggregator>(&self, cursor: Value) -> Result<Event, ReadError>;

    async fn read<A: Aggregator>(
        &self,
        id: String,
        args: Args,
    ) -> Result<ReadResult<Event>, ReadError>;

    async fn get_snapshot<A: Aggregator>(
        &self,
        id: String,
    ) -> Result<Option<(Vec<u8>, Value)>, ReadError>;

    async fn save_snapshot<A: Aggregator>(
        &self,
        id: Ulid,
        data: Vec<u8>,
        cursor: cursor::Value,
    ) -> Result<(), WriteError>;
}

#[derive(Debug, Error)]
pub enum ReadError {
    #[error("not found")]
    NotFound,

    #[error("too many events to aggregate")]
    TooManyEvents,

    #[error("{0}")]
    Unknown(#[from] anyhow::Error),

    #[error("ciborium.ser >> {0}")]
    CiboriumSer(#[from] ciborium::ser::Error<std::io::Error>),

    #[error("ciborium.de >> {0}")]
    CiboriumDe(#[from] ciborium::de::Error<std::io::Error>),

    #[error("base64 decode: {0}")]
    Base64Decode(#[from] base64::DecodeError),

    #[error("write: {0}")]
    Write(#[from] WriteError),
}

#[derive(Debug, Clone)]
pub struct LoadResult<A: Aggregator> {
    pub item: A,
    pub event: Event,
}

pub async fn load<A: Aggregator, E: Executor>(
    executor: &E,
    id: impl Into<String>,
) -> Result<LoadResult<A>, ReadError> {
    let id = id.into();
    let (mut aggregator, mut cursor) = match executor.get_snapshot::<A>(id.to_owned()).await? {
        Some((data, cursor)) => {
            let aggregator: A = ciborium::from_reader(&data[..])?;
            (aggregator, Some(cursor))
        }
        _ => (A::default(), None),
    };

    let mut interval = tokio::time::interval(Duration::from_secs(1));
    let mut loop_count = 0;

    loop {
        let events = executor
            .read::<A>(id.to_owned(), Args::forward(1000, cursor.clone()))
            .await?;

        for event in events.edges.iter() {
            aggregator.aggregate(&event.node).await?;
        }

        if !events.page_info.has_next_page {
            let event = match (cursor, events.edges.last()) {
                (_, Some(event)) => event.node.clone(),
                (Some(cursor), None) => executor.get_event::<A>(cursor).await?,
                _ => return Err(ReadError::NotFound),
            };

            return Ok(LoadResult {
                item: aggregator,
                event,
            });
        }

        cursor = events.page_info.end_cursor;

        if let (Some(event), Some(cursor)) = (events.edges.last().cloned(), cursor.clone()) {
            let mut data = vec![];
            ciborium::into_writer(&aggregator, &mut data)?;

            executor
                .save_snapshot::<A>(event.node.aggregate_id, data, cursor)
                .await?;
        }

        interval.tick().await;

        loop_count += 1;
        if loop_count > 10 {
            return Err(ReadError::TooManyEvents);
        }
    }
}

#[derive(Debug, Error)]
pub enum WriteError {
    #[error("invalid original version")]
    InvalidOriginalVersion,

    #[error("missing data")]
    MissingData,

    #[error("missing metadata")]
    MissingMetadata,

    #[error("{0}")]
    Unknown(#[from] anyhow::Error),

    #[error("ciborium.ser >> {0}")]
    CiboriumSer(#[from] ciborium::ser::Error<std::io::Error>),

    #[error("systemtime >> {0}")]
    SystemTime(#[from] std::time::SystemTimeError),
}

pub struct SaveBuilder<A: Aggregator> {
    aggregate_id: Ulid,
    aggregate_type: String,
    aggregator: A,
    routing_key: Option<String>,
    original_version: u16,
    data: Vec<(&'static str, Vec<u8>)>,
    metadata: Option<Vec<u8>>,
}

impl<A: Aggregator> SaveBuilder<A> {
    pub fn new(aggregator: A, aggregate_id: Ulid) -> SaveBuilder<A> {
        SaveBuilder {
            aggregate_id,
            aggregator,
            aggregate_type: A::name().to_owned(),
            routing_key: None,
            original_version: 0,
            data: Vec::default(),
            metadata: None,
        }
    }

    pub fn original_version(mut self, v: u16) -> Self {
        self.original_version = v;

        self
    }

    pub fn routing_key(mut self, v: impl Into<String>) -> Self {
        if self.routing_key.is_none() {
            self.routing_key = Some(v.into());
        }

        self
    }

    pub fn metadata<M: Serialize>(
        mut self,
        v: &M,
    ) -> Result<Self, ciborium::ser::Error<std::io::Error>> {
        let mut metadata = Vec::new();
        ciborium::into_writer(v, &mut metadata)?;
        self.metadata = Some(metadata);

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

    pub async fn commit<E: Executor>(&self, executor: &E) -> Result<Ulid, WriteError> {
        let routing_key = self.routing_key.to_owned().unwrap_or("default".to_owned());
        let mut aggregator = self.aggregator.clone();
        let mut version = self.original_version;

        let Some(metadata) = &self.metadata else {
            return Err(WriteError::MissingMetadata);
        };

        let mut events = vec![];
        let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();

        for (name, data) in &self.data {
            version += 1;

            let event = Event {
                id: Ulid::new(),
                name: name.to_string(),
                data: data.to_vec(),
                metadata: metadata.to_vec(),
                timestamp,
                aggregate_id: self.aggregate_id,
                aggregate_type: self.aggregate_type.to_owned(),
                version,
                routing_key: routing_key.to_owned(),
            };

            aggregator.aggregate(&event).await?;
            events.push(event);
        }

        let Some(last_event) = events.last().cloned() else {
            return Err(WriteError::MissingData);
        };

        executor.write(events).await?;

        let mut data = vec![];
        ciborium::into_writer(&aggregator, &mut data)?;
        let cursor = last_event.serialize_cursor()?;

        executor
            .save_snapshot::<A>(last_event.aggregate_id, data, cursor)
            .await?;

        Ok(self.aggregate_id)
    }
}

pub fn create<A: Aggregator>() -> SaveBuilder<A> {
    SaveBuilder::new(A::default(), Ulid::new())
}

pub fn save<A: Aggregator>(aggregator: LoadResult<A>, aggregate_id: Ulid) -> SaveBuilder<A> {
    SaveBuilder::new(aggregator.item, aggregate_id)
        .original_version(aggregator.event.version)
        .routing_key(aggregator.event.routing_key)
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
        id: Ulid::default(),
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
    _executor: &E,
    _key: impl Into<String>,
    _event: &Event,
) -> Result<(), AcknowledgeError> {
    todo!()
}
