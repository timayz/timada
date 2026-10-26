use evento::migrator::Migrate;
use serde_json::json;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx_migrator::Info;
use timada_aliexpress::aggregator::SkuLink;
use timada_aliexpress::aggregator::forward::{Checked, Done, ForwardItem};
use timada_aliexpress::{
    AeOrder, AeProduct, AliexpressError, ForwardStatus, FreightOption, LinkState, LogisticsAddress,
    PlaceItem, PlaceOrder, Placement, PlannedItem, choose_service, forward_items,
    freight_from_response, logistics_address, placed_orders_from_response, split_phone,
};
use timada_orders::{OrderLine, ShippingAddress};

const ALICE: &str = "user-alice";
/// The tenant the tests act in.
const TENANT: &str = "acme";

async fn executor() -> evento::Sqlite {
    let options = SqliteConnectOptions::new()
        .filename(":memory:")
        .create_if_missing(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let mut migrator = evento::sql_migrator::new::<sqlx::Sqlite>().unwrap();
    migrator
        .add_migrations(timada_aliexpress::migrations())
        .unwrap();
    migrator
        .run(&mut *conn, &evento::migrator::Plan::apply_all())
        .await
        .unwrap();
    drop(conn);
    pool.into()
}

#[test]
fn a_combination_says_how_an_order_names_it() {
    let body: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/product.json")).unwrap();
    let product = AeProduct::from_response("1005006123456789", &body).unwrap();
    assert_eq!(
        product.sku("12000037001").unwrap().attr.as_deref(),
        Some("14:29#Rose;5:100014064")
    );
    assert_eq!(product.sku("12000037002").unwrap().attr, None);
}

#[test]
fn delivery_options_read_wrapped_or_refused() {
    let body = json!({
        "aliexpress_logistics_buyer_freight_calculate_response": {
            "result": {
                "success": true,
                "aeop_freight_calculate_result_for_buyer_d_t_o_list": {
                    "aeop_freight_calculate_result_for_buyer_dto": [
                        { "service_name": "CAINIAO_STANDARD", "freight": { "cent": 0, "amount": 0, "currency_code": "EUR" }, "tracking_available": "true" },
                        { "service_name": "YANWEN_JYT", "freight": { "amount": "1.20", "currency_code": "EUR" }, "tracking_available": "false" }
                    ]
                }
            }
        }
    });
    assert_eq!(
        freight_from_response(&body).unwrap(),
        vec![
            FreightOption {
                service_name: "CAINIAO_STANDARD".into(),
                cents: 0,
                tracking: true,
            },
            FreightOption {
                service_name: "YANWEN_JYT".into(),
                cents: 120,
                tracking: false,
            },
        ]
    );
    let refused = json!({
        "aliexpress_logistics_buyer_freight_calculate_response": {
            "result": { "success": false, "error_desc": "no delivery to this country" }
        }
    });
    let err = freight_from_response(&refused).unwrap_err();
    assert_eq!(err.message, "no delivery to this country");
}

#[test]
fn a_placed_order_gives_its_numbers_or_why_not() {
    let wrapped = json!({
        "aliexpress_trade_buy_placeorder_response": {
            "result": { "is_success": true, "order_list": { "number": [8123456789012345u64, 8123456789012346u64] } }
        }
    });
    assert_eq!(
        placed_orders_from_response(&wrapped).unwrap(),
        vec!["8123456789012345", "8123456789012346"]
    );
    let bare = json!({
        "aliexpress_trade_buy_placeorder_response": {
            "result": { "is_success": "true", "order_list": [42] }
        }
    });
    assert_eq!(placed_orders_from_response(&bare).unwrap(), vec!["42"]);

    let refused = json!({
        "aliexpress_trade_buy_placeorder_response": {
            "result": { "is_success": false, "error_code": "B_DROPSHIPPER_DELIVERY_ADDRESS_VALIDATE_FAIL", "error_msg": "address" }
        }
    });
    let err = placed_orders_from_response(&refused).unwrap_err();
    assert_eq!(err.code, "B_DROPSHIPPER_DELIVERY_ADDRESS_VALIDATE_FAIL");
    assert!(!err.may_have_placed());

    // Taken, with no number: it may be there.
    let silent = json!({
        "aliexpress_trade_buy_placeorder_response": { "result": { "is_success": true } }
    });
    assert!(
        placed_orders_from_response(&silent)
            .unwrap_err()
            .may_have_placed()
    );
}

#[test]
fn an_order_reads_its_status_and_parcels() {
    let shipped = json!({
        "aliexpress_trade_ds_order_get_response": {
            "rsp_code": "200",
            "result": {
                "order_status": "WAIT_BUYER_ACCEPT_GOODS",
                "logistics_status": "SELLER_SEND_GOODS",
                "logistics_info_list": {
                    "ae_order_logistics_info": [
                        { "logistics_no": "LP00123", "logistics_service": "CAINIAO_STANDARD" },
                        { "logistics_no": "LP00123", "logistics_service": "CAINIAO_STANDARD" }
                    ]
                }
            }
        }
    });
    assert_eq!(
        AeOrder::from_response(&shipped).unwrap(),
        AeOrder {
            status: "WAIT_BUYER_ACCEPT_GOODS".into(),
            logistics_status: "SELLER_SEND_GOODS".into(),
            tracking: vec!["LP00123".into()],
        }
    );
    // One parcel may come as the object itself, under the other name.
    let one = json!({
        "aliexpress_ds_trade_order_get_response": {
            "result": {
                "order_status": "WAIT_BUYER_ACCEPT_GOODS",
                "logistics_info_list": { "ae_order_logistics_info": { "logistics_no": "LP9", "logistics_service": "X" } }
            }
        }
    });
    assert_eq!(AeOrder::from_response(&one).unwrap().tracking, vec!["LP9"]);
    let waiting = json!({
        "aliexpress_trade_ds_order_get_response": {
            "result": { "order_status": "PLACE_ORDER_SUCCESS", "logistics_status": "NO_LOGISTICS" }
        }
    });
    assert!(
        AeOrder::from_response(&waiting)
            .unwrap()
            .tracking
            .is_empty()
    );
}

#[test]
fn only_free_delivery_by_aliexpress_itself_is_chosen() {
    let option = |name: &str, cents, tracking| FreightOption {
        service_name: name.into(),
        cents,
        tracking,
    };
    let options = vec![
        option("YANWEN_JYT", 0, true),
        option("CAINIAO_ECONOMY", 0, false),
        option("CAINIAO_STANDARD", 0, true),
        option("CAINIAO_PREMIUM", 900, true),
    ];
    assert_eq!(
        choose_service(&options).unwrap().service_name,
        "CAINIAO_STANDARD"
    );
    assert_eq!(
        choose_service(&[option("CAINIAO_ECONOMY", 0, false)])
            .unwrap()
            .service_name,
        "CAINIAO_ECONOMY"
    );
    assert!(
        choose_service(&[
            option("YANWEN_JYT", 0, true),
            option("CAINIAO_PREMIUM", 900, true)
        ])
        .is_none()
    );
}

#[test]
fn an_address_and_its_phone_read_as_aliexpress_wants() {
    assert_eq!(
        split_phone("06 12 34 56 78", "FR"),
        ("+33".into(), "612345678".into())
    );
    assert_eq!(
        split_phone("+33 6 12 34 56 78", "FR"),
        ("+33".into(), "612345678".into())
    );
    assert_eq!(
        split_phone("0033612345678", "FR"),
        ("+33".into(), "612345678".into())
    );
    assert_eq!(
        split_phone("+32 470 12 34 56", "FR"),
        ("+32".into(), "470123456".into())
    );
    assert_eq!(split_phone("06 12", "IT"), ("+39".into(), "0612".into()));
    assert_eq!(split_phone("", "FR"), (String::new(), String::new()));

    let address = ShippingAddress {
        name: "Ada Lovelace".into(),
        line1: "12 rue de la Paix".into(),
        line2: Some("Bât. B".into()),
        postal_code: "75002".into(),
        city: "Paris".into(),
        country: "FR".into(),
        phone: Some("06 12 34 56 78".into()),
    };
    assert_eq!(
        logistics_address(&address, Some("fr")),
        LogisticsAddress {
            full_name: "Ada Lovelace".into(),
            contact_person: "Ada Lovelace".into(),
            address: "12 rue de la Paix".into(),
            address2: "Bât. B".into(),
            city: "Paris".into(),
            zip: "75002".into(),
            country: "FR".into(),
            phone_country: "+33".into(),
            mobile_no: "612345678".into(),
            locale: "fr_FR".into(),
        }
    );
}

#[test]
fn an_order_is_sent_with_its_items_and_how_it_is_paid() {
    let order = PlaceOrder {
        address: LogisticsAddress {
            full_name: "Ada".into(),
            address: "1 rue".into(),
            country: "FR".into(),
            ..Default::default()
        },
        items: vec![PlaceItem {
            product_id: "1005006123456789".into(),
            sku_attr: "14:29#Rose".into(),
            product_count: 2,
            logistics_service_name: "CAINIAO_STANDARD".into(),
            order_memo: "K7Q4-MZP2".into(),
        }],
        try_to_pay: true,
        pay_currency: "EUR".into(),
    };
    let (request, extend) = order.params();
    let request: serde_json::Value = serde_json::from_str(&request).unwrap();
    let extend: serde_json::Value = serde_json::from_str(&extend).unwrap();
    assert_eq!(
        request["product_items"][0]["product_id"],
        json!(1005006123456789u64)
    );
    assert_eq!(request["product_items"][0]["sku_attr"], "14:29#Rose");
    assert_eq!(request["logistics_address"]["country"], "FR");
    assert!(request["logistics_address"].get("address2").is_none());
    assert_eq!(extend["payment"]["try_to_pay"], "true");
    assert_eq!(extend["payment"]["pay_currency"], "EUR");
}

/// A linked item: two variants of `product-1`, and `product-2` that does
/// not vary.
async fn links(executor: &evento::Sqlite) -> Vec<LinkState> {
    let gateway = timada_aliexpress::Command::new(executor.clone(), TENANT);
    let sku = |sku_id: &str, variant_id: Option<&str>| SkuLink {
        sku_id: sku_id.into(),
        variant_id: variant_id.map(Into::into),
        cost_cents: 100,
        available: 10,
    };
    let mut all = Vec::new();
    for (ae_id, product_id, skus) in [
        (
            "1005000000000001",
            "product-1",
            vec![
                sku("A1", Some("variant-red")),
                sku("A2", Some("variant-blue")),
            ],
        ),
        ("1005000000000002", "product-2", vec![sku("B1", None)]),
    ] {
        let link_id = gateway.request_import(ALICE, ae_id).await.unwrap();
        gateway
            .record_import(ALICE, &link_id, product_id, skus)
            .await
            .unwrap();
        all.push(gateway.load_link_by_id(&link_id).await.unwrap().unwrap());
    }
    all
}

fn line(product_id: &str, variant_id: Option<&str>, quantity: u32) -> OrderLine {
    OrderLine {
        product_id: product_id.into(),
        variant_id: variant_id.map(Into::into),
        name: product_id.into(),
        quantity,
        unit_amount_cents: 1000,
        ..Default::default()
    }
}

#[tokio::test]
async fn an_order_buys_the_combinations_its_lines_were_imported_from() {
    let executor = executor().await;
    let links = links(&executor).await;

    // An offer splits a unit into its own line: they buy one combination.
    let plan = forward_items(
        &[
            line("product-1", Some("variant-red"), 1),
            line("product-1", Some("variant-red"), 2),
            line("product-2", None, 1),
        ],
        &links,
    );
    assert!(plan.complete);
    assert_eq!(
        plan.items,
        vec![
            PlannedItem {
                ae_product_id: "1005000000000001".into(),
                sku_id: "A1".into(),
                quantity: 3,
            },
            PlannedItem {
                ae_product_id: "1005000000000002".into(),
                sku_id: "B1".into(),
                quantity: 1,
            },
        ]
    );

    // A product of the shop's own, or a variant added by hand, is not one.
    let mixed = forward_items(
        &[
            line("product-1", Some("variant-blue"), 1),
            line("product-1", Some("variant-hand-made"), 1),
            line("product-own", None, 1),
        ],
        &links,
    );
    assert!(!mixed.complete);
    assert_eq!(mixed.items.len(), 1);
    assert_eq!(mixed.items[0].sku_id, "A2");

    let none = forward_items(&[line("product-own", None, 1)], &links);
    assert!(none.items.is_empty());
    assert!(!none.complete);
}

#[tokio::test]
async fn an_order_is_placed_once_and_read_until_it_ships() {
    let executor = executor().await;
    let forwards = timada_aliexpress::Command::new(executor.clone(), TENANT);

    let id = forwards
        .request_forward(ALICE, "order-1", false)
        .await
        .unwrap();
    assert!(matches!(
        forwards.request_forward(ALICE, "order-1", false).await,
        Err(AliexpressError::Forwarding)
    ));
    assert!(forwards.start_forward(ALICE, &id).await.unwrap());
    assert!(!forwards.start_forward(ALICE, &id).await.unwrap());

    // Taken on, never heard of again: only asked for again on purpose.
    assert!(matches!(
        forwards.request_forward(ALICE, "order-1", false).await,
        Err(AliexpressError::Forwarding)
    ));
    forwards
        .request_forward(ALICE, "order-1", true)
        .await
        .unwrap();
    assert!(forwards.start_forward(ALICE, &id).await.unwrap());
    forwards
        .record_forward_failure(ALICE, &id, "AliExpress refused: address")
        .await
        .unwrap();
    let failed = forwards.load_forward("order-1").await.unwrap().unwrap();
    assert_eq!(failed.status, ForwardStatus::Failed);
    assert_eq!(
        failed.last_error.as_deref(),
        Some("AliExpress refused: address")
    );

    // A failure is asked for again like any request.
    forwards
        .request_forward(ALICE, "order-1", false)
        .await
        .unwrap();
    assert!(forwards.start_forward(ALICE, &id).await.unwrap());
    let item = ForwardItem {
        ae_product_id: "1005000000000001".into(),
        sku_id: "A1".into(),
        quantity: 1,
        service_name: "CAINIAO_STANDARD".into(),
    };
    forwards
        .record_placed(
            ALICE,
            &id,
            Placement {
                ae_order_ids: vec!["81".into(), "82".into()],
                items: vec![item.clone()],
                pay_requested: true,
                complete: true,
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        forwards.request_forward(ALICE, "order-1", true).await,
        Err(AliexpressError::AlreadyForwarded)
    ));
    let placed = forwards.load_forward("order-1").await.unwrap().unwrap();
    assert_eq!(placed.status, ForwardStatus::Placed);
    assert_eq!(placed.items, vec![item]);
    assert!(placed.last_error.is_none());
    assert!(!placed.all_shipped());

    let (_, version) = forwards.load_forward_versioned(&id).await.unwrap().unwrap();
    let checked = |id: &str, tracking: &str| Checked {
        ae_order_id: id.into(),
        status: "WAIT_BUYER_ACCEPT_GOODS".into(),
        logistics_status: "SELLER_SEND_GOODS".into(),
        tracking: vec![tracking.into()],
    };
    forwards
        .record_checked(&id, version, vec![checked("81", "LP1")], None)
        .await
        .unwrap();
    // A read from the version before loses to the one recorded.
    assert!(matches!(
        forwards
            .record_checked(&id, version, vec![checked("82", "LP2")], None)
            .await,
        Err(AliexpressError::Write(
            evento::WriteError::InvalidOriginalVersion
        ))
    ));
    let (_, version) = forwards.load_forward_versioned(&id).await.unwrap().unwrap();
    forwards
        .record_checked(
            &id,
            version,
            vec![checked("82", "LP2")],
            Some(Done {
                tracking: Some("LP1, LP2".into()),
                fulfilled: true,
            }),
        )
        .await
        .unwrap();
    let done = forwards.load_forward("order-1").await.unwrap().unwrap();
    assert_eq!(done.status, ForwardStatus::Done);
    assert!(done.all_shipped());
    assert_eq!(done.tracking(), vec!["LP1", "LP2"]);
    assert_eq!(done.fulfilled_tracking.as_deref(), Some("LP1, LP2"));
}
