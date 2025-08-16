use crate::product::QueryProductIden;
use sea_query::{ColumnDef, Expr, SqliteQueryBuilder, Table};
use sqlx::SqliteConnection;
use sqlx_migrator::{Operation, vec_box};

pub struct CreateProductTableOperation;

#[async_trait::async_trait]
impl Operation<sqlx::Sqlite> for CreateProductTableOperation {
    async fn up(&self, connection: &mut SqliteConnection) -> Result<(), sqlx_migrator::Error> {
        let statement = Table::create()
            .table(QueryProductIden::Table)
            .col(
                ColumnDef::new(QueryProductIden::Id)
                    .string()
                    .string_len(26)
                    .primary_key()
                    .not_null(),
            )
            .col(
                ColumnDef::new(QueryProductIden::Name)
                    .string()
                    .string_len(50)
                    .not_null(),
            )
            .col(
                ColumnDef::new(QueryProductIden::State)
                    .string()
                    .string_len(15)
                    .not_null(),
            )
            .col(
                ColumnDef::new(QueryProductIden::FailedReason)
                    .string()
                    .string_len(100)
                    .not_null()
                    .default(""),
            )
            .col(
                ColumnDef::new(QueryProductIden::CreatedAt)
                    .timestamp_with_time_zone()
                    .not_null()
                    .default(Expr::current_timestamp()),
            )
            .to_string(SqliteQueryBuilder);

        sqlx::query(&statement).execute(connection).await?;

        Ok(())
    }

    async fn down(&self, connection: &mut SqliteConnection) -> Result<(), sqlx_migrator::Error> {
        let statement = Table::drop()
            .table(QueryProductIden::Table)
            .to_string(SqliteQueryBuilder);

        sqlx::query(&statement).execute(connection).await?;

        Ok(())
    }
}

pub struct Market202508160417;

sqlx_migrator::sqlite_migration!(
    Market202508160417,
    "main",
    "market_2025_08_16_04_17",
    vec_box![],
    vec_box![CreateProductTableOperation]
);
