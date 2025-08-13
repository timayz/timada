use liteventd::{AggregatorName, SubscribeBuilder};

use crate::product::{CreateFailed, CreateRequested, Created, Product};

#[liteventd::handler(Product)]
async fn products_create_requested<E: liteventd::Executor>(
    context: &liteventd::Context<'_, E>,
    _data: CreateRequested,
    metadata: shared::Metadata,
) -> anyhow::Result<()> {
    todo!()
}

#[liteventd::handler(Product)]
async fn products_create_failed<E: liteventd::Executor>(
    context: &liteventd::Context<'_, E>,
    _data: CreateFailed,
    metadata: shared::Metadata,
) -> anyhow::Result<()> {
    todo!()
}

#[liteventd::handler(Product)]
async fn products_created<E: liteventd::Executor>(
    context: &liteventd::Context<'_, E>,
    _data: Created,
    metadata: shared::Metadata,
) -> anyhow::Result<()> {
    todo!()
}

pub fn subscribe_query_products<E: liteventd::Executor + Clone>(
    region: impl Into<String>,
) -> SubscribeBuilder<E> {
    let region = region.into();

    liteventd::subscribe(format!("{region}.product.query.products"))
        .routing_key(region)
        .aggregator::<Product>()
        .handler(products_create_requested())
        .handler(products_created())
        .handler(products_create_failed())
}
