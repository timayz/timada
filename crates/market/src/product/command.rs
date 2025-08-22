use evento::{AggregatorName, SkipHandler, SubscribeBuilder};
use serde::Deserialize;
use validator::Validate;

use crate::{
    RequestEvent,
    product::{CreateFailed, CreateRequested, Created, Product},
};

#[derive(Validate, Deserialize)]
pub struct CreateInput {
    #[validate(length(min = 3, max = 25))]
    pub name: String,
}

pub fn create(input: CreateInput) -> anyhow::Result<evento::SaveBuilder<Product>> {
    input.validate()?;

    Ok(evento::create().data(&CreateRequested {
        name: input.name,
        state: super::ProductState::Checking,
    })?)
}

#[evento::handler(Product)]
async fn command_create_requested<E: evento::Executor>(
    context: &evento::Context<'_, E>,
    event: RequestEvent<CreateRequested>,
) -> anyhow::Result<()> {
    evento::save::<Product>(&event.aggregator_id)
        .data(&Created {
            state: super::ProductState::Ready,
        })?
        .metadata(&event.metadata)?
        .commit(context.executor)
        .await?;

    Ok(())
}

pub fn subscribe_command<E: evento::Executor + Clone>(
    region: impl Into<String>,
) -> SubscribeBuilder<E> {
    let region = region.into();

    evento::subscribe(format!("market.{region}.product.command"))
        .routing_key(region)
        .aggregator::<Product>()
        .handler(SkipHandler::<Product, CreateFailed>::default())
        .handler(SkipHandler::<Product, Created>::default())
        .handler(command_create_requested())
}
