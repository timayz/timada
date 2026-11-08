use timada_aliexpress::aggregator::SkuLink;
use timada_aliexpress::{LinkState, product_link_id};

const ALICE: &str = "user-alice";
const TENANT: &str = "acme";

async fn gateway() -> timada_aliexpress::Command<evento::Turso> {
    let executor = evento::Turso::in_memory_concurrent().await.unwrap();
    timada_aliexpress::Command::new(executor, TENANT)
}

fn sku(sku_id: &str) -> SkuLink {
    SkuLink {
        sku_id: sku_id.into(),
        variant_id: None,
        cost_cents: 100,
    }
}

/// The orders that buy a product on AliExpress, and the payments that wait
/// for that purchase, find its item in the store: the import records it in
/// the product's own stream, and that record follows the item's fate.
#[tokio::test]
async fn an_import_is_found_by_its_product_from_the_store() {
    let links = gateway().await;
    assert_eq!(links.load_link_of_product("product-1").await.unwrap(), None);

    let link_id = links
        .request_import(ALICE, "1005000000000001")
        .await
        .unwrap();
    // An item being imported fills nothing yet.
    assert_eq!(links.load_link_of_product("product-1").await.unwrap(), None);
    links
        .record_import(ALICE, &link_id, "product-1", vec![sku("A1")])
        .await
        .unwrap();

    let found: LinkState = links
        .load_link_of_product("product-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.id, link_id);
    assert_eq!(found.product_id.as_deref(), Some("product-1"));
    assert_eq!(found.skus, vec![sku("A1")]);

    // Unlinked, the product is no import's any more, whatever the index says.
    links.unlink(ALICE, &link_id).await.unwrap();
    assert_eq!(links.load_link_of_product("product-1").await.unwrap(), None);
}

/// The index is written once per item, however often the record is tried,
/// and the latest import of a product is the one that fills it.
#[tokio::test]
async fn the_index_is_written_once_and_the_latest_import_wins() {
    let links = gateway().await;
    let first = links
        .request_import(ALICE, "1005000000000001")
        .await
        .unwrap();
    timada_aliexpress::claim_product_link(
        links.executor(),
        TENANT,
        "product-1",
        &first,
        ALICE,
        None,
    )
    .await
    .unwrap();
    timada_aliexpress::claim_product_link(
        links.executor(),
        TENANT,
        "product-1",
        &first,
        ALICE,
        None,
    )
    .await
    .unwrap();
    let index = timada_aliexpress::product_link_projection()
        .load(product_link_id("product-1"))
        .tenant(TENANT)
        .execute(links.executor())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(index.aggregate_version, 1, "claimed again, nothing written");

    let second = links
        .request_import(ALICE, "1005000000000002")
        .await
        .unwrap();
    links
        .record_import(ALICE, &second, "product-1", vec![sku("B1")])
        .await
        .unwrap();
    let found = links
        .load_link_of_product("product-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.id, second);
}
