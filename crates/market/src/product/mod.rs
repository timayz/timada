mod command;
mod query;

use evento::AggregatorName;
use serde::{Deserialize, Serialize};
use strum::Display;

pub use command::*;
pub use query::*;

use crate::RequestEvent;

#[derive(Debug, Default, Serialize, Deserialize, Clone, PartialEq, sqlx::Type, Display)]
#[sqlx(type_name = "product_state", rename_all = "kebab-case")]
#[strum(serialize_all = "kebab_case")]
pub enum ProductState {
    #[default]
    Checking,
    Failed,
    Ready,
}

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct Product {
    pub name: String,
    pub state: ProductState,
    pub failed_reason: String,
}

#[evento::aggregator]
impl Product {
    async fn create_requested(
        &mut self,
        event: RequestEvent<CreateRequested>,
    ) -> anyhow::Result<()> {
        self.name = event.data.name;
        self.state = event.data.state;

        Ok(())
    }

    async fn create_failed(&mut self, event: RequestEvent<CreateFailed>) -> anyhow::Result<()> {
        self.state = event.data.state;

        Ok(())
    }

    async fn created(&mut self, event: RequestEvent<Created>) -> anyhow::Result<()> {
        self.state = event.data.state;

        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq, AggregatorName)]
pub struct CreateRequested {
    pub name: String,
    pub state: ProductState,
}

#[derive(Debug, Default, Serialize, Deserialize, PartialEq, AggregatorName)]
pub struct Created {
    pub state: ProductState,
}

#[derive(Default, Debug, Serialize, Deserialize, PartialEq, AggregatorName)]
pub struct CreateFailed {
    pub state: ProductState,
    pub failed_reason: String,
}
