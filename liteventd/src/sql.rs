use std::ops::{Deref, DerefMut};

use sea_query::{
    ColumnDef, Expr, ExprTrait, Iden, Index, IntoColumnRef, OnConflict, Query, SelectStatement,
    SqliteQueryBuilder, Table,
};
use sea_query_binder::SqlxBinder;
use sqlx::{Database, Pool};
use ulid::Ulid;

use crate::{
    Aggregator, Executor, ReadError, WriteError,
    cursor::{self, Args, Cursor, Edge, PageInfo, ReadResult, Value},
};

#[derive(Iden, Clone)]
pub enum Event {
    Table,
    Id,
    Name,
    AggregateType,
    AggregateId,
    Version,
    Data,
    Metadata,
    RoutingKey,
    Timestamp,
}

#[derive(Iden)]
enum Snapshot {
    Table,
    Id,
    Type,
    Cursor,
    Revision,
    Data,
    CreatedAt,
    UpdatedAt,
}

pub struct Sql<DB: Database>(Pool<DB>);

impl<DB: Database> Sql<DB> {
    pub fn get_schema() -> String {
        let event_table = Table::create()
            .table(Event::Table)
            .if_not_exists()
            .col(
                ColumnDef::new(Event::Id)
                    .string()
                    .not_null()
                    .string_len(26)
                    .primary_key(),
            )
            .col(
                ColumnDef::new(Event::Name)
                    .string()
                    .string_len(20)
                    .not_null(),
            )
            .col(
                ColumnDef::new(Event::AggregateType)
                    .string()
                    .string_len(50)
                    .not_null(),
            )
            .col(
                ColumnDef::new(Event::AggregateId)
                    .string()
                    .string_len(26)
                    .not_null(),
            )
            .col(ColumnDef::new(Event::Version).integer().not_null())
            .col(ColumnDef::new(Event::Data).blob().not_null())
            .col(ColumnDef::new(Event::Metadata).blob().not_null())
            .col(
                ColumnDef::new(Event::RoutingKey)
                    .string()
                    .string_len(50)
                    .not_null(),
            )
            .col(
                ColumnDef::new(Event::Timestamp)
                    .integer()
                    .not_null()
                    .default(Expr::custom_keyword("(strftime('%s', 'now'))")),
            )
            .to_owned();

        let idx_event_type = Index::create()
            .if_not_exists()
            .name("idx_event_type")
            .table(Event::Table)
            .col(Event::AggregateType)
            .to_owned();

        let idx_event_type_id = Index::create()
            .if_not_exists()
            .name("idx_event_type_id")
            .table(Event::Table)
            .col(Event::AggregateType)
            .col(Event::AggregateId)
            .to_owned();

        let idx_event_routing_key_type = Index::create()
            .if_not_exists()
            .name("idx_event_routing_key_type")
            .table(Event::Table)
            .col(Event::RoutingKey)
            .col(Event::AggregateType)
            .to_owned();

        let idx_event_type_id_version = Index::create()
            .if_not_exists()
            .name("idx_event_type_id_version")
            .table(Event::Table)
            .unique()
            .col(Event::AggregateType)
            .col(Event::AggregateId)
            .col(Event::Version)
            .to_owned();

        let snapshot_table = Table::create()
            .table(Snapshot::Table)
            .if_not_exists()
            .col(
                ColumnDef::new(Snapshot::Id)
                    .string()
                    .not_null()
                    .string_len(26),
            )
            .col(
                ColumnDef::new(Snapshot::Type)
                    .string()
                    .string_len(50)
                    .not_null(),
            )
            .col(ColumnDef::new(Snapshot::Cursor).string().not_null())
            .col(ColumnDef::new(Snapshot::Revision).string().not_null())
            .col(ColumnDef::new(Snapshot::Data).blob().not_null())
            .col(
                ColumnDef::new(Snapshot::CreatedAt)
                    .integer()
                    .not_null()
                    .default(Expr::custom_keyword("(strftime('%s', 'now'))")),
            )
            .col(ColumnDef::new(Snapshot::UpdatedAt).integer().null())
            .primary_key(Index::create().col(Snapshot::Type).col(Snapshot::Id))
            .to_owned();

        match DB::NAME {
            "SQLite" => [
                event_table.to_string(SqliteQueryBuilder),
                idx_event_type.to_string(SqliteQueryBuilder),
                idx_event_type_id.to_string(SqliteQueryBuilder),
                idx_event_routing_key_type.to_string(SqliteQueryBuilder),
                idx_event_type_id_version.to_string(SqliteQueryBuilder),
                snapshot_table.to_string(SqliteQueryBuilder),
            ]
            .join(";\n"),
            name => panic!("'{name}' not supported, consider using SQLite"),
        }
    }
}

