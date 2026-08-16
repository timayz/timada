//! AliExpress provider for timada stores.
//!
//! Catalog sourcing and stock are currently backed by a deterministic
//! in-memory stub so the import flow can be exercised end to end; the real
//! AliExpress API integration will land behind the same [`Provider`] trait.
//! Fulfillment is not implemented yet.

use timada_provider::{
    ConfigField, FulfillmentReceipt, FulfillmentRequest, Money, Provider, ProviderContext,
    ProviderError, SourcePage, SourceProduct, SourceVariant, StockLevel, TrackingStatus,
};

/// AliExpress dropshipping provider.
#[derive(Debug, Default)]
pub struct AliExpress;

fn usd(amount_minor: i64) -> Money {
    Money {
        amount_minor,
        currency: "USD".to_owned(),
    }
}

/// The deterministic stub catalog.
fn catalog() -> Vec<SourceProduct> {
    vec![
        SourceProduct {
            external_ref: "ae-1001".to_owned(),
            title: "Wireless Earbuds Pro".to_owned(),
            description: "Bluetooth 5.3 earbuds with noise cancellation and charging case."
                .to_owned(),
            image_urls: vec!["https://picsum.photos/seed/ae-1001/600".to_owned()],
            price: usd(1899),
            variants: vec![
                SourceVariant {
                    external_ref: "ae-1001-black".to_owned(),
                    title: "Black".to_owned(),
                    price: usd(1899),
                    options: vec![("Color".to_owned(), "Black".to_owned())],
                },
                SourceVariant {
                    external_ref: "ae-1001-white".to_owned(),
                    title: "White".to_owned(),
                    price: usd(1999),
                    options: vec![("Color".to_owned(), "White".to_owned())],
                },
            ],
        },
        SourceProduct {
            external_ref: "ae-1002".to_owned(),
            title: "Stainless Steel Water Bottle 750ml".to_owned(),
            description: "Double-wall vacuum insulated bottle, keeps drinks cold for 24h."
                .to_owned(),
            image_urls: vec!["https://picsum.photos/seed/ae-1002/600".to_owned()],
            price: usd(1250),
            variants: Vec::new(),
        },
        SourceProduct {
            external_ref: "ae-1003".to_owned(),
            title: "LED Desk Lamp with USB Charging".to_owned(),
            description: "Dimmable desk lamp, three color temperatures, foldable arm.".to_owned(),
            image_urls: vec!["https://picsum.photos/seed/ae-1003/600".to_owned()],
            price: usd(2340),
            variants: Vec::new(),
        },
        SourceProduct {
            external_ref: "ae-1004".to_owned(),
            title: "Laptop Stand Aluminium".to_owned(),
            description: "Ergonomic adjustable laptop riser for 10–17\" laptops.".to_owned(),
            image_urls: vec!["https://picsum.photos/seed/ae-1004/600".to_owned()],
            price: usd(2799),
            variants: Vec::new(),
        },
        SourceProduct {
            external_ref: "ae-1005".to_owned(),
            title: "Mechanical Keyboard 75%".to_owned(),
            description: "Hot-swappable RGB mechanical keyboard with knob.".to_owned(),
            image_urls: vec!["https://picsum.photos/seed/ae-1005/600".to_owned()],
            price: usd(5499),
            variants: vec![
                SourceVariant {
                    external_ref: "ae-1005-red".to_owned(),
                    title: "Red switches".to_owned(),
                    price: usd(5499),
                    options: vec![("Switch".to_owned(), "Red".to_owned())],
                },
                SourceVariant {
                    external_ref: "ae-1005-brown".to_owned(),
                    title: "Brown switches".to_owned(),
                    price: usd(5499),
                    options: vec![("Switch".to_owned(), "Brown".to_owned())],
                },
            ],
        },
        SourceProduct {
            external_ref: "ae-1006".to_owned(),
            title: "Phone Tripod with Remote".to_owned(),
            description: "Extendable tripod with Bluetooth shutter remote.".to_owned(),
            image_urls: vec!["https://picsum.photos/seed/ae-1006/600".to_owned()],
            price: usd(1590),
            variants: Vec::new(),
        },
    ]
}

/// The stub still enforces authentication so the credentials flow is
/// exercised: an `app_key` must be configured.
fn authenticate(ctx: &ProviderContext) -> Result<(), ProviderError> {
    match ctx.config.get("app_key") {
        Some(key) if !key.trim().is_empty() => Ok(()),
        _ => Err(ProviderError::Auth),
    }
}

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
        ctx: &ProviderContext,
        query: &str,
        _cursor: Option<String>,
    ) -> Result<SourcePage, ProviderError> {
        authenticate(ctx)?;

        let query = query.trim().to_lowercase();
        let items = catalog()
            .into_iter()
            .filter(|product| query.is_empty() || product.title.to_lowercase().contains(&query))
            .collect();

        Ok(SourcePage {
            items,
            next_cursor: None,
        })
    }

    async fn fetch_product(
        &self,
        ctx: &ProviderContext,
        external_ref: &str,
    ) -> Result<SourceProduct, ProviderError> {
        authenticate(ctx)?;

        catalog()
            .into_iter()
            .find(|product| product.external_ref == external_ref)
            .ok_or_else(|| ProviderError::NotFound(external_ref.to_owned()))
    }

    async fn stock(
        &self,
        ctx: &ProviderContext,
        external_ref: &str,
    ) -> Result<StockLevel, ProviderError> {
        authenticate(ctx)?;

        if !catalog()
            .iter()
            .any(|product| product.external_ref == external_ref)
        {
            return Err(ProviderError::NotFound(external_ref.to_owned()));
        }
        Ok(StockLevel {
            external_ref: external_ref.to_owned(),
            available: 250,
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
