//! AliExpress provider for timada stores.
//!
//! Currently a deterministic stub: every operation returns
//! [`ProviderError::Unsupported`]. The real AliExpress API integration will
//! land behind the same [`Provider`] trait.

use timada_provider::{
    ConfigField, FulfillmentReceipt, FulfillmentRequest, Provider, ProviderContext, ProviderError,
    SourcePage, SourceProduct, StockLevel, TrackingStatus,
};

/// AliExpress dropshipping provider.
#[derive(Debug, Default)]
pub struct AliExpress;

#[async_trait::async_trait]
impl Provider for AliExpress {
    fn kind(&self) -> &'static str {
        "aliexpress"
    }

    fn display_name(&self) -> &'static str {
        "AliExpress"
    }

    fn config_schema(&self) -> Vec<ConfigField> {
        vec![
            ConfigField {
                key: "app_key",
                label: "App key",
                secret: false,
            },
            ConfigField {
                key: "app_secret",
                label: "App secret",
                secret: true,
            },
        ]
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