#[async_trait::async_trait]
impl<DB> Executor for Sql<DB>
where
    DB: Database,
    for<'c> &'c mut DB::Connection: sqlx::Executor<'c, Database = DB>,
    sea_query_binder::SqlxValues: for<'q> sqlx::IntoArguments<'q, DB>,
    String: for<'r> sqlx::Decode<'r, DB> + sqlx::Type<DB>,
    Vec<u8>: for<'r> sqlx::Decode<'r, DB> + sqlx::Type<DB>,
    usize: sqlx::ColumnIndex<DB::Row>,
    crate::Event: for<'r> sqlx::FromRow<'r, DB::Row>,
{
    async fn get_event<A: Aggregator>(&self, cursor: Value) -> Result<crate::Event, ReadError> {
        todo!()
    }

    async fn read<A: Aggregator>(
        &self,
        id: String,
        args: Args,
    ) -> Result<ReadResult<crate::Event>, ReadError> {
        let statement = Query::select()
            .columns([
                Event::Id,
                Event::Name,
                Event::AggregateType,
                Event::AggregateId,
                Event::Version,
                Event::Data,
                Event::Metadata,
                Event::RoutingKey,
                Event::Timestamp,
            ])
            .from(Event::Table)
            .and_where(Expr::col(Event::AggregateType).eq(Expr::value(A::name())))
            .and_where(Expr::col(Event::AggregateId).eq(Expr::value(id)))
            .to_owned();

        Reader::new(statement)
            .args(args)
            .execute::<_, crate::Event, _>(&self.0)
            .await
    }

    async fn get_snapshot<A: Aggregator>(
        &self,
        id: String,
    ) -> Result<Option<(Vec<u8>, Value)>, ReadError> {
        let query = Query::select()
            .columns([Snapshot::Data, Snapshot::Cursor])
            .from(Snapshot::Table)
            .and_where(Expr::col(Snapshot::Type).eq(Expr::value(A::name())))
            .and_where(Expr::col(Snapshot::Id).eq(Expr::value(id)))
            .and_where(Expr::col(Snapshot::Revision).eq(Expr::value(A::revision())))
            .limit(1)
            .to_owned();

        let (sql, values) = match DB::NAME {
            "SQLite" => query.build_sqlx(SqliteQueryBuilder),
            name => panic!("'{name}' not supported, consider using SQLite"),
        };

        sqlx::query_as_with::<DB, (Vec<u8>, String), _>(&sql, values)
            .fetch_optional(&self.0)
            .await
            .map(|res| res.map(|(data, cursor)| (data, cursor.into())))
            .map_err(|err| ReadError::Unknown(err.into()))
    }

    async fn write(&self, events: Vec<crate::Event>) -> Result<(), WriteError> {
        let mut query = Query::insert()
            .into_table(Event::Table)
            .columns([
                Event::Id,
                Event::Name,
                Event::Data,
                Event::Metadata,
                Event::AggregateType,
                Event::AggregateId,
                Event::Version,
                Event::RoutingKey,
            ])
            .to_owned();

        for event in events {
            query.values_panic([
                event.id.to_string().into(),
                event.name.into(),
                event.data.into(),
                event.metadata.into(),
                event.aggregate_type.into(),
                event.aggregate_id.to_string().into(),
                event.version.into(),
                event.routing_key.into(),
            ]);
        }

        let (sql, values) = match DB::NAME {
            "SQLite" => query.build_sqlx(SqliteQueryBuilder),
            name => panic!("'{name}' not supported, consider using SQLite"),
        };

        sqlx::query_with::<DB, _>(&sql, values)
            .execute(&self.0)
            .await
            .map_err(|err| {
                if err.to_string().contains("(code: 2067)") {
                    WriteError::InvalidOriginalVersion
                } else {
                    WriteError::Unknown(err.into())
                }
            })?;

        Ok(())
    }

    async fn save_snapshot<A: Aggregator>(
        &self,
        event: crate::Event,
        data: Vec<u8>,
        cursor: Value,
    ) -> Result<(), WriteError> {
        let query = Query::insert()
            .into_table(Snapshot::Table)
            .columns([
                Snapshot::Type,
                Snapshot::Id,
                Snapshot::Cursor,
                Snapshot::Revision,
                Snapshot::Data,
            ])
            .values_panic([
                event.aggregate_type.into(),
                event.aggregate_id.to_string().into(),
                cursor.to_string().into(),
                A::revision().into(),
                data.into(),
            ])
            .on_conflict(
                OnConflict::columns([Snapshot::Type, Snapshot::Id])
                    .update_columns([Snapshot::Data, Snapshot::Cursor, Snapshot::Revision])
                    .value(
                        Snapshot::UpdatedAt,
                        Expr::custom_keyword("(strftime('%s', 'now'))"),
                    )
                    .to_owned(),
            )
            .to_owned();

        let (sql, values) = match DB::NAME {
            "SQLite" => query.build_sqlx(SqliteQueryBuilder),
            name => panic!("'{name}' not supported, consider using SQLite"),
        };

        sqlx::query_with::<DB, _>(&sql, values)
            .execute(&self.0)
            .await
            .map_err(|err| WriteError::Unknown(err.into()))?;

        Ok(())
    }
}

