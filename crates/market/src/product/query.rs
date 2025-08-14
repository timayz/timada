use evento::{AggregatorName, SubscribeBuilder};

use crate::product::{CreateFailed, CreateRequested, Created, Product};

#[evento::handler(Product)]
async fn products_create_requested<E: evento::Executor>(
    context: &evento::Context<'_, E>,
    _data: CreateRequested,
    metadata: shared::Metadata,
) -> anyhow::Result<()> {
    todo!()
}

#[evento::handler(Product)]
async fn products_create_failed<E: evento::Executor>(
    context: &evento::Context<'_, E>,
    _data: CreateFailed,
    metadata: shared::Metadata,
) -> anyhow::Result<()> {
    todo!()
}

#[evento::handler(Product)]
async fn products_created<E: evento::Executor>(
    context: &evento::Context<'_, E>,
    _data: Created,
    metadata: shared::Metadata,
) -> anyhow::Result<()> {
    todo!()
}

pub fn subscribe_query_products<E: evento::Executor + Clone>(
    region: impl Into<String>,
) -> SubscribeBuilder<E> {
    let region = region.into();

    evento::subscribe(format!("market.{region}.product.query.products"))
        .routing_key(region)
        .aggregator::<Product>()
        .handler(products_create_requested())
        .handler(products_created())
        .handler(products_create_failed())
}
