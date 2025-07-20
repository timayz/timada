use crate::{
    Aggregator, Event, Executor, ReadError, WriteError,
    cursor::{Args, ReadResult, Value},
};

pub struct RocksDB;

#[async_trait::async_trait()]
impl Executor for RocksDB {
    async fn write<A: Aggregator>(
        &self,
        aggregator: &A,
        events: Vec<Event>,
    ) -> Result<(), WriteError> {
        todo!()
    }

    async fn get_event<A: Aggregator>(&self, cursor: Value) -> Result<Event, ReadError> {
        todo!()
    }

    async fn read<A: Aggregator>(
        &self,
        id: String,
        args: Args,
    ) -> Result<ReadResult<Event>, ReadError> {
        todo!()
    }

    async fn get_default_aggregator<A: Aggregator>(
        &self,
        id: String,
    ) -> Result<(A, Option<Value>), ReadError> {
        todo!()
    }
}
