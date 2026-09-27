//! The AliExpress adapter against a local stand-in for their gateway: what it
//! signs and sends, and what it makes of the answers — including the ones that
//! arrive as HTTP 200 with an error code inside, which is their way.
//!
//! This is the whole of the adapter's testing. It has **never** been pointed
//! at the real API; the first round trip against a real app key belongs to
//! whoever has one.
#![cfg(feature = "aliexpress")]

use std::{
    collections::{BTreeMap, HashMap},
    sync::{Arc, Mutex},
};

use axum::{Form, Json, Router, extract::State, http::StatusCode, routing::post};
use serde_json::{Value, json};
use timada_core::{Address, Money};
use timada_sourcing::{
    ConnectorError, ConnectorTask, PlaceOrder, PurchaseLine, SupplierConnector, SupplierItemRef,
    SupplierOrderStanding,
    aliexpress::{AliExpressConfig, AliExpressConnector, sign},
};

/// What the stand-in remembers, and how it is told to answer.
#[derive(Default)]
struct Gateway {
    /// Every call it received, as its form parameters.
    calls: Vec<HashMap<String, String>>,
    /// `(HTTP status, body)` queued for the next call, in order.
    answers: Vec<(u16, Value)>,
}

type Shared = Arc<Mutex<Gateway>>;

fn lock(gateway: &Shared) -> std::sync::MutexGuard<'_, Gateway> {
    gateway.lock().unwrap_or_else(|e| e.into_inner())
}

async fn handle(
    State(gateway): State<Shared>,
    Form(params): Form<HashMap<String, String>>,
) -> (StatusCode, Json<Value>) {
    let mut gateway = lock(&gateway);
    gateway.calls.push(params);
    if gateway.answers.is_empty() {
        return (StatusCode::OK, Json(json!({ "code": "0" })));
    }
    let (status, body) = gateway.answers.remove(0);
    (
        StatusCode::from_u16(status).unwrap_or(StatusCode::OK),
        Json(body),
    )
}

/// One `offers` call, for the tests that only care what came back.
async fn ask(
    connector: &AliExpressConnector,
) -> Result<Vec<timada_sourcing::SupplierOffer>, ConnectorError> {
    connector
        .offers(&[SupplierItemRef::new("1005006100001", None)])
        .await
}

