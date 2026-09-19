use timada_core::{Address, Money};
use timada_shipping::{
    Command, CreateShipment, DeliveryKind, DeliveryMethod, ShipmentLine, ShipmentStatus,
    ShippingError, delivery_offers, load_shipment, shipment_id, shipping_fee,
};

fn dom_address() -> Address {
    Address {
        first_name: "Jonathan".into(),
        last_name: "Lapiquonne".into(),
        line1: "La agnès".into(),
        postal_code: "97290".into(),
        city: "Le Marin".into(),
        country_code: "MQ".into(),
        ..Address::default()
    }
}

fn chronopost() -> DeliveryMethod {
    DeliveryMethod {
        code: "chronopost-dom".into(),
        carrier: "Chronopost".into(),
        kind: DeliveryKind::HomeDelivery,
    }
}

#[test]
fn resolves_delivery_methods() {
    assert_eq!(
        DeliveryMethod::resolve("chronopost-dom", None),
        Some(chronopost())
    );
    assert_eq!(DeliveryMethod::resolve("store-pickup", None), None);
    assert_eq!(
        DeliveryMethod::resolve("store-pickup", Some("toulouse".into())).map(|m| m.kind),
        Some(DeliveryKind::StorePickup {
            store_id: "toulouse".into()
        })
    );
    assert_eq!(DeliveryMethod::resolve("pigeon", None), None);
    assert_eq!(shipping_fee("chronopost-dom"), Some(Money::eur(2_395)));
    assert_eq!(shipping_fee("store-pickup"), Some(Money::eur(0)));
    assert_eq!(shipping_fee("pigeon"), None);
}

#[test]
fn every_offer_resolves_and_has_its_fee() {
    let offers = delivery_offers();
    assert!(!offers.is_empty());
    for offer in offers {
        let store = offer.requires_pickup_store.then(|| "toulouse".to_owned());
        assert!(DeliveryMethod::resolve(offer.code, store).is_some());
        assert_eq!(shipping_fee(offer.code), Some(offer.fee));
    }
}

#[tokio::test]
async fn shipment_lifecycle() -> anyhow::Result<()> {
    let (executor, _db) = timada_core::testing::memory_executor(Vec::new()).await?;
    let cmd = Command(&executor);

    let create = CreateShipment {
        order_id: "order-1".into(),
        method: chronopost(),
        destination: dom_address(),
        lines: vec![ShipmentLine {
            product_id: "aoc-24g4xe".into(),
            quantity: 1,
        }],
    };
    let id = cmd.create_shipment(create.clone()).await?;
    assert_eq!(id, shipment_id("order-1"));
    assert_eq!(cmd.create_shipment(create).await?, id);

    cmd.dispatch_shipment(&id, "Chronopost".into(), "XY123".into())
        .await?;
    let view = load_shipment(&executor, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("shipment missing"))?;
    assert_eq!(view.status, ShipmentStatus::Dispatched);
    assert_eq!(view.order_id, "order-1");
    assert_eq!(view.destination.country_code, "MQ");
    assert_eq!(view.lines.len(), 1);
    assert_eq!(view.carrier.as_deref(), Some("Chronopost"));
    assert_eq!(view.tracking_number.as_deref(), Some("XY123"));

    cmd.mark_delivered(&id).await?;
    let view = load_shipment(&executor, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("shipment missing"))?;
    assert_eq!(view.status, ShipmentStatus::Delivered);

    let again = cmd
        .dispatch_shipment(&id, "Chronopost".into(), "XY124".into())
        .await;
    assert!(matches!(again, Err(ShippingError::NotCreated)));

    // The carrier has the parcel: too late to cancel.
    let too_late = cmd.cancel_shipment(&id, "order cancelled").await;
    assert!(matches!(too_late, Err(ShippingError::NotCreated)));

    // A shipment still waiting can be cancelled, once, and never leaves.
    let waiting = cmd
        .create_shipment(CreateShipment {
            order_id: "order-2".into(),
            method: chronopost(),
            destination: dom_address(),
            lines: vec![ShipmentLine {
                product_id: "aoc-24g4xe".into(),
                quantity: 1,
            }],
        })
        .await?;
    cmd.cancel_shipment(&waiting, "order cancelled").await?;
    cmd.cancel_shipment(&waiting, "order cancelled").await?;
    let view = load_shipment(&executor, &waiting)
        .await?
        .ok_or_else(|| anyhow::anyhow!("shipment missing"))?;
    assert_eq!(view.status, ShipmentStatus::Cancelled);
    assert_eq!(view.cancelled_reason.as_deref(), Some("order cancelled"));
    let dispatch = cmd
        .dispatch_shipment(&waiting, "Chronopost".into(), "XY125".into())
        .await;
    assert!(matches!(dispatch, Err(ShippingError::NotCreated)));

    Ok(())
}

#[tokio::test]
async fn rejects_invalid_shipments() -> anyhow::Result<()> {
    let (executor, _db) = timada_core::testing::memory_executor(Vec::new()).await?;
    let cmd = Command(&executor);

    let no_lines = cmd
        .create_shipment(CreateShipment {
            order_id: "order-2".into(),
            method: chronopost(),
            destination: dom_address(),
            lines: Vec::new(),
        })
        .await;
    assert!(matches!(no_lines, Err(ShippingError::NoLines)));

    let bad_address = cmd
        .create_shipment(CreateShipment {
            order_id: "order-2".into(),
            method: chronopost(),
            destination: Address {
                city: String::new(),
                ..dom_address()
            },
            lines: vec![ShipmentLine {
                product_id: "aoc-24g4xe".into(),
                quantity: 1,
            }],
        })
        .await;
    assert!(matches!(bad_address, Err(ShippingError::Address(_))));

    let missing = cmd.mark_delivered(shipment_id("order-2")).await;
    assert!(matches!(missing, Err(ShippingError::ShipmentNotFound)));

    Ok(())
}
