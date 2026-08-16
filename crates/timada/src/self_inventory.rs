//! The built-in provider backed by the store's own catalog and stock.
//!
//! For self-inventory products the provider-side reference *is* the product
//! id. Fulfillment (mark-as-shipped semantics) arrives with the orders
//! milestone.

use sqlx::SqlitePool;
use timada_provider::{
    ConfigField, FulfillmentReceipt, FulfillmentRequest, Money, Provider, ProviderContext,
    ProviderError, SourcePage, SourceProduct, SourceVariant, StockLevel, TrackingStatus,
};

use crate::read_model::{catalog_detail, stock_levels};

/// Stable provider kind for self-inventory, persisted in events.
pub const KIND: &str = "self_inventory";

pub struct SelfInventory {
    read_pool: SqlitePool,
}

impl SelfInventory {
    /// `read_pool` should be the read-only pool from
    /// [`crate::db::create_read_pool`].
    pub fn new(read_pool: SqlitePool) -> Self {
        Self { read_pool }
    }
}

fn to_source(detail: catalog_detail::CatalogDetail) -> SourceProduct {
    SourceProduct {
        external_ref: detail.id,
        title: detail.title,
        description: detail.description,
        image_urls: detail.image_urls,
        price: Money {
            amount_minor: detail.amount_minor,
            currency: detail.currency,
        },
        variants: detail
            .variants
            .into_iter()
            .map(|variant| SourceVariant {
                external_ref: variant.external_ref,
                title: variant.title,
                price: Money {
                    amount_minor: variant.price_amount_minor,
                    currency: variant.currency,
                },
                options: variant.options,
            })
            .collect(),
    }
}

#[async_trait::async_trait]
impl Provider for SelfInventory {
    fn kind(&self) -> &'static str {
        KIND
    }

    fn display_name(&self) -> &'static str {
        "Self inventory"
    }

    fn config_schema(&self) -> Vec<ConfigField> {
        Vec::new()
    }

    async fn search_products(
        &self,
        _ctx: &ProviderContext,
        query: &str,
        _cursor: Option<String>,
    ) -> Result<SourcePage, ProviderError> {
        let items = catalog_detail::search_self_inventory(&self.read_pool, query, 20)
            .await?
            .into_iter()
            .map(to_source)
            .collect();

        Ok(SourcePage {
            items,
            next_cursor: None,
        })
    }

    async fn fetch_product(
        &self,
        _ctx: &ProviderContext,
        external_ref: &str,
    ) -> Result<SourceProduct, ProviderError> {
        let detail = catalog_detail::by_id(&self.read_pool, external_ref)
            .await?
            .filter(|detail| detail.provider_kind == KIND)
            .ok_or_else(|| ProviderError::NotFound(external_ref.to_owned()))?;

        Ok(to_source(detail))
    }

    async fn stock(
        &self,
        _ctx: &ProviderContext,
        external_ref: &str,
    ) -> Result<StockLevel, ProviderError> {
        let available = stock_levels::available(&self.read_pool, external_ref).await?;
        Ok(StockLevel {
            external_ref: external_ref.to_owned(),
            available,
        })
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
