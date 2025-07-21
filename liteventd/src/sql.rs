use std::ops::Shl;

use sea_query::{
    ColumnDef, Expr, ExprTrait, Iden, Index, OnConflict, Query, SqliteQueryBuilder, Table,
};
use sea_query_binder::SqlxBinder;
use sqlx::{Database, Pool};

use crate::{
    Aggregator, Event, EventIden, Executor, ReadError, WriteError,
    cursor::{Args, ReadResult, Value},
};

pub struct Sql<DB: Database>(Pool<DB>);

impl<DB: Database> Sql<DB> {
    pub fn get_schema() -> String {
        let event_table = Table::create()
            .table(EventIden::Table)
            .if_not_exists()
            .col(
                ColumnDef::new(EventIden::Id)
                    .string()
                    .not_null()
                    .string_len(26)
                    .primary_key(),
            )
            .col(
                ColumnDef::new(EventIden::Name)
                    .string()
                    .string_len(20)
                    .not_null(),
            )
            .col(
                ColumnDef::new(EventIden::AggregateType)
                    .string()
                    .string_len(50)
                    .not_null(),
            )
            .col(
                ColumnDef::new(EventIden::AggregateId)
                    .string()
                    .string_len(26)
                    .not_null(),
            )
            .col(ColumnDef::new(EventIden::Version).integer().not_null())
            .col(ColumnDef::new(EventIden::Data).blob().not_null())
            .col(ColumnDef::new(EventIden::Metadata).blob().not_null())
            .col(
                ColumnDef::new(EventIden::RoutingKey)
                    .string()
                    .string_len(50)
                    .not_null(),
            )
            .col(
                ColumnDef::new(EventIden::Timestamp)
                    .integer()
                    .not_null()
                    .default(Expr::custom_keyword("(strftime('%s', 'now'))")),
            )
            .to_owned();

        let idx_event_type = Index::create()
            .if_not_exists()
            .name("idx_event_type")
            .table(EventIden::Table)
            .col(EventIden::AggregateType)
            .to_owned();

        let idx_event_type_id = Index::create()
            .if_not_exists()
            .name("idx_event_type_id")
            .table(EventIden::Table)
            .col(EventIden::AggregateType)
            .col(EventIden::AggregateId)
            .to_owned();

        let idx_event_routing_key_type = Index::create()
            .if_not_exists()
            .name("idx_event_routing_key_type")
            .table(EventIden::Table)
            .col(EventIden::RoutingKey)
            .col(EventIden::AggregateType)
            .to_owned();

        let idx_event_type_id_version = Index::create()
            .if_not_exists()
            .name("idx_event_type_id_version")
            .table(EventIden::Table)
            .unique()
            .col(EventIden::AggregateType)
            .col(EventIden::AggregateId)
            .col(EventIden::Version)
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
    for<'args> sea_query_binder::SqlxValues: sqlx::IntoArguments<'args, DB>,
    for<'args> String: sqlx::Decode<'args, DB> + sqlx::Type<DB>,
    for<'args> Vec<u8>: sqlx::Decode<'args, DB> + sqlx::Type<DB>,
    for<'args> usize: sqlx::ColumnIndex<DB::Row>,
{
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

    async fn write(&self, events: Vec<Event>) -> Result<(), WriteError> {
        let mut query = Query::insert()
            .into_table(EventIden::Table)
            .columns([
                EventIden::Id,
                EventIden::Name,
                EventIden::Data,
                EventIden::Metadata,
                EventIden::AggregateType,
                EventIden::AggregateId,
                EventIden::Version,
                EventIden::RoutingKey,
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
        event: Event,
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