/// Starts the stand-in on a free local port.
async fn gateway() -> anyhow::Result<(AliExpressConnector, Shared)> {
    let gateway = Shared::default();
    let app = Router::new()
        .route("/sync", post(handle))
        .with_state(gateway.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    tokio::spawn(async move {
        if let Err(err) = axum::serve(listener, app).await {
            tracing::error!(%err, "stand-in stopped");
        }
    });
    let config = AliExpressConfig::new("app-key-1", "app-secret-1", "token-1")
        .with_api_base(format!("http://{address}/sync"));
    Ok((AliExpressConnector::new(config)?, gateway))
}

fn address() -> Address {
    Address {
        first_name: "Jonathan".into(),
        last_name: "Lapiquonne".into(),
        line1: "1 rue de l'Entrepôt".into(),
        postal_code: "31000".into(),
        city: "Toulouse".into(),
        country_code: "FR".into(),
        mobile: Some("+33600000000".into()),
        ..Address::default()
    }
}

#[tokio::test]
async fn it_signs_what_it_sends_and_says_who_it_is() -> anyhow::Result<()> {
    let (connector, gateway) = gateway().await?;
    lock(&gateway).answers.push((
        200,
        json!({
            "code": "0",
            "items": [{
                "product_id": "1005006100001",
                "sku_price": "34.50",
                "shipping_price": "2.00",
                "sku_stock": 12,
                "product_title": "A monitor",
                "product_detail_url": "https://example.test/i/1"
            }]
        }),
    ));

    let offers = connector
        .offers(&[SupplierItemRef::new("1005006100001", None)])
        .await?;
    assert_eq!(offers.len(), 1);
    assert_eq!(offers[0].cost, Money::new(3_450, "USD"));
    assert_eq!(offers[0].shipping, Money::new(200, "USD"));
    assert_eq!(offers[0].available, 12);
    assert_eq!(offers[0].title.as_deref(), Some("A monitor"));

    let calls = lock(&gateway).calls.clone();
    assert_eq!(calls.len(), 1);
    let sent = &calls[0];
    assert_eq!(sent.get("app_key").map(String::as_str), Some("app-key-1"));
    assert_eq!(sent.get("session").map(String::as_str), Some("token-1"));
    assert_eq!(
        sent.get("method").map(String::as_str),
        Some("aliexpress.ds.product.get")
    );
    assert_eq!(
        sent.get("sign_method").map(String::as_str),
        Some("hmac-sha256")
    );
    // The secret never travels.
    assert!(!sent.values().any(|value| value.contains("app-secret-1")));

    // And the signature is over exactly the parameters that were sent.
    let signature = sent
        .get("sign")
        .ok_or_else(|| anyhow::anyhow!("unsigned call"))?;
    let signed: BTreeMap<String, String> = sent
        .iter()
        .filter(|(name, _)| name.as_str() != "sign")
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    assert_eq!(&sign("app-secret-1", &signed), signature);

    Ok(())
}

#[tokio::test]
async fn an_item_it_does_not_answer_about_is_left_out() -> anyhow::Result<()> {
    let (connector, gateway) = gateway().await?;
    lock(&gateway).answers.push((
        200,
        json!({
            "code": "0",
            "items": [{ "product_id": "1005006100001", "sku_price": "10.00", "sku_stock": 3 }]
        }),
    ));

    // Two asked about, one answered: the sync worker reads the absence as
    // "no longer listed" and says so to an operator.
    let offers = connector
        .offers(&[
            SupplierItemRef::new("1005006100001", None),
            SupplierItemRef::new("1005006199999", None),
        ])
        .await?;
    assert_eq!(offers.len(), 1);
    assert_eq!(offers[0].item.external_item_id, "1005006100001");

    // A SKU the answer does not carry is not the SKU that was asked for.
    lock(&gateway).answers.push((
        200,
        json!({
            "code": "0",
            "items": [{ "product_id": "1005006100001", "sku_price": "10.00", "sku_stock": 3 }]
        }),
    ));
    assert!(
        connector
            .offers(&[SupplierItemRef::new(
                "1005006100001",
                Some("black".to_owned())
            )])
            .await?
            .is_empty()
    );

    Ok(())
}

#[tokio::test]
async fn an_order_carries_the_shops_own_reference_as_its_key() -> anyhow::Result<()> {
    let (connector, gateway) = gateway().await?;
    lock(&gateway).answers.push((
        200,
        json!({ "code": "0", "order_id": "3000012345678", "total_amount": "36.50" }),
    ));

    let lines = vec![PurchaseLine {
        item: SupplierItemRef::new("1005006100001", Some("14:350853#black".to_owned())),
        quantity: 2,
        unit_cost: Money::new(3_450, "USD"),
    }];
    let placed = connector
        .place(&PlaceOrder {
            reference: "purchase-abc",
            lines: &lines,
            ship_to: &address(),
            note: None,
        })
        .await?;
    assert_eq!(placed.external_order_id, "3000012345678");
    assert_eq!(placed.cost, Money::new(3_650, "USD"));

    let calls = lock(&gateway).calls.clone();
    let request = calls[0]
        .get("param_place_order_request4_open_api_d_t_o")
        .ok_or_else(|| anyhow::anyhow!("no order payload"))?;
    let request: Value = serde_json::from_str(request)?;
    // Their idempotency key is the shop's purchase id, so a retry after a
    // lost answer is the same order rather than a second one.
    assert_eq!(request["out_order_id"], "purchase-abc");
    assert_eq!(request["product_items"][0]["product_count"], 2);
    assert_eq!(request["product_items"][0]["sku_attr"], "14:350853#black");
    assert_eq!(request["logistics_address"]["zip"], "31000");
    assert_eq!(request["logistics_address"]["country"], "FR");
    assert_eq!(
        request["logistics_address"]["contact_person"],
        "Jonathan Lapiquonne"
    );

    Ok(())
}

#[tokio::test]
async fn shipped_means_a_tracking_number_not_a_status() -> anyhow::Result<()> {
    let (connector, gateway) = gateway().await?;
    // Their status says the seller sent it, but there is nothing to tell the
    // customer yet: still on its way, as far as the shop is concerned.
    lock(&gateway).answers.push((
        200,
        json!({ "code": "0", "order_status": "SELLER_SEND_GOODS" }),
    ));
    assert_eq!(
        connector.standing("3000012345678").await?,
        SupplierOrderStanding::Pending
    );

    lock(&gateway).answers.push((
        200,
        json!({
            "code": "0",
            "order_status": "SELLER_SEND_GOODS",
            "logistics_service_name": "4PX",
            "logistics_no": "4PX-778899"
        }),
    ));
    assert_eq!(
        connector.standing("3000012345678").await?,
        SupplierOrderStanding::Shipped {
            carrier: "4PX".into(),
            tracking_number: "4PX-778899".into(),
        }
    );

    lock(&gateway)
        .answers
        .push((200, json!({ "code": "0", "order_status": "ORDER_CANCEL" })));
    assert!(matches!(
        connector.standing("3000012345678").await?,
        SupplierOrderStanding::Cancelled { .. }
    ));

    Ok(())
}

#[tokio::test]
async fn a_failure_in_a_two_hundred_is_still_a_failure() -> anyhow::Result<()> {
    let (connector, gateway) = gateway().await?;

    // This is the trap their platform sets: HTTP 200, and the fault in the
    // body. A status check alone would take it for an empty catalogue.
    lock(&gateway).answers.push((
        200,
        json!({ "code": "APP_CALL_LIMITED", "msg": "too many requests" }),
    ));
    assert!(matches!(
        ask(&connector).await,
        Err(ConnectorError::RateLimited { .. })
    ));

    lock(&gateway).answers.push((
        200,
        json!({ "code": "ISP.TOP-REMOTE-SERVICE-UNAVAILABLE", "msg": "later" }),
    ));
    assert!(matches!(
        ask(&connector).await,
        Err(ConnectorError::Unavailable(_))
    ));

    lock(&gateway)
        .answers
        .push((200, json!({ "code": "PRODUCT_NOT_EXIST", "msg": "gone" })));
    assert!(matches!(
        ask(&connector).await,
        Err(ConnectorError::UnknownItem(_))
    ));

    lock(&gateway).answers.push((
        200,
        json!({ "sub_code": "ADDRESS_NOT_SUPPORTED", "msg": "no" }),
    ));
    assert!(matches!(
        ask(&connector).await,
        Err(ConnectorError::Refused(_))
    ));

    // And the ordinary HTTP failures on top of that.
    lock(&gateway).answers.push((429, json!({})));
    assert!(matches!(
        ask(&connector).await,
        Err(ConnectorError::RateLimited { .. })
    ));
    lock(&gateway).answers.push((503, json!({})));
    assert!(matches!(
        ask(&connector).await,
        Err(ConnectorError::Unavailable(_))
    ));
    lock(&gateway).answers.push((400, json!({})));
    assert!(matches!(
        ask(&connector).await,
        Err(ConnectorError::Refused(_))
    ));

    Ok(())
}

#[tokio::test]
async fn calling_an_order_off_is_done_on_the_platform() -> anyhow::Result<()> {
    let (connector, gateway) = gateway().await?;
    // It says so plainly instead of pretending, and asks nothing of them.
    assert!(matches!(
        connector.cancel("3000012345678").await,
        Err(ConnectorError::Refused(_))
    ));
    assert!(lock(&gateway).calls.is_empty());

    // It still quotes, orders and tracks: cancelling is not a task of its own.
    for task in [
        ConnectorTask::Offers,
        ConnectorTask::Placing,
        ConnectorTask::Tracking,
    ] {
        assert!(connector.does(task), "{task:?} should be offered");
    }
    assert_eq!(connector.key(), "aliexpress");

    Ok(())
}
