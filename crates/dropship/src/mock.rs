//! An in-process supplier that fakes the whole vendor round trip, so the
//! storefront slice can be demoed and tested without a network.

use std::collections::HashMap;
use std::sync::Mutex;

use timada_core::{Currency, Money, new_id};

use crate::supplier::{
    Supplier, SupplierConfirmation, SupplierError, SupplierOrderRequest, SupplierProduct,
    TrackingStatus,
};

/// Fake supplier: fixed catalog, instant order confirmation, and a tracking
/// status that advances one step per [`Supplier::track_shipment`] call so the
/// admin "refresh tracking" button visibly drives the shipment forward.
#[derive(Debug, Default)]
pub struct MockSupplier {
    /// `external_ref` -> number of tracking polls served so far.
    tracking_polls: Mutex<HashMap<String, u8>>,
}

impl MockSupplier {
    pub fn new() -> Self {
        Self::default()
    }
}

fn eur(amount_cents: i64) -> Money {
    Money::new(amount_cents, Currency::Eur)
}

/// The demo catalog. `Aurora Desk Lamp` is priced at `…99` cents on purpose:
/// `FakePaymentProvider` declines those totals, which is how the compensation
/// branch of the fulfillment saga gets exercised.
fn demo_catalog() -> Vec<SupplierProduct> {
    vec![
        SupplierProduct {
            supplier_product_ref: "MP-1001".to_owned(),
            title: "Aurora Desk Lamp".to_owned(),
            description: "Warm dimmable LED lamp with a brushed aluminium arm.".to_owned(),
            price: eur(3499),
            image_url: "https://placehold.co/400x400?text=Aurora+Desk+Lamp".to_owned(),
        },
        SupplierProduct {
            supplier_product_ref: "MP-1002".to_owned(),
            title: "Basalt Pour-Over Kettle".to_owned(),
            description: "Gooseneck spout, 1L matte-black stainless steel body.".to_owned(),
            price: eur(4200),
            image_url: "https://placehold.co/400x400?text=Basalt+Kettle".to_owned(),
        },
        SupplierProduct {
            supplier_product_ref: "MP-1003".to_owned(),
            title: "Kestrel Packable Backpack".to_owned(),
            description: "18L ripstop daypack that folds into its own pocket.".to_owned(),
            price: eur(2750),
            image_url: "https://placehold.co/400x400?text=Kestrel+Backpack".to_owned(),
        },
        SupplierProduct {
            supplier_product_ref: "MP-1004".to_owned(),
            title: "Meridian Linen Throw".to_owned(),
            description: "Stonewashed linen blanket, 130x170cm, oatmeal.".to_owned(),
            price: eur(5900),
            image_url: "https://placehold.co/400x400?text=Meridian+Throw".to_owned(),
        },
        SupplierProduct {
            supplier_product_ref: "MP-1005".to_owned(),
            title: "Tidal Bluetooth Speaker".to_owned(),
            description: "Pocket speaker with 12h battery and IPX7 housing.".to_owned(),
            price: eur(6400),
            image_url: "https://placehold.co/400x400?text=Tidal+Speaker".to_owned(),
        },
        SupplierProduct {
            supplier_product_ref: "MP-1006".to_owned(),
            title: "Foundry Cast Iron Pan".to_owned(),
            description: "Pre-seasoned 26cm skillet with a helper handle.".to_owned(),
            price: eur(3800),
            image_url: "https://placehold.co/400x400?text=Foundry+Pan".to_owned(),
        },
    ]
}

#[async_trait::async_trait]
impl Supplier for MockSupplier {
    fn id(&self) -> &'static str {
        "mock"
    }

    async fn search_products(&self, query: &str) -> Result<Vec<SupplierProduct>, SupplierError> {
        let needle = query.trim().to_lowercase();
        if needle.is_empty() {
            return Ok(demo_catalog());
        }

        Ok(demo_catalog()
            .into_iter()
            .filter(|product| {
                product.title.to_lowercase().contains(&needle)
                    || product.description.to_lowercase().contains(&needle)
            })
            .collect())
    }

    async fn place_order(
        &self,
        req: &SupplierOrderRequest,
    ) -> Result<SupplierConfirmation, SupplierError> {
        let external_ref = format!("MOCK-{}", new_id());
        tracing::info!(
            order_id = %req.order_id,
            lines = req.lines.len(),
            %external_ref,
            "mock supplier accepted order"
        );
        Ok(SupplierConfirmation { external_ref })
    }

    async fn track_shipment(&self, external_ref: &str) -> Result<TrackingStatus, SupplierError> {
        let polls = {
            let mut tracking_polls = self.tracking_polls.lock().map_err(|source| {
                SupplierError::Api(format!("tracking state poisoned: {source}"))
            })?;
            let polls = tracking_polls.entry(external_ref.to_owned()).or_insert(0);
            *polls = polls.saturating_add(1);
            *polls
        };

        // Shipments are created holding `Pending`, so the first refresh
        // dispatches and the second delivers.
        Ok(match polls {
            1 => TrackingStatus::Dispatched {
                tracking_number: format!("TRK-{external_ref}"),
                carrier: "MockExpress".to_owned(),
            },
            _ => TrackingStatus::Delivered,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::supplier::SupplierLine;

    #[tokio::test]
    async fn search_returns_the_whole_catalog_for_an_empty_query() {
        let supplier = MockSupplier::new();
        assert_eq!(supplier.search_products("  ").await.unwrap().len(), 6);
    }

    #[tokio::test]
    async fn search_filters_case_insensitively() {
        let supplier = MockSupplier::new();
        let hits = supplier.search_products("KETTLE").await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].supplier_product_ref, "MP-1002");
        assert!(
            supplier
                .search_products("nothing here")
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn catalog_contains_a_price_that_triggers_the_payment_decline_demo() {
        let supplier = MockSupplier::new();
        let declines = supplier
            .search_products("")
            .await
            .unwrap()
            .into_iter()
            .filter(|product| product.price.amount_cents % 100 == 99)
            .count();
        assert!(declines > 0, "need at least one …99 price for the demo");
    }

    #[tokio::test]
    async fn place_order_confirms_with_a_prefixed_reference() {
        let supplier = MockSupplier::new();
        let confirmation = supplier
            .place_order(&SupplierOrderRequest {
                order_id: "order-1".to_owned(),
                lines: vec![SupplierLine {
                    supplier_product_ref: "MP-1001".to_owned(),
                    title: "Aurora Desk Lamp".to_owned(),
                    quantity: 1,
                }],
            })
            .await
            .unwrap();
        assert!(confirmation.external_ref.starts_with("MOCK-"));
    }

    #[tokio::test]
    async fn tracking_advances_from_dispatched_to_delivered_per_reference() {
        let supplier = MockSupplier::new();

        assert_eq!(
            supplier.track_shipment("MOCK-A").await.unwrap(),
            TrackingStatus::Dispatched {
                tracking_number: "TRK-MOCK-A".to_owned(),
                carrier: "MockExpress".to_owned(),
            }
        );
        assert_eq!(
            supplier.track_shipment("MOCK-A").await.unwrap(),
            TrackingStatus::Delivered
        );
        assert_eq!(
            supplier.track_shipment("MOCK-A").await.unwrap(),
            TrackingStatus::Delivered
        );

        // Counters are per external_ref, not global.
        assert!(matches!(
            supplier.track_shipment("MOCK-B").await.unwrap(),
            TrackingStatus::Dispatched { .. }
        ));
    }
}
