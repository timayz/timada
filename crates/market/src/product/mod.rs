mod command;
mod query;

use evento::{AggregatorName, EventData};
use serde::{Deserialize, Serialize};
use timada_shared::Metadata;

pub use command::*;
pub use query::*;

type ProductEvent<D> = EventData<D, Metadata>;

#[derive(Debug, Default, Serialize, Deserialize, Clone, PartialEq)]
pub enum ProductState {
    #[default]
    Checking,
    Failed(String, String),
    Ready,
}

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct Product {
    pub name: String,
    pub state: ProductState,
}

#[evento::aggregator]
impl Product {
    async fn create_requested(
        &mut self,
        event: ProductEvent<CreateRequested>,
    ) -> anyhow::Result<()> {
        self.name = event.data.name;
        self.state = event.data.state;

        Ok(())
    }

    async fn create_failed(&mut self, event: ProductEvent<CreateFailed>) -> anyhow::Result<()> {
        self.state = event.data.state;

        Ok(())
    }

    async fn created(&mut self, event: ProductEvent<Created>) -> anyhow::Result<()> {
        self.state = event.data.state;

        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq, AggregatorName)]
pub struct CreateRequested {
    pub name: String,
    pub state: ProductState,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, AggregatorName)]
pub struct Created {
    pub state: ProductState,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, AggregatorName)]
pub struct CreateFailed {
    pub state: ProductState,
}
