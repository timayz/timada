//! Provider abstraction for timada e-commerce stores.
//!
//! A [`Provider`] is a source of products and a fulfillment channel: the
//! store's own inventory, a dropshipping supplier like AliExpress, or any
//! other integration. Implementations are registered once at startup in a
//! [`ProviderRegistry`]; *connections* to a provider (credentials, enabled
//! state) are runtime data owned by the store's domain layer and handed to
//! every call through a [`ProviderContext`].

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

/// A monetary amount in minor units (e.g. cents) plus an ISO 4217 currency.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Money {
    pub amount_minor: i64,
    pub currency: String,
}

/// A purchasable variant of a [`SourceProduct`] (e.g. "Color: Red / Size: M").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceVariant {
    /// Provider-side identifier for this variant.
    pub external_ref: String,
    pub title: String,
    pub price: Money,
    /// Option name/value pairs, e.g. `("Color", "Red")`.
    pub options: Vec<(String, String)>,
}

/// A product as described by a provider, before it is imported into a store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceProduct {
    /// Provider-side identifier for this product.
    pub external_ref: String,
    pub title: String,
    pub description: String,
    pub image_urls: Vec<String>,
    pub price: Money,
    pub variants: Vec<SourceVariant>,
}

/// One page of provider search results.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourcePage {
    pub items: Vec<SourceProduct>,
    /// Opaque cursor to request the next page, if any.
    pub next_cursor: Option<String>,
}

/// Stock available at the provider for one product or variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StockLevel {
    pub external_ref: String,
    pub available: i64,
}

/// A shipping destination for a fulfillment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Address {
    pub full_name: String,
    pub line1: String,
    pub line2: Option<String>,
    pub city: String,
    pub postal_code: String,
    /// ISO 3166-1 alpha-2 country code.
    pub country: String,
}

/// One order line to fulfill at the provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FulfillmentLine {
    pub external_ref: String,
    pub quantity: u32,
}

/// A request to fulfill (part of) a store order through a provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FulfillmentRequest {
    /// The store-side order reference, for idempotency at the provider.
    pub order_ref: String,
    pub lines: Vec<FulfillmentLine>,
    pub shipping_address: Address,
}

/// Provider acknowledgement of an accepted fulfillment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FulfillmentReceipt {
    /// Provider-side identifier used to query [`Provider::tracking`].
    pub fulfillment_ref: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackingState {
    Pending,
    Shipped,
    Delivered,
    Cancelled,
}

/// Shipment progress for an accepted fulfillment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackingStatus {
    pub carrier: Option<String>,
    pub tracking_number: Option<String>,
    pub state: TrackingState,
}

/// One field of a provider's connection configuration, used by the admin UI
/// to render a credentials form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfigField {
    pub key: &'static str,
    pub label: &'static str,
    /// Render as a password input and never echo the stored value.
    pub secret: bool,
}

/// Per-connection runtime configuration, resolved by the caller from the
/// store's provider-connection state and passed on every call.
#[derive(Debug, Clone, Default)]
pub struct ProviderContext {
    pub connection_id: String,
    /// Credentials and settings keyed by [`ConfigField::key`].
    pub config: BTreeMap<String, String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("authentication with the provider failed")]
    Auth,
    #[error("not found at provider: {0}")]
    NotFound(String),
    #[error("rate limited by the provider")]
    RateLimited,
    #[error("operation not supported by this provider")]
    Unsupported,
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// A source of products and a fulfillment channel.
///
/// Implementations are stateless with respect to any particular connection:
/// credentials arrive in the [`ProviderContext`] on every call, so one
/// registered instance serves every configured connection of its kind.
#[async_trait::async_trait]
pub trait Provider: Send + Sync + 'static {
    /// Stable key persisted in the store's events, e.g. `"self_inventory"`,
    /// `"aliexpress"`. Never change it for a published provider.
    fn kind(&self) -> &'static str;

    fn display_name(&self) -> &'static str;

    /// Configuration fields the admin UI renders as a credentials form.
    /// Empty means the provider needs no configuration.
    fn config_schema(&self) -> Vec<ConfigField>;

    /// Search the provider's catalog. `cursor` is the `next_cursor` of a
    /// previous [`SourcePage`].
    async fn search_products(
        &self,
        ctx: &ProviderContext,
        query: &str,
        cursor: Option<String>,
    ) -> Result<SourcePage, ProviderError>;

    /// Fetch one product by its provider-side reference.
    async fn fetch_product(
        &self,
        ctx: &ProviderContext,
        external_ref: &str,
    ) -> Result<SourceProduct, ProviderError>;

    /// Current stock available at the provider for one product or variant.
    async fn stock(
        &self,
        ctx: &ProviderContext,
        external_ref: &str,
    ) -> Result<StockLevel, ProviderError>;

    /// Forward an order (or part of one) to the provider for fulfillment.
    async fn fulfill(
        &self,
        ctx: &ProviderContext,
        request: &FulfillmentRequest,
    ) -> Result<FulfillmentReceipt, ProviderError>;

    /// Shipment progress for a previously accepted fulfillment.
    async fn tracking(
        &self,
        ctx: &ProviderContext,
        fulfillment_ref: &str,
    ) -> Result<TrackingStatus, ProviderError>;
}

/// Compile-time registration, runtime lookup by [`Provider::kind`].
///
/// Built once at startup by the host application and shared through app
/// state as an `Arc<ProviderRegistry>`.
#[derive(Default)]
pub struct ProviderRegistry {
    inner: HashMap<&'static str, Arc<dyn Provider>>,
}

impl ProviderRegistry {
    #[must_use]
    pub fn register(mut self, provider: Arc<dyn Provider>) -> Self {
        self.inner.insert(provider.kind(), provider);
        self
    }

    pub fn get(&self, kind: &str) -> Option<Arc<dyn Provider>> {
        self.inner.get(kind).cloned()
    }

    pub fn contains(&self, kind: &str) -> bool {
        self.inner.contains_key(kind)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Arc<dyn Provider>> {
        self.inner.values()
    }
}
