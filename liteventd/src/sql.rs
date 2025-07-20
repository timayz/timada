use std::marker::PhantomData;

use crate::{
    Aggregator, Event, EventCursor, Executor, ReadError, WriteError,
    cursor::{Args, Cursor, Edge, Order, PageInfo, ReadResult, Value},
};
use base64::{
    Engine, alphabet,
    engine::{GeneralPurpose, general_purpose},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sqlx::{
    Arguments, Database, Encode, Execute, FromRow, IntoArguments, Pool, QueryBuilder, Row, Type,
    query::QueryAs,
};

#[derive(Debug, Serialize, Deserialize, FromRow)]
pub struct Snapshot {
    pub cursor: Vec<u8>,
    pub data: Vec<u8>,
}

fn get_database_bind_key<D: Database>() -> &'static str {
    match D::NAME {
        "SQLite" => "?",
        name => panic!("get_database_schema not supported for '{name}'"),
    }
}

pub struct SqlExecutor<D: Database>(pub Pool<D>);

impl<D> SqlExecutor<D>
where
    D: Database,
    for<'a> D::Arguments<'a>: sqlx::IntoArguments<'a, D>,
    for<'c> &'c mut D::Connection: sqlx::Executor<'c, Database = D>,
{
    pub async fn create_database_schema(&self) -> sqlx::Result<()> {
        let schema = match D::NAME {
            "SQLite" => Self::get_sqlite_schema(),
            name => panic!("get_database_schema not supported for '{name}'"),
        };

        sqlx::query(schema).execute(&self.0).await?;

        Ok(())
    }

    fn get_sqlite_schema() -> &'static str {
        r#"
CREATE TABLE IF NOT EXISTS liteventd_event (
    id  TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    aggregate_type TEXT NOT NULL,
    aggregate_id TEXT NOT NULL,
    version INTEGER NOT NULL,
    data BLOB NOT NULL,
    metadata BLOB NULL,
    routing_key TEXT NOT NULL,
    timestamp INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
);

CREATE INDEX IF NOT EXISTS idx_event_5sQ1BZoDjCO ON liteventd_event(aggregate_type, aggregate_id);
CREATE INDEX IF NOT EXISTS idx_event_CISJrZpQwSz ON liteventd_event(aggregate_type);
CREATE INDEX IF NOT EXISTS idx_event_sX8G8WjC8WQ ON liteventd_event(routing_key, aggregate_type);
CREATE UNIQUE INDEX IF NOT EXISTS idx_event_BVjOvQoJPQj ON liteventd_event(aggregate_type,aggregate_id,version);

CREATE TABLE IF NOT EXISTS liteventd_snapshot (
    id TEXT NOT NULL,
    type TEXT NOT NULL,
    cursor BLOB NOT NULL,
    revision TEXT NOT NULL,
    data BLOB NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
    updated_at INTEGER NULL,
    PRIMARY KEY (id,type)
);
        "#
    }
}

impl<D: Database> Clone for SqlExecutor<D> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<D: Database> From<Pool<D>> for SqlExecutor<D> {
    fn from(value: Pool<D>) -> Self {
        Self(value)
    }
}