impl<D: Database> From<Pool<D>> for Sql<D> {
    fn from(value: Pool<D>) -> Self {
        Self(value)
    }
}

pub struct Reader {
    statement: SelectStatement,
    args: Args,
    order: cursor::Order,
}

impl Reader {
    pub fn new(statement: SelectStatement) -> Self {
        Self {
            statement,
            args: Args::default(),
            order: cursor::Order::Asc,
        }
    }

    pub fn order(&mut self, order: cursor::Order) -> &mut Self {
        self.order = order;

        self
    }

    pub fn desc(&mut self) -> &mut Self {
        self.order(cursor::Order::Desc)
    }

    pub fn args(&mut self, args: Args) -> &mut Self {
        self.args = args;

        self
    }

    pub fn backward(&mut self, last: u16, before: Option<Value>) -> &mut Self {
        self.args(Args {
            last: Some(last),
            before,
            ..Default::default()
        })
    }

    pub fn forward(&mut self, first: u16, after: Option<Value>) -> &mut Self {
        self.args(Args {
            first: Some(first),
            after,
            ..Default::default()
        })
    }

    pub async fn execute<'e, 'c: 'e, DB, O, E>(
        &mut self,
        executor: E,
    ) -> Result<ReadResult<O>, ReadError>
    where
        DB: Database,
        E: 'e + sqlx::Executor<'c, Database = DB>,
        O: for<'r> sqlx::FromRow<'r, DB::Row>,
        O: Cursor,
        O: Send + Unpin,
        O: Bind<Cursor = O>,
        <<O as Bind>::I as IntoIterator>::IntoIter: DoubleEndedIterator,
        <<O as Bind>::V as IntoIterator>::IntoIter: DoubleEndedIterator,
        sea_query_binder::SqlxValues: for<'q> sqlx::IntoArguments<'q, DB>,
    {
        let limit = self.build_reader::<O, O>()?;

        let (sql, values) = match DB::NAME {
            "SQLite" => self.build_sqlx(SqliteQueryBuilder),
            name => panic!("'{name}' not supported, consider using SQLite"),
        };

        let mut rows = sqlx::query_as_with::<DB, O, _>(&sql, values)
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

        if self.args.is_backward() {
            edges = edges.into_iter().rev().collect();
        }

        let page_info = if self.args.is_backward() {
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

    fn build_reader<O: Cursor, B: Bind<Cursor = O>>(&mut self) -> Result<u16, ReadError>
    where
        B::T: Clone,
        <<B as Bind>::I as IntoIterator>::IntoIter: DoubleEndedIterator,
        <<B as Bind>::V as IntoIterator>::IntoIter: DoubleEndedIterator,
    {
        let (limit, cursor) = self.args.get_info();

        if let Some(cursor) = cursor.as_ref() {
            self.build_reader_where::<O, B>(cursor)?;
        }

        self.build_reader_order::<B>();
        self.limit((limit + 1).into());

        Ok(limit)
    }

    fn build_reader_where<O, B>(&mut self, cursor: &Value) -> Result<(), ReadError>
    where
        O: Cursor,
        B: Bind<Cursor = O>,
        B::T: Clone,
        <<B as Bind>::I as IntoIterator>::IntoIter: DoubleEndedIterator,
        <<B as Bind>::V as IntoIterator>::IntoIter: DoubleEndedIterator,
    {
        let is_order_desc = self.is_order_desc();
        let cursor = O::deserialize_cursor(cursor)?;
        let colums = B::columns().into_iter().rev();
        let values = B::values(cursor).into_iter().rev();

        let mut expr = None::<Expr>;
        for (col, value) in colums.zip(values) {
            let current_expr = if is_order_desc {
                Expr::col(col.clone()).lt(value.clone())
            } else {
                Expr::col(col.clone()).gt(value.clone())
            };

            let Some(ref prev_expr) = expr else {
                expr = Some(current_expr.clone());
                continue;
            };

            expr = Some(current_expr.or(Expr::col(col).eq(value).and(prev_expr.clone())));
        }

        self.and_where(expr.unwrap());

        Ok(())
    }

    fn build_reader_order<O: Bind>(&mut self) {
        let order = if self.is_order_desc() {
            sea_query::Order::Desc
        } else {
            sea_query::Order::Asc
        };

        let colums = O::columns();
        for col in colums {
            self.order_by(col, order.clone());
        }
    }

    fn is_order_desc(&self) -> bool {
        matches!(
            (&self.order, self.args.is_backward()),
            (cursor::Order::Asc, true) | (cursor::Order::Desc, false)
        )
    }
}

impl Deref for Reader {
    type Target = SelectStatement;

    fn deref(&self) -> &Self::Target {
        &self.statement
    }
}

impl DerefMut for Reader {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.statement
    }
}

