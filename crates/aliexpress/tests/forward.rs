use serde_json::json;
use timada_aliexpress::aggregator::SkuLink;
use timada_aliexpress::aggregator::forward::{Checked, Done, ForwardItem};
use timada_aliexpress::{
    AeOrder, AeProduct, AliexpressError, ForwardStatus, FreightOption, LinkState, LogisticsAddress,
    PlaceItem, PlaceOrder, Placement, PlannedItem, choose_service, delivery_country,
    describe_options, forward_items, freight_from_response, french_department, logistics_address,
    placed_orders_from_response, province_of, split_phone,
};
use timada_orders::{OrderLine, ShippingAddress};

const ALICE: &str = "user-alice";
/// The tenant the tests act in.
const TENANT: &str = "acme";

async fn executor() -> evento::Turso {
    evento::Turso::in_memory_concurrent().await.unwrap()
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
fn delivery_by_aliexpress_itself_is_chosen_free_first_then_cheapest() {
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
    // Nothing free of AliExpress's own: the cheapest it ships itself, never
    // the seller's, however cheap.
    assert_eq!(
        choose_service(&[
            option("YANWEN_JYT", 0, true),
            option("CAINIAO_PREMIUM", 900, true),
            option("CAINIAO_FULFILLMENT_STD", 199, true),
        ])
        .unwrap()
        .service_name,
        "CAINIAO_FULFILLMENT_STD"
    );
    assert!(choose_service(&[option("YANWEN_JYT", 0, true)]).is_none());
    assert_eq!(
        describe_options(
            &[
                option("CAINIAO_PREMIUM", 990, true),
                option("YANWEN_JYT", 0, false)
            ],
            "EUR"
        ),
        "CAINIAO_PREMIUM (9.90 EUR, tracked), YANWEN_JYT (free)"
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
        logistics_address(&address, Some("fr"), Some("01 89 76 02 12")),
        LogisticsAddress {
            full_name: "Ada Lovelace".into(),
            contact_person: "Ada Lovelace".into(),
            address: "12 rue de la Paix".into(),
            address2: "Bât. B".into(),
            city: "Paris".into(),
            province: "Paris".into(),
            zip: "75002".into(),
            country: "FR".into(),
            phone_country: "+33".into(),
            mobile_no: "612345678".into(),
            locale: "fr_FR".into(),
        }
    );
    // Without a phone of the shopper's, the shop's; without either, none.
    let silent = ShippingAddress {
        phone: Some(" ".into()),
        ..address.clone()
    };
    let mapped = logistics_address(&silent, None, Some("01 89 76 02 12"));
    assert_eq!(
        (mapped.phone_country.as_str(), mapped.mobile_no.as_str()),
        ("+33", "189760212")
    );
    let mapped = logistics_address(&silent, None, None);
    assert!(mapped.mobile_no.is_empty());
}

#[test]
fn a_french_postal_code_names_its_department_and_the_city_stands_in_elsewhere() {
    assert_eq!(french_department("75002"), Some("Paris"));
    assert_eq!(french_department("69003"), Some("Rhone"));
    assert_eq!(french_department("13 001"), Some("Bouches-du-Rhone"));
    assert_eq!(french_department("20000"), Some("Corse-du-Sud"));
    assert_eq!(french_department("20200"), Some("Haute-Corse"));
    assert_eq!(french_department("21000"), Some("Cote-d'Or"));
    assert_eq!(french_department("90000"), Some("Territoire de Belfort"));
    assert_eq!(french_department("95000"), Some("Val-d'Oise"));
    assert_eq!(french_department("97400"), None);
    assert_eq!(french_department("SW1A 1AA"), None);
    assert_eq!(province_of("FR", "59000", "Lille"), "Nord");
    assert_eq!(province_of("FR", "98800", "Nouméa"), "Other");
    assert_eq!(province_of("LU", "1009", " Luxembourg "), "Luxembourg");
}

#[test]
fn a_postal_code_names_the_province_aliexpress_lists() {
    assert_eq!(
        province_of("BE", "1000", "Bruxelles"),
        "Region Bruxelles-Capitale"
    );
    assert_eq!(province_of("BE", "1300", "Wavre"), "Region Wallonne");
    assert_eq!(province_of("BE", "1500", "Halle"), "Region Flamande");
    assert_eq!(province_of("BE", "2000", "Antwerpen"), "Region Flamande");
    assert_eq!(province_of("BE", "4000", "Liège"), "Region Wallonne");
    assert_eq!(province_of("BE", "9000", "Gent"), "Region Flamande");
    assert_eq!(province_of("BE", "0999", "Nulle part"), "Other");
    assert_eq!(province_of("ES", "28001", "Madrid"), "Madrid");
    assert_eq!(province_of("ES", "03001", "Alicante"), "Alacant");
    assert_eq!(province_of("ES", "48001", "Bilbao"), "Vizcaya");
    assert_eq!(province_of("ES", "99999", "Nulle part"), "Other");
    assert_eq!(province_of("CA", "h2x 1y4", "Montréal"), "Quebec");
    assert_eq!(province_of("CA", "M5V 2T6", "Toronto"), "Ontario");
    assert_eq!(province_of("CA", "X0A 0H0", "Iqaluit"), "Other");
    assert_eq!(province_of("IT", "66010", "Altino"), "Abruzzo");
    assert_eq!(province_of("IT", "00184", "Roma"), "Lazio");
    assert_eq!(province_of("IT", "20121", "Milano"), "Lombardia");
    assert_eq!(province_of("IT", "11100", "Aosta"), "Valle d'Aosta");
    // No "Other" on Italy's list: a postal code naming no region keeps the city.
    assert_eq!(province_of("IT", "99000", "Nulle part"), "Nulle part");
}

#[test]
fn other_is_sent_where_the_list_has_it_and_the_city_where_it_cannot_help() {
    for country in ["AT", "CZ", "DE", "GB", "LV", "NL", "PT"] {
        assert_eq!(
            province_of(country, "1010", "Somewhere"),
            "Other",
            "{country}"
        );
    }
    // A list without "Other" and no mapping yet, then no list at all.
    assert_eq!(province_of("US", "10001", "New York"), "New York");
    assert_eq!(province_of("SE", "11120", "Stockholm"), "Stockholm");
    assert_eq!(
        province_of("MQ", "97200", "Fort-de-France"),
        "Fort-de-France"
    );
}

#[test]
fn a_french_overseas_department_is_a_country_of_its_own_for_aliexpress() {
    assert_eq!(delivery_country("FR", "97200"), "MQ");
    assert_eq!(delivery_country(" fr ", "972 00"), "MQ");
    assert_eq!(delivery_country("FR", "97100"), "GP");
    assert_eq!(delivery_country("FR", "97300"), "GF");
    assert_eq!(delivery_country("FR", "97400"), "RE");
    assert_eq!(delivery_country("FR", "97600"), "YT");
    // Not part of Guadeloupe, and not departments.
    assert_eq!(delivery_country("FR", "97133"), "FR");
    assert_eq!(delivery_country("FR", "97150"), "FR");
    assert_eq!(delivery_country("FR", "98800"), "FR");
    assert_eq!(delivery_country("FR", "75002"), "FR");
    assert_eq!(delivery_country("FR", ""), "FR");
    // A postal code means nothing of the kind elsewhere.
    assert_eq!(delivery_country("DE", "97200"), "DE");
    assert_eq!(delivery_country("MQ", "97200"), "MQ");
}

#[test]
fn a_martinique_address_goes_as_martinique_with_its_city_and_calling_code() {
    let address = ShippingAddress {
        name: "Ada Lovelace".into(),
        line1: "1 rue Victor Hugo".into(),
        line2: None,
        postal_code: "97200".into(),
        city: "Fort-de-France".into(),
        country: "FR".into(),
        phone: Some("+596 696 12 34 56".into()),
    };
    let mapped = logistics_address(&address, Some("fr"), None);
    assert_eq!(mapped.country, "MQ");
    assert_eq!(mapped.province, "Fort-de-France");
    assert_eq!(mapped.zip, "97200");
    assert_eq!(mapped.locale, "fr_FR");
    assert_eq!(
        (mapped.phone_country.as_str(), mapped.mobile_no.as_str()),
        ("+596", "696123456")
    );
    // A number given the national way takes the department's code, one given
    // for the mainland keeps its own.
    assert_eq!(
        split_phone("0696 12 34 56", &mapped.country),
        ("+596".to_owned(), "696123456".to_owned())
    );
    assert_eq!(
        split_phone("+33 6 12 34 56 78", &mapped.country),
        ("+33".to_owned(), "612345678".to_owned())
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
async fn links(executor: &evento::Turso) -> Vec<LinkState> {
    let gateway = timada_aliexpress::Command::new(executor.clone(), TENANT);
    let sku = |sku_id: &str, variant_id: Option<&str>| SkuLink {
        sku_id: sku_id.into(),
        variant_id: variant_id.map(Into::into),
        cost_cents: 100,
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
    let after = forwards
        .record_checked(&id, version, vec![checked("81", "LP1")], None)
        .await
        .unwrap();
    assert_eq!(after, version + 1);
    // Nothing to record leaves it where it is.
    assert_eq!(
        forwards
            .record_checked(&id, after, Vec::new(), None)
            .await
            .unwrap(),
        after
    );
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
    assert_eq!(version, after);
    let after = forwards
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
    assert_eq!(after, version + 2);
    let (_, version) = forwards.load_forward_versioned(&id).await.unwrap().unwrap();
    assert_eq!(version, after);
    let done = forwards.load_forward("order-1").await.unwrap().unwrap();
    assert_eq!(done.status, ForwardStatus::Done);
    assert!(done.all_shipped());
    assert_eq!(done.tracking(), vec!["LP1", "LP2"]);
    assert_eq!(done.fulfilled_tracking.as_deref(), Some("LP1, LP2"));
}
