use crate::product::{CreateFailed, CreateRequested, Created, Product, ProductState};
use evento::{AggregatorName, SubscribeBuilder, cursor::Reader};
use serde::{Deserialize, Serialize};

type QueryProductDB = heed::Database<heed::types::Str, heed::types::SerdeBincode<QueryProduct>>;

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct QueryProduct {
    pub id: String,
    pub name: String,
    pub state: ProductState,
    pub created_at: i64,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct QueryProductCursor {
    pub i: String,
    pub c: i64,
}

impl evento::cursor::Cursor for QueryProduct {
    type T = QueryProductCursor;

    fn serialize(&self) -> Self::T {
        Self::T {
            i: self.id.to_owned(),
            c: self.created_at,
        }
    }
}

impl evento::cursor::Bind for QueryProduct {
    type T = Self;

    fn sort_by(data: &mut Vec<Self::T>, is_order_desc: bool) {
        if !is_order_desc {
            data.sort_by(|a, b| {
                if a.created_at != b.created_at {
                    return a.created_at.cmp(&b.created_at);
                }

                a.id.cmp(&b.id)
            });
        } else {
            data.sort_by(|a, b| {
                if a.created_at != b.created_at {
                    return b.created_at.cmp(&a.created_at);
                }

                b.id.cmp(&a.id)
            });
        }
    }
    fn retain(
        data: &mut Vec<Self::T>,
        cursor: <<Self as evento::cursor::Bind>::T as evento::cursor::Cursor>::T,
        is_order_desc: bool,
    ) {
        data.retain(|event| {
            if is_order_desc {
                event.created_at < cursor.c || (event.created_at == cursor.c && event.id < cursor.i)
            } else {
                event.created_at > cursor.c || (event.created_at == cursor.c && event.id > cursor.i)
            }
        });
    }
}

#[evento::handler(Product)]
async fn products_create_requested<E: evento::Executor>(
    context: &evento::Context<'_, E>,
    data: CreateRequested,
    _metadata: shared::Metadata,
) -> anyhow::Result<()> {
    let lmdb = context.extract::<heed::Env>();
    let mut wtxn = lmdb.write_txn()?;
    let db: QueryProductDB = lmdb.create_database(&mut wtxn, Some("market-products"))?;
    db.put(
        &mut wtxn,
        &context.event.aggregator_id,
        &QueryProduct {
            id: context.event.aggregator_id.to_owned(),
            name: data.name,
            state: data.state,
            created_at: context.event.timestamp,
        },
    )?;

    wtxn.commit()?;

    Ok(())
}

#[evento::handler(Product)]
async fn products_create_failed<E: evento::Executor>(
    context: &evento::Context<'_, E>,
    data: CreateFailed,
    _metadata: shared::Metadata,
) -> anyhow::Result<()> {
    let lmdb = context.extract::<heed::Env>();
    let mut wtxn = lmdb.write_txn()?;
    let db: QueryProductDB = lmdb.create_database(&mut wtxn, Some("market-products"))?;
    let Some(mut product) = db.get(&wtxn, &context.event.aggregator_id)? else {
        tracing::error!("QueryProduct {} not found", context.event.aggregator_id);

        std::process::exit(1);
    };

    product.state = data.state;
    db.put(&mut wtxn, &context.event.aggregator_id, &product)?;

    wtxn.commit()?;

    Ok(())
}

#[evento::handler(Product)]
async fn products_created<E: evento::Executor>(
    context: &evento::Context<'_, E>,
    data: Created,
    _metadata: shared::Metadata,
) -> anyhow::Result<()> {
    let lmdb = context.extract::<heed::Env>();
    let mut wtxn = lmdb.write_txn()?;
    let db: QueryProductDB = lmdb.create_database(&mut wtxn, Some("market-products"))?;
    let Some(mut product) = db.get(&wtxn, &context.event.aggregator_id)? else {
        tracing::error!("QueryProduct {} not found", context.event.aggregator_id);

        std::process::exit(1);
    };

    product.state = data.state;
    db.put(&mut wtxn, &context.event.aggregator_id, &product)?;

    wtxn.commit()?;

    Ok(())
}

pub fn query_products(
    lmdb: &heed::Env,
) -> anyhow::Result<evento::cursor::ReadResult<QueryProduct>> {
    let rtxn = lmdb.read_txn()?;
    let Some::<QueryProductDB>(db) = lmdb.open_database(&rtxn, Some("market-products"))? else {
        return Err(anyhow::anyhow!(
            "market-products database not found while quering products"
        ));
    };

    let rows = db
        .iter(&rtxn)?
        .collect::<Result<Vec<(&str, QueryProduct)>, _>>()?
        .into_iter()
        .map(|(_, p)| p)
        .collect::<Vec<_>>();

    Ok(Reader::new(rows).execute()?)
}

pub fn subscribe_query_products<E: evento::Executor + Clone>(
    region: impl Into<String>,
    lmdb: &heed::Env,
) -> anyhow::Result<SubscribeBuilder<E>> {
    let region = region.into();
    let mut wtxn = lmdb.write_txn()?;
    let _: QueryProductDB = lmdb.create_database(&mut wtxn, Some("market-products"))?;
    wtxn.commit()?;

    Ok(
        evento::subscribe(format!("market.{region}.product.query.products"))
            .routing_key(region)
            .data(lmdb.clone())
            .aggregator::<Product>()
            .handler(products_create_requested())
            .handler(products_created())
            .handler(products_create_failed()),
    )
}