#[async_trait::async_trait]
impl<D> Executor for SqlExecutor<D>
where
    D: Database,
    for<'a> D::Arguments<'a>: sqlx::IntoArguments<'a, D> + Clone,
    for<'c> &'c mut D::Connection: sqlx::Executor<'c, Database = D>,
    for<'t> String: sqlx::Encode<'t, D> + sqlx::Decode<'t, D> + sqlx::Type<D>,
    for<'t> u32: sqlx::Encode<'t, D> + sqlx::Decode<'t, D> + sqlx::Type<D>,
    for<'t> u16: sqlx::Encode<'t, D> + sqlx::Decode<'t, D> + sqlx::Type<D>,
    for<'t> Vec<u8>: sqlx::Encode<'t, D> + sqlx::Decode<'t, D> + sqlx::Type<D>,
    for<'t> &'t [u8]: sqlx::Encode<'t, D> + sqlx::Decode<'t, D> + sqlx::Type<D>,
    for<'t> &'t str: sqlx::ColumnIndex<D::Row>,
{
    async fn get_event<A: Aggregator>(&self, cursor: Value) -> Result<Event, ReadError> {
        todo!()
    }

    async fn read<A: Aggregator>(
        &self,
        id: String,
        args: Args,
    ) -> Result<ReadResult<Event>, ReadError> {
        // let q = Query::<_, Event>::new("").args(args).query(&self.0).await;

        todo!()
    }

    async fn get_default_aggregator<A: Aggregator>(
        &self,
        id: String,
    ) -> Result<(A, Option<Value>), ReadError> {
        let Some(snapshot) = sqlx::query_as::<_, Snapshot>(&format!(
            "SELECT cursor, data FROM liteventd_snapshot WHERE id = {0} AND type = {0} AND revision = {0}",
            get_database_bind_key::<D>()
        ))
        .bind(id)
        .bind(A::name().to_owned())
        .bind(A::revision().to_owned())
        .fetch_optional(&self.0)
        .await.map_err(|err| ReadError::Unknown(err.into()))?
        else {
            return Ok((A::default(), None));
        };

        let aggregator: A = ciborium::from_reader(&snapshot.data[..])?;

        Ok((
            aggregator,
            Some(
                String::from_utf8(snapshot.cursor)
                    .map_err(|e| ReadError::Unknown(e.into()))?
                    .into(),
            ),
        ))
    }

    async fn write<A: Aggregator>(
        &self,
        aggregator: &A,
        events: Vec<Event>,
    ) -> Result<(), WriteError> {
        let Some(info) = events.first() else {
            return Err(WriteError::NoData);
        };

        let values = events
            .iter()
            .map(|_| {
                format!(
                    "({0},{0},{0},{0},{0},{0},{0},{0})",
                    get_database_bind_key::<D>()
                )
            })
            .collect::<Vec<_>>()
            .join(",");

        let sql = format!(
            "INSERT INTO liteventd_event (id,name,data,metadata,aggregate_type,aggregate_id,version,routing_key) VALUES {}",
            values
        );

        let mut query = sqlx::query(&sql);

        for event in events.iter() {
            query = query
                .bind(&event.id)
                .bind(&event.name)
                .bind(&event.data)
                .bind(&event.metadata)
                .bind(&event.aggregate_type)
                .bind(&event.aggregate_id)
                .bind(event.version)
                .bind(&event.routing_key);
        }

        query.execute(&self.0).await.map_err(|err| {
            if err.to_string().contains("(code: 2067)") {
                WriteError::InvalidOriginalVersion
            } else {
                WriteError::Unknown(err.into())
            }
        })?;

        let mut data = vec![];
        ciborium::into_writer(aggregator, &mut data)?;
        let cursor = info.serialize_cursor()?;

        sqlx::query(&format!(
            r#"
        INSERT INTO liteventd_snapshot (type,id,cursor,revision,data)
        VALUES ({0},{0},{0},{0},{0})
            ON CONFLICT(type,id) DO UPDATE SET 
                data=excluded.data,
                cursor=excluded.cursor,
                revision=excluded.revision,
                updated_at=(strftime('%s', 'now'))
        "#,
            get_database_bind_key::<D>()
        ))
        .bind(&info.aggregate_type)
        .bind(&info.aggregate_id)
        .bind(cursor.as_ref())
        .bind(A::revision().to_owned())
        .bind(&data)
        .execute(&self.0)
        .await
        .map_err(|err| WriteError::Unknown(err.into()))?;

        Ok(())
    }
}

