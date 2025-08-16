use crate::product::{CreateFailed, CreateRequested, Created, Product, ProductState};
use evento::{AggregatorName, SubscribeBuilder, sql::Reader};
use sea_query::{Expr, Query, SqliteQueryBuilder};
use sea_query_sqlx::SqlxBinder;
use serde::{Deserialize, Serialize};
use sqlx::{SqlitePool, prelude::FromRow};
use timada_shared::Metadata;

#[derive(Default, Serialize, Deserialize, Debug, Clone, FromRow)]
#[sea_query::enum_def]
pub struct QueryProduct {
    pub id: String,
    pub name: String,
    pub failed_reason: String,
    pub state: ProductState,
    pub created_at: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct QueryProductCursor {
    pub i: String,
    pub c: String,
}

impl evento::cursor::Cursor for QueryProduct {
    type T = QueryProductCursor;

    fn serialize(&self) -> Self::T {
        Self::T {
            i: self.id.to_owned(),
            c: self.created_at.to_owned(),
        }
    }
}

impl evento::sql::Bind for QueryProduct {
    type T = QueryProductIden;
    type I = [Self::T; 2];
    type V = [Expr; 2];
    type Cursor = Self;

    fn columns() -> Self::I {
        [QueryProductIden::CreatedAt, QueryProductIden::Id]
    }

    fn values(
        cursor: <<Self as evento::sql::Bind>::Cursor as evento::cursor::Cursor>::T,
    ) -> Self::V {
        [cursor.c.into(), cursor.i.to_string().into()]
    }
}

#[evento::handler(Product)]
async fn products_create_requested<E: evento::Executor>(
    context: &evento::Context<'_, E>,
    data: CreateRequested,
    _metadata: Metadata,
) -> anyhow::Result<()> {
    let pool = context.extract::<SqlitePool>();
    let mut conn = pool.acquire().await?;
    let statement = Query::insert()
        .into_table(QueryProductIden::Table)
        .columns([
            QueryProductIden::Id,
            QueryProductIden::Name,
            QueryProductIden::State,
        ])
        .values_panic([
            context.event.aggregator_id.to_string().into(),
            data.name.into(),
            data.state.to_string().into(),
        ])
        .to_owned();

    let (sql, values) = statement.build_sqlx(SqliteQueryBuilder);
    sqlx::query_with(&sql, values).execute(&mut *conn).await?;

    Ok(())
}

#[evento::handler(Product)]
async fn products_create_failed<E: evento::Executor>(
    context: &evento::Context<'_, E>,
    data: CreateFailed,
    _metadata: Metadata,
) -> anyhow::Result<()> {
    // let lmdb = context.extract::<heed::Env>();
    // let mut wtxn = lmdb.write_txn()?;
    // let db: QueryProductDB = lmdb.create_database(&mut wtxn, Some("market-products"))?;
    // let Some(mut product) = db.get(&wtxn, &context.event.aggregator_id)? else {
    //     tracing::error!("QueryProduct {} not found", context.event.aggregator_id);
    //
    //     std::process::exit(1);
    // };
    //
    // product.state = data.state;
    // db.put(&mut wtxn, &context.event.aggregator_id, &product)?;
    //
    // wtxn.commit()?;

    Ok(())
}

#[evento::handler(Product)]
async fn products_created<E: evento::Executor>(
    context: &evento::Context<'_, E>,
    data: Created,
    _metadata: Metadata,
) -> anyhow::Result<()> {
    // let lmdb = context.extract::<heed::Env>();
    // let mut wtxn = lmdb.write_txn()?;
    // let db: QueryProductDB = lmdb.create_database(&mut wtxn, Some("market-products"))?;
    // let Some(mut product) = db.get(&wtxn, &context.event.aggregator_id)? else {
    //     tracing::error!("QueryProduct {} not found", context.event.aggregator_id);
    //
    //     std::process::exit(1);
    // };
    //
    // product.state = data.state;
    // db.put(&mut wtxn, &context.event.aggregator_id, &product)?;
    //
    // wtxn.commit()?;

    Ok(())
}

pub async fn query_products(
    pool: &SqlitePool,
) -> anyhow::Result<evento::cursor::ReadResult<QueryProduct>> {
    let mut conn = pool.acquire().await?;

    let statement = Query::select()
        .columns([
            QueryProductIden::Id,
            QueryProductIden::Name,
            QueryProductIden::State,
            QueryProductIden::CreatedAt,
            QueryProductIden::FailedReason,
        ])
        .from(QueryProductIden::Table)
        .to_owned();

    Ok(Reader::new(statement).execute(&mut *conn).await?)
}

pub fn subscribe_query_products<E: evento::Executor + Clone>(
    region: impl Into<String>,
) -> anyhow::Result<SubscribeBuilder<E>> {
    let region = region.into();

    Ok(
        evento::subscribe(format!("market.{region}.product.query.products"))
            .routing_key(region)
            .aggregator::<Product>()
            .handler(products_create_requested())
            .handler(products_created())
            .handler(products_create_failed()),
    )
}
