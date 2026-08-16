use std::sync::Arc;

use evento::migrator::Migrate as _;
use sqlx::sqlite::SqlitePoolOptions;
use timada::provider_connection;
use timada_provider::{
    ConfigField, FulfillmentReceipt, FulfillmentRequest, Provider, ProviderContext, ProviderError,
    ProviderRegistry, SourcePage, SourceProduct, StockLevel, TrackingStatus,
};

/// Minimal provider with a non-empty config schema, so enable-without-
/// credentials can be exercised without depending on a real provider crate.
struct TestProvider;

#[async_trait::async_trait]
impl Provider for TestProvider {
    fn kind(&self) -> &'static str {
        "test"
    }

    fn display_name(&self) -> &'static str {
        "Test Provider"
    }

    fn config_schema(&self) -> Vec<ConfigField> {
        vec![ConfigField {
            key: "api_key",
            label: "API key",
            secret: true,
        }]
    }

    async fn search_products(
        &self,
        _ctx: &ProviderContext,
        _query: &str,
        _cursor: Option<String>,
    ) -> Result<SourcePage, ProviderError> {
        Err(ProviderError::Unsupported)
    }

    async fn fetch_product(
        &self,
        _ctx: &ProviderContext,
        _external_ref: &str,
    ) -> Result<SourceProduct, ProviderError> {
        Err(ProviderError::Unsupported)
    }

    async fn stock(
        &self,
        _ctx: &ProviderContext,
        _external_ref: &str,
    ) -> Result<StockLevel, ProviderError> {
        Err(ProviderError::Unsupported)
    }

    async fn fulfill(
        &self,
        _ctx: &ProviderContext,
        _request: &FulfillmentRequest,
    ) -> Result<FulfillmentReceipt, ProviderError> {
        Err(ProviderError::Unsupported)
    }

    async fn tracking(
        &self,
        _ctx: &ProviderContext,
        _fulfillment_ref: &str,
    ) -> Result<TrackingStatus, ProviderError> {
        Err(ProviderError::Unsupported)
    }
}

fn registry() -> ProviderRegistry {
    ProviderRegistry::default().register(Arc::new(TestProvider))
}

/// In-memory event store. One connection so both halves of the Rw executor
/// see the same database.
async fn executor() -> anyhow::Result<evento::sql::RwSqlite> {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .idle_timeout(None)
        .max_lifetime(None)
        .connect("sqlite::memory:")
        .await?;

    let mut conn = pool.acquire().await?;
    evento::sql_migrator::new::<sqlx::Sqlite>()?
        .run(&mut *conn, &evento::migrator::Plan::apply_all())
        .await?;
    drop(conn);

    Ok((pool.clone().into(), pool.into()).into())
}

#[tokio::test]
async fn connect_unknown_kind_is_refused() {
    let executor = executor().await.unwrap();

    let result =
        provider_connection::connect_provider(&executor, &registry(), "does-not-exist").await;

    assert!(result.is_err());
}

#[tokio::test]
async fn connect_known_kind_records_the_connection() {
    let executor = executor().await.unwrap();

    let id = provider_connection::connect_provider(&executor, &registry(), "test")
        .await
        .unwrap();

    let state = provider_connection::load(&executor, &id).await.unwrap().unwrap();
    assert_eq!(state.provider_kind, "test");
    assert_eq!(state.display_name, "Test Provider");
    assert!(!state.enabled);
    assert!(state.config.is_empty());
}

#[tokio::test]
async fn enable_without_credentials_is_refused() {
    let executor = executor().await.unwrap();
    let registry = registry();
    let id = provider_connection::connect_provider(&executor, &registry, "test")
        .await
        .unwrap();

    let result = provider_connection::enable_provider(&executor, &registry, &id).await;

    assert!(result.is_err());
}

#[tokio::test]
async fn enable_after_credentials_succeeds() {
    let executor = executor().await.unwrap();
    let registry = registry();
    let id = provider_connection::connect_provider(&executor, &registry, "test")
        .await
        .unwrap();

    provider_connection::configure_provider_credentials(
        &executor,
        &id,
        vec![("api_key".to_owned(), "secret".to_owned())],
    )
    .await
    .unwrap();
    provider_connection::enable_provider(&executor, &registry, &id)
        .await
        .unwrap();

    let state = provider_connection::load(&executor, &id).await.unwrap().unwrap();
    assert!(state.enabled);
}

#[tokio::test]
async fn disable_when_already_disabled_is_refused() {
    let executor = executor().await.unwrap();
    let id = provider_connection::connect_provider(&executor, &registry(), "test")
        .await
        .unwrap();

    let result = provider_connection::disable_provider(&executor, &id).await;

    assert!(result.is_err());
}