pub trait BindCursor<'q, DB: Database> {
    type Cursor: DeserializeOwned;

    fn bing_keys() -> Vec<&'static str>;

    fn bind_query<O>(
        cursor: Self::Cursor,
        query: QueryAs<'q, DB, O, DB::Arguments<'q>>,
    ) -> QueryAs<'q, DB, O, DB::Arguments<'q>>;

    fn bind_cursor<O>(
        value: &Value,
        query: QueryAs<'q, DB, O, DB::Arguments<'q>>,
    ) -> Result<QueryAs<'q, DB, O, DB::Arguments<'q>>, ReadError> {
        let engine = GeneralPurpose::new(&alphabet::URL_SAFE, general_purpose::PAD);
        let decoded = engine.decode(value)?;
        let cursor = ciborium::from_reader(&decoded[..])?;

        Ok(Self::bind_query(cursor, query))
    }
}

impl<'args, DB, O> Query<'args, DB, O>
where
    DB: Database,
    DB::Arguments<'args>: IntoArguments<'args, DB> + Clone,
    O: for<'r> FromRow<'r, DB::Row>,
    O: 'args + Send + Unpin,
    O: 'args + BindCursor<'args, DB> + Cursor,
{
    pub fn new(sql: impl Into<String>) -> Self {
        Self {
            qb: QueryBuilder::new(sql),
            qb_args: DB::Arguments::default(),
            phantom_o: PhantomData,
            order: Order::Asc,
            args: Default::default(),
        }
    }

    pub fn bind<Arg>(mut self, arg: Arg) -> Result<Self, sqlx::error::BoxDynError>
    where
        Arg: 'args + Send + sqlx::Encode<'args, DB> + sqlx::Type<DB>,
    {
        self.qb_args.add(arg)?;
        Ok(self)
    }

    pub fn order(mut self, value: Order) -> Self {
        self.order = value;

        self
    }

    pub fn args(mut self, value: Args) -> Self {
        self.args = value;

        self
    }

    pub fn desc(self) -> Self {
        self.order(Order::Desc)
    }

    pub fn backward(self, last: u16, before: Option<Value>) -> Self {
        self.args(Args {
            last: Some(last),
            before,
            ..Default::default()
        })
    }

    pub fn forward(self, first: u16, after: Option<Value>) -> Self {
        self.args(Args {
            first: Some(first),
            after,
            ..Default::default()
        })
    }

    pub async fn query<'a, E>(&'args mut self, executor: E) -> Result<ReadResult<O>, ReadError>
    where
        E: 'a + sqlx::Executor<'a, Database = DB>,
    {
        let (limit, cursor) = self.build();

        let mut query = sqlx::query_as_with::<_, O, _>(self.qb.sql(), self.qb_args.clone());
        if let Some(cursor) = cursor {
            query = O::bind_cursor(&cursor, query)?;
        }
        let mut rows = query
            .fetch_all(executor)
            .await
            .map_err(|err| ReadError::Unknown(err.into()))?;
        let has_more = rows.len() > limit as usize;

        if has_more {
            rows.pop();
        }

        let mut edges = vec![];
        for node in rows.into_iter() {
            edges.push(Edge {
                cursor: node.serialize_cursor()?,
                node,
            });
        }

        if self.is_backward() {
            edges = edges.into_iter().rev().collect();
        }

        let page_info = if self.is_backward() {
            let start_cursor = edges.first().map(|e| e.cursor.clone());

            PageInfo {
                has_previous_page: has_more,
                has_next_page: false,
                start_cursor,
                end_cursor: None,
            }
        } else {
            let end_cursor = edges.last().map(|e| e.cursor.clone());
            PageInfo {
                has_previous_page: false,
                has_next_page: has_more,
                start_cursor: None,
                end_cursor,
            }
        };

        Ok(ReadResult { edges, page_info })
    }

    fn build(&mut self) -> (u16, Option<Value>) {
        let (limit, cursor) = if self.is_backward() {
            (self.args.last.unwrap_or(40), self.args.before.clone())
        } else {
            (self.args.first.unwrap_or(40), self.args.after.clone())
        };

        if cursor.is_some() {
            let cursor_expr = self.build_cursor_expr(O::bing_keys(), self.qb_args.len() + 1);
            let where_expr = if self.qb.sql().contains(" WHERE ") {
                format!("AND ({cursor_expr})")
            } else {
                format!("WHERE {cursor_expr}")
            };

            self.qb.push(format!(" {where_expr}"));
        }

        let order = match (&self.order, self.is_backward()) {
            (Order::Asc, true) | (Order::Desc, false) => "DESC",
            (Order::Asc, false) | (Order::Desc, true) => "ASC",
        };

        let order_expr = O::bing_keys()
            .iter()
            .map(|k| format!("{k} {order}"))
            .collect::<Vec<_>>()
            .join(", ");

        self.qb
            .push(format!(" ORDER BY {order_expr} LIMIT {}", limit + 1));

        (limit, cursor)
    }

    fn build_cursor_expr(&self, mut keys: Vec<&str>, pos: usize) -> String {
        let sign = match (&self.order, self.is_backward()) {
            (Order::Asc, true) | (Order::Desc, false) => "<",
            (Order::Asc, false) | (Order::Desc, true) => ">",
        };

        let current_key = keys.remove(0);
        let expr = format!(
            "{current_key} {sign} {}{pos}",
            get_database_bind_key::<DB>()
        );

        if keys.is_empty() {
            return expr;
        }

        format!(
            "{expr} OR ({current_key} = {}{pos} AND {})",
            get_database_bind_key::<DB>(),
            self.build_cursor_expr(keys, pos + 1)
        )
    }

    fn is_backward(&self) -> bool {
        (self.args.last.is_some() || self.args.before.is_some())
            && self.args.first.is_none()
            && self.args.after.is_none()
    }
}