pub trait Bind {
    type T: IntoColumnRef + Clone;
    type I: IntoIterator<Item = Self::T>;
    type V: IntoIterator<Item = Expr>;
    type Cursor: Cursor;

    fn columns() -> Self::I;
    fn values(cursor: <<Self as Bind>::Cursor as Cursor>::T) -> Self::V;
}

impl Bind for crate::Event {
    type T = Event;
    type I = [Self::T; 3];
    type V = [Expr; 3];
    type Cursor = Self;

    fn columns() -> Self::I {
        [Event::Timestamp, Event::Version, Event::Id]
    }

    fn values(cursor: <<Self as Bind>::Cursor as Cursor>::T) -> Self::V {
        [
            cursor.t.into(),
            cursor.v.into(),
            cursor.i.to_string().into(),
        ]
    }
}

impl<R: sqlx::Row> sqlx::FromRow<'_, R> for crate::Event
where
    u32: sqlx::Type<R::Database> + for<'r> sqlx::Decode<'r, R::Database>,
    u16: sqlx::Type<R::Database> + for<'r> sqlx::Decode<'r, R::Database>,
    Vec<u8>: sqlx::Type<R::Database> + for<'r> sqlx::Decode<'r, R::Database>,
    String: sqlx::Type<R::Database> + for<'r> sqlx::Decode<'r, R::Database>,
    for<'r> &'r str: sqlx::Type<R::Database> + sqlx::Decode<'r, R::Database>,
    for<'r> &'r str: sqlx::ColumnIndex<R>,
{
    fn from_row(row: &R) -> Result<Self, sqlx::Error> {
        Ok(crate::Event {
            id: Ulid::from_string(row.try_get("id")?)
                .map_err(|err| sqlx::Error::InvalidArgument(err.to_string()))?,
            aggregate_id: Ulid::from_string(row.try_get("aggregate_id")?)
                .map_err(|err| sqlx::Error::InvalidArgument(err.to_string()))?,
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
