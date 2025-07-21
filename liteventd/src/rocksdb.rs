use crate::{
    Aggregator, Event, Executor, ReadError, WriteError,
    cursor::{Args, ReadResult, Value},
};

pub struct RocksDB;

#[async_trait::async_trait()]
impl Executor for RocksDB {
    async fn write(&self, events: Vec<Event>) -> Result<(), WriteError> {
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

    async fn get_snapshot<A: Aggregator>(
        &self,
        id: String,
    ) -> Result<Option<(Vec<u8>, Value)>, ReadError> {
        todo!()
    }

    async fn save_snapshot<A: Aggregator>(
        &self,
        event: Event,
        data: Vec<u8>,
        cursor: Value,
    ) -> Result<(), WriteError> {
        todo!()
    }
}
