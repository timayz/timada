use serde::{Serialize, de::DeserializeOwned};
use thiserror::Error;
use ulid::Ulid;

pub struct Event {
    pub id: Ulid,
    pub aggregate_id: Ulid,
    pub aggregate_type: String,
    pub name: String,
    pub data: Vec<u8>,
    pub metadata: Vec<u8>,
    pub timestamp: u32,
}

impl Event {
    pub fn to_data<D: AggregatorEvent>(&self) -> anyhow::Result<Option<D>> {
        if D::name() != self.name {
            return Ok(None);
        }
        todo!();
    }

    pub fn to_metadata<M>(&self) -> anyhow::Result<M> {
        todo!();
    }
}

pub trait Aggregator: Default + Serialize + DeserializeOwned {
    fn aggregate(&mut self, event: &'_ Event) -> anyhow::Result<()>;
    fn revision() -> &'static str;
    fn name() -> &'static str;
}

pub trait AggregatorEvent {
    fn name() -> &'static str;
}

pub trait Executor {}

pub struct SqlExecutor();

impl Executor for SqlExecutor {}

#[derive(Debug, Error)]
pub enum LoadError {}

pub struct LoadResult<A: Aggregator> {
    pub item: A,
    pub version: u16,
}

pub fn load<E: Executor, A: Aggregator>(
    _executor: &E,
    id: Ulid,
) -> Result<LoadResult<A>, LoadError> {
    todo!()
}

#[derive(Debug, Error)]
pub enum SaveError {}

pub struct SaveBuilder<A: Aggregator> {
    aggregate_id: Ulid,
    aggregate_type: String,
    aggregator: A,
    original_version: u16,
    data: Vec<(&'static str, Vec<u8>)>,
    metadata: Vec<u8>,
}

impl<A: Aggregator> SaveBuilder<A> {
    pub fn original_version(mut self, v: u16) -> Self {
        self.original_version = v;

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

    pub fn commit<E: Executor>(&self, _executor: &E) -> Result<(), SaveError> {
        todo!()
    }
}

pub fn save<A: Aggregator>(aggregator: A, aggregate_id: Ulid) -> SaveBuilder<A> {
    SaveBuilder {
        aggregate_id,
        aggregator,
        aggregate_type: A::name().to_owned(),
        original_version: 0,
        data: Vec::default(),
        metadata: Vec::default(),
    }
}

pub fn subscribe() {}
