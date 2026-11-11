mod common;

use timada_core::db::Db;

use common::*;
use timada_catalog::{
    Command, DefineVariant, OptionSelection, ProductFilter, ProductPage, ProductStatus, Sellable,
    categories_of_product, categories_of_tenant, category_by_id, count_products, live_child_count,
    product_by_id, product_count, products_of_tenant, read_model_subscription, sellable, tree_of,
    upsert_product,
};

async fn catch_up(executor: &evento::Turso, pool: &Db) {
    read_model_subscription::<evento::Turso>(pool.clone())
        .run_once(executor)
        .await
        .unwrap();
}

#[tokio::test]
async fn products_are_listed_searched_counted_and_stay_idempotent() {
    let (executor, pool) = executor_with_pool().await;
    let cmd = Command::new(executor.clone(), TENANT);
    let mug = create_product(&cmd, "Blue Mug").await;
    let plate = create_product(&cmd, "Plate").await;
    cmd.price_product(ALICE, &mug.id, 1250, "EUR")
        .await
        .unwrap();
    cmd.describe_product(ALICE, &mug.id, "Holds 50 cl.")
        .await
        .unwrap();
    cmd.publish_product(ALICE, &mug.id).await.unwrap();

    catch_up(&executor, &pool).await;
    catch_up(&executor, &pool).await;

    let all = ProductFilter::default();
    assert_eq!(count_products(&pool, TENANT, &all).await.unwrap(), 2);
    assert_eq!(count_products(&pool, "other", &all).await.unwrap(), 0);
    assert_eq!(
        product_count(&pool, TENANT).await.unwrap(),
        2,
        "the home's count"
    );
    let rows = products_of_tenant(&pool, TENANT, &all, ProductPage::First, 50)
        .await
        .unwrap()
        .rows;
    assert_eq!(rows.len(), 2);
    // Both were changed within the same second, so the order between them is
    // the id tiebreak; look them up instead.
    let mug_row = rows.iter().find(|r| r.id == mug.id).unwrap();
    assert_eq!(mug_row.name, "Blue Mug");
    assert_eq!(mug_row.slug, "blue-mug");
    assert_eq!(mug_row.price().unwrap().to_string(), "12.50 €");
    assert_eq!(mug_row.status(), ProductStatus::Published);
    assert_eq!(mug_row.created_by, ALICE);
    let plate_row = rows.iter().find(|r| r.id == plate.id).unwrap();
    assert_eq!(plate_row.price(), None);
    assert_eq!(plate_row.status(), ProductStatus::Draft);

    let found = products_of_tenant(
        &pool,
        TENANT,
        &all.clone().search(Some("MUG")),
        ProductPage::First,
        50,
    )
    .await
    .unwrap()
    .rows;
    assert_eq!(found.len(), 1, "search is case-insensitive");
    // The filter takes a status, on its own or with a search.
    let published = all.clone().status(Some(ProductStatus::Published));
    assert_eq!(count_products(&pool, TENANT, &published).await.unwrap(), 1);
    assert_eq!(
        products_of_tenant(&pool, TENANT, &published, ProductPage::First, 50)
            .await
            .unwrap()
            .rows[0]
            .id,
        mug.id
    );
    let drafts = all.clone().status(Some(ProductStatus::Draft));
    assert_eq!(count_products(&pool, TENANT, &drafts).await.unwrap(), 1);
    assert_eq!(
        count_products(&pool, TENANT, &drafts.clone().search(Some("mug")))
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        count_products(
            &pool,
            TENANT,
            &all.clone().status(Some(ProductStatus::Archived))
        )
        .await
        .unwrap(),
        0
    );
    assert_eq!(
        count_products(&pool, TENANT, &all.clone().search(Some("%")))
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        product_by_id(&pool, TENANT, &plate.id)
            .await
            .unwrap()
            .unwrap()
            .name,
        "Plate"
    );
    assert!(
        product_by_id(&pool, "other", &plate.id)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn an_archived_product_leaves_its_slug_to_the_next_one() {
    let (executor, pool) = executor_with_pool().await;
    let cmd = Command::new(executor.clone(), TENANT);
    let first = create_product(&cmd, "Blue Mug").await;
    cmd.archive_product(ALICE, &first.id).await.unwrap();
    let second = create_product(&cmd, "Blue Mug").await;
    catch_up(&executor, &pool).await;

    let rows = products_of_tenant(
        &pool,
        TENANT,
        &ProductFilter::default().search(Some("blue-mug")),
        ProductPage::First,
        50,
    )
    .await
    .unwrap()
    .rows;
    assert_eq!(rows.len(), 2, "both rows keep the slug, one archived");
    assert_eq!(rows[0].id, second.id);
    assert_eq!(rows[0].status(), ProductStatus::Draft);
    assert_eq!(rows[1].id, first.id);
    assert_eq!(rows[1].status(), ProductStatus::Archived);
}

#[tokio::test]
async fn a_deleted_product_loses_its_rows_and_its_category_count() {
    let (executor, pool) = executor_with_pool().await;
    let cmd = Command::new(executor.clone(), TENANT);
    let toys = create_category(&cmd, "Toys").await;
    let ball = create_product(&cmd, "Ball").await;
    cmd.categorize_product(ALICE, &ball.id, &toys.id)
        .await
        .unwrap();
    catch_up(&executor, &pool).await;
    let count = |pool: Db, id: String| async move {
        category_by_id(&pool, TENANT, &id)
            .await
            .unwrap()
            .unwrap()
            .product_count
    };
    assert_eq!(count(pool.clone(), toys.id.clone()).await, 1);

    cmd.delete_product(ALICE, &ball.id).await.unwrap();
    // Twice: delivery may repeat, with nothing left to drop.
    catch_up(&executor, &pool).await;
    catch_up(&executor, &pool).await;

    assert!(
        product_by_id(&pool, TENANT, &ball.id)
            .await
            .unwrap()
            .is_none(),
        "the row is gone, not archived"
    );
    assert_eq!(product_count(&pool, TENANT).await.unwrap(), 0);
    assert!(
        categories_of_product(&pool, TENANT, &ball.id)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(count(pool.clone(), toys.id.clone()).await, 0);
    let every = ProductFilter::default();
    assert_eq!(count_products(&pool, TENANT, &every).await.unwrap(), 0);
}

#[tokio::test]
async fn the_category_tree_carries_its_product_counts() {
    let (executor, pool) = executor_with_pool().await;
    let cmd = Command::new(executor.clone(), TENANT);
    let bedding = create_category(&cmd, "Bedding").await;
    let mats = create_child(&cmd, "Mats", &bedding.id).await;
    let toys = create_category(&cmd, "Toys").await;
    cmd.position_category(ALICE, &toys.id, 1).await.unwrap();
    cmd.set_category_in_menu(ALICE, &toys.id, false)
        .await
        .unwrap();
    let mat = create_product(&cmd, "Cooling mat").await;
    let ball = create_product(&cmd, "Ball").await;
    cmd.categorize_product(ALICE, &mat.id, &bedding.id)
        .await
        .unwrap();
    cmd.categorize_product(ALICE, &mat.id, &mats.id)
        .await
        .unwrap();
    cmd.categorize_product(ALICE, &ball.id, &toys.id)
        .await
        .unwrap();

    catch_up(&executor, &pool).await;
    catch_up(&executor, &pool).await;

    let rows = categories_of_tenant(&pool, TENANT).await.unwrap();
    assert_eq!(rows.len(), 3);
    assert!(
        categories_of_tenant(&pool, "other")
            .await
            .unwrap()
            .is_empty(),
        "rows are the tenant's own"
    );
    let tree = tree_of(rows);
    assert_eq!(
        tree.iter()
            .map(|n| n.item.name.as_str())
            .collect::<Vec<_>>(),
        ["Bedding", "Toys"],
        "position orders the top level"
    );
    assert_eq!(tree[0].children.len(), 1);
    assert_eq!(tree[0].children[0].item.name, "Mats");
    assert_eq!(tree[0].item.product_count, 1);
    assert_eq!(tree[0].children[0].item.product_count, 1);
    assert_eq!(tree[1].item.product_count, 1);
    assert!(tree[0].item.in_menu);
    assert!(!tree[1].item.in_menu, "hidden from the menu, still listed");
    assert_eq!(
        live_child_count(&pool, TENANT, &bedding.id).await.unwrap(),
        1
    );
    assert_eq!(live_child_count(&pool, TENANT, &toys.id).await.unwrap(), 0);

    // The filter takes a category, and membership rows follow the product.
    let in_bedding = ProductFilter::default().category(Some(&bedding.id));
    assert_eq!(count_products(&pool, TENANT, &in_bedding).await.unwrap(), 1);
    assert_eq!(
        products_of_tenant(&pool, TENANT, &in_bedding, ProductPage::First, 50)
            .await
            .unwrap()
            .rows[0]
            .id,
        mat.id
    );
    assert_eq!(
        categories_of_product(&pool, TENANT, &mat.id)
            .await
            .unwrap()
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        ["Bedding", "Mats"]
    );

    cmd.uncategorize_product(ALICE, &mat.id, &bedding.id)
        .await
        .unwrap();
    catch_up(&executor, &pool).await;
    assert_eq!(
        count_products(&pool, TENANT, &in_bedding).await.unwrap(),
        0,
        "leaving a category removes the row"
    );
    assert_eq!(
        category_by_id(&pool, TENANT, &bedding.id)
            .await
            .unwrap()
            .unwrap()
            .product_count,
        0
    );
}

#[tokio::test]
async fn an_archived_category_leaves_the_tree_but_keeps_its_memberships() {
    let (executor, pool) = executor_with_pool().await;
    let cmd = Command::new(executor.clone(), TENANT);
    let bedding = create_category(&cmd, "Bedding").await;
    let mat = create_product(&cmd, "Cooling mat").await;
    cmd.categorize_product(ALICE, &mat.id, &bedding.id)
        .await
        .unwrap();
    cmd.archive_category(ALICE, &bedding.id).await.unwrap();
    catch_up(&executor, &pool).await;

    assert!(
        categories_of_tenant(&pool, TENANT)
            .await
            .unwrap()
            .is_empty(),
        "an archived category is not listed"
    );
    let row = category_by_id(&pool, TENANT, &bedding.id)
        .await
        .unwrap()
        .unwrap();
    assert!(row.archived, "it is still there by id");
    let memberships = categories_of_product(&pool, TENANT, &mat.id).await.unwrap();
    assert_eq!(memberships.len(), 1);
    assert!(
        memberships[0].archived,
        "so the product page can show it as archived"
    );
}

#[tokio::test]
async fn an_archived_product_drops_out_of_its_categories_count() {
    let (executor, pool) = executor_with_pool().await;
    let cmd = Command::new(executor.clone(), TENANT);
    let bedding = create_category(&cmd, "Bedding").await;
    let mat = create_product(&cmd, "Cooling mat").await;
    cmd.categorize_product(ALICE, &mat.id, &bedding.id)
        .await
        .unwrap();
    catch_up(&executor, &pool).await;
    assert_eq!(
        category_by_id(&pool, TENANT, &bedding.id)
            .await
            .unwrap()
            .unwrap()
            .product_count,
        1
    );

    cmd.archive_product(ALICE, &mat.id).await.unwrap();
    catch_up(&executor, &pool).await;
    assert_eq!(
        category_by_id(&pool, TENANT, &bedding.id)
            .await
            .unwrap()
            .unwrap()
            .product_count,
        0,
        "an archived product is not counted"
    );
}

#[tokio::test]
async fn the_stored_counts_follow_a_move() {
    let (executor, pool) = executor_with_pool().await;
    let cmd = Command::new(executor.clone(), TENANT);
    let bedding = create_category(&cmd, "Bedding").await;
    let mats = create_child(&cmd, "Mats", &bedding.id).await;
    let toys = create_category(&cmd, "Toys").await;
    let mat = create_product(&cmd, "Cooling mat").await;
    cmd.categorize_product(ALICE, &mat.id, &bedding.id)
        .await
        .unwrap();
    cmd.categorize_product(ALICE, &mat.id, &mats.id)
        .await
        .unwrap();
    catch_up(&executor, &pool).await;
    let row = |id: String| {
        let pool = pool.clone();
        async move {
            let row = category_by_id(&pool, TENANT, &id).await.unwrap().unwrap();
            (row.product_count, row.subtree_count)
        }
    };
    assert_eq!(row(bedding.id.clone()).await, (1, 1), "counted once");
    assert_eq!(row(mats.id.clone()).await, (1, 1));
    assert_eq!(row(toys.id.clone()).await, (0, 0));

    cmd.move_category(ALICE, &mats.id, Some(&toys.id))
        .await
        .unwrap();
    catch_up(&executor, &pool).await;
    assert_eq!(row(bedding.id.clone()).await, (1, 1));
    assert_eq!(
        row(toys.id.clone()).await,
        (0, 1),
        "the new parent lists what came with it"
    );

    cmd.archive_product(ALICE, &mat.id).await.unwrap();
    catch_up(&executor, &pool).await;
    assert_eq!(row(bedding.id).await, (0, 0));
    assert_eq!(row(toys.id).await, (0, 0));
}

#[tokio::test]
async fn a_search_reads_the_index_of_its_own_tenant() {
    let (executor, pool) = executor_with_pool().await;
    let cmd = Command::new(executor.clone(), TENANT);
    let mug = create_product(&cmd, "Café Mug").await;
    create_product(&cmd, "Plate").await;
    catch_up(&executor, &pool).await;
    // A second tenant with a product of the same name.
    upsert_product(&pool, "tenant-zeta", &mug).await.unwrap();
    let search = |q: &'static str| ProductFilter::default().search(Some(q));

    assert_eq!(
        count_products(&pool, TENANT, &search("cafe"))
            .await
            .unwrap(),
        1,
        "one tenant's match, accents ignored"
    );
    assert_eq!(
        count_products(&pool, "tenant-zeta", &search("CAFÉ"))
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        count_products(&pool, TENANT, &search("é m")).await.unwrap(),
        1,
        "a phrase matches across a space"
    );
    assert_eq!(
        count_products(&pool, TENANT, &search("ug")).await.unwrap(),
        1,
        "shorter than the index cuts, still found"
    );

    cmd.rename_product(ALICE, &mug.id, "Tea Cup").await.unwrap();
    catch_up(&executor, &pool).await;
    assert_eq!(
        count_products(&pool, TENANT, &search("é m")).await.unwrap(),
        0,
        "the old name is out of the index"
    );
    assert_eq!(
        count_products(&pool, TENANT, &search("cafe"))
            .await
            .unwrap(),
        0,
        "and so is the old slug"
    );
    let found = products_of_tenant(&pool, TENANT, &search("tea c"), ProductPage::First, 50)
        .await
        .unwrap()
        .rows;
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].id, mug.id);
    assert_eq!(
        count_products(&pool, "tenant-zeta", &search("é m"))
            .await
            .unwrap(),
        1,
        "the other tenant's product kept its name"
    );
}

/// Most events of a product move none of its combinations: their rows are
/// left as they are, and written again only when one of them moved. Another
/// product's rows sit in the table too, so a rewrite shows in the row ids.
#[tokio::test]
async fn the_combinations_are_written_again_only_when_one_of_them_moved() {
    let (executor, pool) = executor_with_pool().await;
    let cmd = Command::new(executor.clone(), TENANT);
    let sell = |option_id: &str, value: &str, sku: &str, amount_cents: u64| DefineVariant {
        selections: vec![OptionSelection {
            option_id: option_id.to_owned(),
            value: value.to_owned(),
        }],
        sku: Some(sku.to_owned()),
        amount_cents,
        currency: "EUR".into(),
        image_id: None,
    };
    let harness = create_product(&cmd, "Cat Harness").await;
    let colour = cmd
        .define_option(
            ALICE,
            &harness.id,
            "Colour",
            vec!["Beige".into(), "Blue".into()],
        )
        .await
        .unwrap();
    let beige = cmd
        .define_variant(ALICE, &harness.id, sell(&colour, "Beige", "H-BEIGE", 2290))
        .await
        .unwrap();
    cmd.define_variant(ALICE, &harness.id, sell(&colour, "Blue", "H-BLUE", 1990))
        .await
        .unwrap();
    let collar = create_product(&cmd, "Collar").await;
    let size = cmd
        .define_option(ALICE, &collar.id, "Size", vec!["S".into()])
        .await
        .unwrap();
    cmd.define_variant(ALICE, &collar.id, sell(&size, "S", "C-S", 990))
        .await
        .unwrap();
    catch_up(&executor, &pool).await;

    let rows = async |pool: &Db| -> Vec<(i64, String, Option<String>, i64, bool)> {
        pool.fetch_all(
            "SELECT rowid, id, label, amount_cents, on_sale FROM catalog_variants
             WHERE tenant_id = ?1 AND product_id = ?2 ORDER BY position",
            (TENANT, harness.id.as_str()),
        )
        .await
        .unwrap()
    };
    let before = rows(&pool).await;
    assert_eq!(before.len(), 2);
    assert_eq!(before[0].1, beige);
    assert_eq!(before[0].2.as_deref(), Some("Beige"));

    // A description moves no combination: the rows stay, ids and all.
    cmd.describe_product(ALICE, &harness.id, "Soft, with a leash ring.")
        .await
        .unwrap();
    catch_up(&executor, &pool).await;
    assert_eq!(rows(&pool).await, before);

    // A price moves one: the rows are written again.
    cmd.price_variant(ALICE, &harness.id, &beige, 2490, "EUR")
        .await
        .unwrap();
    catch_up(&executor, &pool).await;
    let after = rows(&pool).await;
    assert_eq!(after[0].3, 2490);
    assert_eq!(after[1].3, 1990);
    assert!(
        after.iter().zip(&before).all(|(now, was)| now.0 != was.0),
        "written again: {after:?} vs {before:?}"
    );

    // What a cart is priced with reads the same from the rows as from the
    // product's own stream, a product sold through its combinations
    // carrying no price of its own either way.
    let product = cmd.load_product(&harness.id).await.unwrap().unwrap();
    let shown = sellable(&pool, TENANT, &harness.id).await.unwrap();
    assert_eq!(shown, Some(Sellable::of(&product)));
    assert_eq!(shown.unwrap().price, None);
    assert!(
        sellable(&pool, TENANT, "nope").await.unwrap().is_none(),
        "no row, no product"
    );
}

#[tokio::test]
async fn the_list_pages_from_where_the_page_beside_it_stopped() {
    let (executor, pool) = executor_with_pool().await;
    let cmd = Command::new(executor.clone(), TENANT);
    for n in 0..5 {
        create_product(&cmd, &format!("Mug {n}")).await;
    }
    catch_up(&executor, &pool).await;
    let all = ProductFilter::default();
    let ids = |page: &timada_catalog::ProductsPage| {
        page.rows
            .iter()
            .map(|row| row.id.clone())
            .collect::<Vec<_>>()
    };
    let every = products_of_tenant(&pool, TENANT, &all, ProductPage::First, 50)
        .await
        .unwrap();
    assert_eq!(every.rows.len(), 5);
    assert_eq!((every.newer.as_ref(), every.older.as_ref()), (None, None));

    let first = products_of_tenant(&pool, TENANT, &all, ProductPage::First, 2)
        .await
        .unwrap();
    assert_eq!(ids(&first), ids(&every)[..2]);
    assert!(
        first.newer.is_none(),
        "nothing is newer than the first page"
    );
    let older = first.older.clone().expect("more below");
    let second = products_of_tenant(&pool, TENANT, &all, ProductPage::Older(&older), 2)
        .await
        .unwrap();
    assert_eq!(ids(&second), ids(&every)[2..4]);
    let third = products_of_tenant(
        &pool,
        TENANT,
        &all,
        ProductPage::Older(second.older.as_ref().expect("more below")),
        2,
    )
    .await
    .unwrap();
    assert_eq!(ids(&third), ids(&every)[4..]);
    assert!(third.older.is_none(), "the last page has nothing below");

    // Back up: the page above the third is the second, and the one above
    // the second is the first, full.
    let back = products_of_tenant(
        &pool,
        TENANT,
        &all,
        ProductPage::Newer(third.newer.as_ref().expect("a page above")),
        2,
    )
    .await
    .unwrap();
    assert_eq!(ids(&back), ids(&second));
    assert_eq!(back.older, second.older);
    let top = products_of_tenant(
        &pool,
        TENANT,
        &all,
        ProductPage::Newer(back.newer.as_ref().expect("a page above")),
        2,
    )
    .await
    .unwrap();
    assert_eq!(top, first);

    // A cursor round-trips through a link.
    assert_eq!(
        timada_catalog::ProductCursor::parse(&older.param()),
        Some(older)
    );
    assert_eq!(timada_catalog::ProductCursor::parse("x-1"), None);
}
