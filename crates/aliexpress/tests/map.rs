use timada_aliexpress::{AeProduct, MapError, map_product, selections_of};
use timada_tenancy::{PricingRule, Rounding};

fn fixture() -> AeProduct {
    let body: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/product.json")).unwrap();
    AeProduct::from_response("1005006123456789", &body).unwrap()
}

#[test]
fn the_answer_reads_whatever_shape_its_fields_come_in() {
    let product = fixture();
    assert_eq!(product.id, "1005006123456789");
    assert!(product.on_sale());
    assert_eq!(product.images.len(), 2);
    assert_eq!(product.skus.len(), 3);
    let rose = product.sku("12000037001").unwrap();
    // The sale price wins over the list price; the shop's name for a value
    // over AliExpress's.
    assert_eq!(rose.cost_cents, Some(431));
    assert_eq!(rose.available, 120);
    assert_eq!(rose.properties[0].value, "Rose");
    let blue = product.sku("12000037002").unwrap();
    assert_eq!(blue.cost_cents, Some(550));
    assert_eq!(blue.available, 0);
    assert_eq!(product.sku("12000037003").unwrap().cost_cents, None);
}

#[test]
fn a_product_maps_to_options_variants_and_their_costs() {
    let rule = PricingRule {
        multiplier_bps: 25_000,
        plus_cents: 0,
        rounding: Rounding::Ninety9,
    };
    let mapped = map_product(&fixture(), "EUR", Some(rule)).unwrap();
    let seed = &mapped.seed;
    assert_eq!(seed.name, "Cat Harness & Leash Set");
    assert_eq!(seed.slug, "cat-harness-leash-set");
    assert!(seed.description.contains("Soft mesh harness."));
    assert_eq!(seed.currency, "EUR");
    // The unpriced combination is left out, with the value only it took.
    assert_eq!(seed.options.len(), 2);
    assert_eq!(seed.options[0].name, "Color");
    assert_eq!(seed.options[0].values, ["Rose", "Blue"]);
    assert_eq!(seed.options[1].values, ["S"]);
    assert_eq!(seed.variants.len(), 2);
    assert_eq!(seed.variants[0].amount_cents, 1099);
    assert_eq!(seed.variants[1].amount_cents, 1399);
    assert_eq!(seed.variants[0].sku.as_deref(), Some("12000037001"));
    // The combination's own picture joins the product's.
    assert_eq!(seed.images.len(), 3);
    assert_eq!(seed.variants[0].image, Some(2));
    assert_eq!(seed.attributes.len(), 2);
    assert_eq!(mapped.skus.len(), 2);
    assert_eq!(mapped.skus[0].cost_cents, 431);
    assert_eq!(mapped.skus[1].available, 0);
}

#[test]
fn without_a_rule_the_price_is_the_cost_and_a_withdrawn_product_is_refused() {
    let mapped = map_product(&fixture(), "EUR", None).unwrap();
    assert_eq!(mapped.seed.variants[0].amount_cents, 431);

    let mut product = fixture();
    product.status = "offline".into();
    assert_eq!(map_product(&product, "EUR", None), Err(MapError::Offline));
    let mut product = fixture();
    product.currency = "CNY".into();
    assert_eq!(
        map_product(&product, "EUR", None),
        Err(MapError::Currency("CNY".into()))
    );
}

#[test]
fn a_refusal_in_the_answer_is_told() {
    let body = serde_json::json!({
        "aliexpress_ds_product_get_response": { "rsp_code": "405", "rsp_msg": "The product does not exist" }
    });
    let err = AeProduct::from_response("1", &body).unwrap_err();
    assert!(err.gone);
    assert!(!err.retryable);
}

#[test]
fn a_combination_names_the_options_and_values_an_import_makes() {
    let product = fixture();
    let mapped = map_product(&product, "EUR", None).unwrap();
    for (variant, sku) in mapped.seed.variants.iter().zip(&mapped.skus) {
        let read = product.sku(&sku.sku_id).unwrap();
        let named: Vec<(String, String)> = variant
            .selections
            .iter()
            .map(|selection| (selection.option.clone(), selection.value.clone()))
            .collect();
        assert_eq!(selections_of(read), named);
    }
}