pub struct Query<'args, DB, O>
where
    DB: Database,
    DB::Arguments<'args>: IntoArguments<'args, DB> + Clone,
    O: for<'r> FromRow<'r, DB::Row>,
    O: 'args + Send + Unpin,
    O: 'args + BindCursor<'args, DB> + Cursor,
{
    qb: QueryBuilder<'args, DB>,
    qb_args: DB::Arguments<'args>,
    phantom_o: PhantomData<O>,
    order: Order,
    args: Args,
}

impl<'q, DB: Database> BindCursor<'q, DB> for Event
where
    u16: sqlx::Encode<'q, DB> + sqlx::Type<DB>,
    u32: sqlx::Encode<'q, DB> + sqlx::Type<DB>,
    String: sqlx::Encode<'q, DB> + sqlx::Type<DB>,
{
    type Cursor = EventCursor;

    fn bing_keys() -> Vec<&'static str> {
        vec!["timestamp", "version", "id"]
    }

    fn bind_query<O>(
        cursor: Self::Cursor,
        query: QueryAs<'q, DB, O, <DB as Database>::Arguments<'q>>,
    ) -> QueryAs<'q, DB, O, <DB as Database>::Arguments<'q>> {
        query.bind(cursor.t).bind(cursor.v).bind(cursor.i)
    }
}

impl<R: sqlx::Row> FromRow<'_, R> for Event
where
    for<'r> u32: sqlx::Type<R::Database> + sqlx::Decode<'r, R::Database>,
    for<'r> u16: sqlx::Type<R::Database> + sqlx::Decode<'r, R::Database>,
    for<'r> Vec<u8>: sqlx::Type<R::Database> + sqlx::Decode<'r, R::Database>,
    for<'r> String: sqlx::Type<R::Database> + sqlx::Decode<'r, R::Database>,
    for<'r> &'r str: sqlx::ColumnIndex<R>,
{
    fn from_row(row: &R) -> Result<Self, sqlx::Error> {
        Ok(Event {
            id: row.try_get("id")?,
            aggregate_id: row.try_get("aggregate_id")?,
            aggregate_type: row.try_get("aggregate_type")?,
            version: row.try_get("version")?,
            name: row.try_get("name")?,
            routing_key: row.try_get("routing_key")?,
            data: row.try_get("data")?,
            metadata: row.try_get("metadata")?,
            timestamp: row.try_get("timestamp")?,
        })
    }
}
