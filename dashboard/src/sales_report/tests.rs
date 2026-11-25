use timada_core::db::params_from_iter;

use super::*;

const SHOP: &str = "tenant-shop";
const ACCOUNT: &str = "1234567890";
const EVER: Period = Period {
    from: 0,
    to: i64::MAX,
};
const DAYS: (&str, &str) = ("2026-11-01", "2026-11-30");

/// The read models, empty, with a product sold in two variants filed under
/// a category, and one in none.
async fn shop() -> Db {
    let turso = evento::Turso::in_memory_concurrent().await.unwrap();
    crate::db::migrate_store_read_models(&turso).await.unwrap();
    let db = Db::from(turso);
    for (id, name) in [("01TREE", "Cat tree"), ("01POST", "Scratching post")] {
        db.execute(
            "INSERT INTO catalog_products (tenant_id, id, slug, name, status, created_by,
                created_at, updated_at, image_count, variant_count, tax_category,
                slug_search, name_search)
             VALUES (?1, ?2, ?2, ?3, 'published', 'alice', 0, 0, 0, 0, 'standard', ?2, ?3)",
            (SHOP, id, name),
        )
        .await
        .unwrap();
    }
    db.execute(
        "INSERT INTO catalog_variants (tenant_id, product_id, id, position, amount_cents,
            currency, on_sale)
         VALUES (?1, '01TREE', '01TREEBIG', 0, 9900, 'EUR', 1)",
        (SHOP,),
    )
    .await
    .unwrap();
    db.execute(
        "INSERT INTO catalog_categories (tenant_id, id, slug, name, description, position,
            archived, in_menu, created_by, created_at, updated_at)
         VALUES (?1, 'cat-trees', 'trees', 'Cat trees', '', 0, 0, 1, 'alice', 0, 0)",
        (SHOP,),
    )
    .await
    .unwrap();
    db.execute(
        "INSERT INTO catalog_product_categories (tenant_id, product_id, category_id)
         VALUES (?1, '01TREE', 'cat-trees')",
        (SHOP,),
    )
    .await
    .unwrap();
    db
}

/// An order of one line, as the orders' read model keeps it.
async fn order(
    db: &Db,
    id: &str,
    source: Option<&str>,
    product: (&str, &str),
    total_cents: i64,
    profit_cents: Option<i64>,
) {
    db.execute(
        "INSERT INTO orders_sales (tenant_id, order_id, placed_at, currency, counted,
            total_cents, refunded_cents, item_count, profit_cents, source)
         VALUES (?1, ?2, 100, 'EUR', 1, ?3, 0, 1, ?4, ?5)",
        (SHOP, id, total_cents, profit_cents, source),
    )
    .await
    .unwrap();
    db.execute(
        "INSERT INTO orders_sale_lines (tenant_id, order_id, line, placed_at, currency,
            counted, product_id, name, quantity, amount_cents, profit_cents)
         VALUES (?1, ?2, 0, 100, 'EUR', 1, ?3, ?4, 1, ?5, ?6)",
        (SHOP, id, product.0, product.1, total_cents, profit_cents),
    )
    .await
    .unwrap();
}

/// The shop's Google Ads account, with one day of spend: `total` in all,
/// of which what `items` say went to each item of the feed.
async fn spend(db: &Db, currency: &str, total_micros: i64, items: &[(&str, i64)]) {
    db.execute(
        "INSERT INTO google_ads_accounts (tenant_id, customer_id, lapsed) VALUES (?1, ?2, 0)",
        (SHOP, ACCOUNT),
    )
    .await
    .unwrap();
    db.execute(
        "INSERT INTO google_ads_days (tenant_id, customer_id, day, currency, cost_micros,
            impressions, clicks, conversions_milli, value_micros)
         VALUES (?1, ?2, '2026-11-20', ?3, ?4, 1000, 50, 0, 0)",
        (SHOP, ACCOUNT, currency, total_micros),
    )
    .await
    .unwrap();
    for (item, micros) in items {
        db.execute(
            "INSERT INTO google_ads_products (tenant_id, customer_id, day, item_id,
                ad_group_id, campaign_id, cost_micros, impressions, clicks,
                conversions_milli, value_micros)
             VALUES (?1, ?2, '2026-11-20', ?3, '21', '11', ?4, 100, 5, 0, 0)",
            params_from_iter([
                timada_core::db::Value::from(SHOP),
                timada_core::db::Value::from(ACCOUNT),
                timada_core::db::Value::from(*item),
                timada_core::db::Value::from(*micros),
            ]),
        )
        .await
        .unwrap();
    }
}

#[test]
fn a_click_is_paid_when_its_campaign_says_so() {
    assert_eq!(paid_of(Some("google / cpc")), Paid::GoogleAds);
    assert_eq!(paid_of(Some("google / ppc")), Paid::GoogleAds);
    assert_eq!(paid_of(Some("facebook / paid_social")), Paid::Other);
    assert_eq!(paid_of(Some("bing / cpc")), Paid::Other);
    // A search, a mail, a link, a click Meta tagged without saying what
    // it is, and nothing said.
    for free in [
        "google / organic",
        "newsletter / email",
        "blog.example / referral",
        "facebook / fbclid",
        "(not set)",
    ] {
        assert_eq!(paid_of(Some(free)), Paid::Free, "{free}");
    }
    assert_eq!(paid_of(None), Paid::Free);
}

#[tokio::test]
async fn sales_are_set_against_what_the_ads_that_brought_them_cost() {
    let db = shop().await;
    // Google Ads brought two orders of the tree (60.00 left of 198.00) and
    // one of the post, which does not say what it left.
    order(
        &db,
        "o1",
        Some("google / cpc"),
        ("01TREE", "Cat tree"),
        9_900,
        Some(3_000),
    )
    .await;
    order(
        &db,
        "o2",
        Some("google / cpc"),
        ("01TREE", "Cat tree"),
        9_900,
        Some(3_000),
    )
    .await;
    order(
        &db,
        "o3",
        Some("google / cpc"),
        ("01POST", "Scratching post"),
        2_000,
        None,
    )
    .await;
    // A search, a mail and another network's ad brought the rest.
    order(
        &db,
        "o4",
        Some("google / organic"),
        ("01TREE", "Cat tree"),
        9_900,
        Some(3_000),
    )
    .await;
    order(
        &db,
        "o5",
        None,
        ("01POST", "Scratching post"),
        2_000,
        Some(500),
    )
    .await;
    order(
        &db,
        "o6",
        Some("facebook / paid_social"),
        ("01POST", "Scratching post"),
        2_000,
        Some(500),
    )
    .await;
    // 100.00 spent: 40.00 on the tree's two items, 30.00 on the post,
    // 10.00 on an item the catalog no longer has, the rest on no product.
    spend(
        &db,
        "EUR",
        100_000_000,
        &[
            ("01tree", 15_000_000),
            ("01treebig", 25_000_000),
            ("01post", 30_000_000),
            ("01old", 10_000_000),
        ],
    )
    .await;

    let report = ad_report(&db, SHOP, EVER, DAYS.0, DAYS.1).await.unwrap();
    assert!(report.connected);
    assert_eq!(report.currency, "EUR");
    assert_eq!((report.spend_cents, report.spend_days), (10_000, 1));

    // By kind of click, the paid ones first.
    let keys: Vec<&str> = report.channels.iter().map(ChannelRow::key).collect();
    assert_eq!(keys, vec!["google-ads", "other-paid", "free"]);
    let google = report.channel(Paid::GoogleAds).unwrap();
    assert_eq!((google.orders, google.revenue_cents), (3, 21_800));
    assert_eq!(google.spend_cents, Some(10_000));
    assert_eq!(google.return_hundredths(), Some(218));
    // One of its orders does not say what it left.
    assert_eq!(google.after_ads_cents(), None);
    let other = report.channel(Paid::Other).unwrap();
    assert_eq!((other.orders, other.spend_cents), (1, None));
    assert_eq!(other.return_hundredths(), None);
    let free = report.channel(Paid::Free).unwrap();
    assert_eq!((free.orders, free.revenue_cents), (2, 11_900));
    assert_eq!(free.after_ads_cents(), None);
    // 119.00 of 357.00.
    assert_eq!(report.free_share_bps(), Some(3_333));

    // By product, the one that cost the most first: a product's variants
    // are one product.
    let keys: Vec<&str> = report.products.iter().map(|row| row.key.as_str()).collect();
    assert_eq!(keys, vec!["01TREE", "01POST", "01old"]);
    let tree = &report.products[0];
    assert_eq!(tree.name, "Cat tree");
    assert_eq!(
        (tree.spend_cents, tree.quantity, tree.sales_cents),
        (4_000, 2, 19_800)
    );
    assert_eq!(tree.return_hundredths(), Some(495));
    // 198.00 sold for 60.00 left.
    assert_eq!(tree.break_even_hundredths(), Some(330));
    assert_eq!(tree.gap_hundredths(), Some(165));
    assert_eq!(tree.verdict(), Some(Verdict::Earns));
    assert_eq!(tree.after_ads_cents(), Some(2_000));
    let post = &report.products[1];
    assert_eq!((post.spend_cents, post.sales_cents), (3_000, 2_000));
    assert_eq!(post.return_hundredths(), Some(67));
    assert_eq!(post.break_even_hundredths(), None);
    assert_eq!(post.verdict(), Some(Verdict::NotKnown));
    let gone = &report.products[2];
    assert_eq!(
        (gone.product_id.as_deref(), gone.spend_cents),
        (None, 1_000)
    );
    assert_eq!(gone.verdict(), Some(Verdict::Costs));

    // By category, as the catalog files the products today.
    let read: Vec<(&str, &str, i64)> = report
        .categories
        .iter()
        .map(|row| (row.key.as_str(), row.name.as_str(), row.spend_cents))
        .collect();
    assert_eq!(
        read,
        vec![
            ("cat-trees", "Cat trees", 4_000),
            ("none", "In no category", 3_000),
            ("gone", "No longer in the catalog", 1_000),
        ]
    );
    assert_eq!(report.categories[0].verdict(), Some(Verdict::Earns));

    // Days with no spend read: the same sales, nothing to set them against.
    let before = ad_report(&db, SHOP, EVER, "2026-10-01", "2026-10-31")
        .await
        .unwrap();
    assert_eq!((before.spend_cents, before.spend_days), (0, 0));
    assert_eq!(before.currency, "EUR");
    assert_eq!(
        before.channel(Paid::GoogleAds).unwrap().spend_cents,
        Some(0)
    );
    assert_eq!(before.products[0].verdict(), None);

    // The file says the same, a section a kind of row.
    let file = sales_lines(&db, SHOP, EVER, DAYS.0, DAYS.1).await.unwrap();
    let lines: Vec<&str> = file.lines().collect();
    assert_eq!(lines[0], "total,,,EUR,6,6,357.00,0.00,,100.00,3.57,,");
    assert!(
        lines.contains(&"source,google / cpc,,EUR,3,,218.00,0.00,,,,,"),
        "{file}"
    );
    assert!(
        lines.contains(&"channel,google-ads,Google Ads,EUR,3,,218.00,0.00,,100.00,2.18,,"),
        "{file}"
    );
    assert!(
        lines.contains(&"channel,free,Without a paid click,EUR,2,,119.00,0.00,35.00,,,,"),
        "{file}"
    );
    assert!(
        lines.contains(&"product,01TREE,Cat tree,EUR,,2,198.00,,60.00,40.00,4.95,3.30,20.00"),
        "{file}"
    );
    assert!(
        lines.contains(&"product,01old,01old,EUR,,0,0.00,,,10.00,0.00,,-10.00"),
        "{file}"
    );
    assert!(
        lines.contains(&"category,cat-trees,Cat trees,EUR,,2,198.00,,60.00,40.00,4.95,3.30,20.00"),
        "{file}"
    );
}

#[tokio::test]
async fn a_shop_without_an_ads_account_still_tells_paid_from_free() {
    let db = shop().await;
    order(
        &db,
        "o1",
        Some("google / cpc"),
        ("01TREE", "Cat tree"),
        9_900,
        Some(3_000),
    )
    .await;
    order(
        &db,
        "o2",
        None,
        ("01POST", "Scratching post"),
        2_000,
        Some(500),
    )
    .await;
    let report = ad_report(&db, SHOP, EVER, DAYS.0, DAYS.1).await.unwrap();
    assert!(!report.connected);
    assert_eq!(report.currency, "EUR");
    let google = report.channel(Paid::GoogleAds).unwrap();
    // Nothing read: no spend to tell, no return.
    assert_eq!((google.orders, google.spend_cents), (1, None));
    assert_eq!(google.after_ads_cents(), None);
    assert_eq!(report.free_share_bps(), Some(1_681));
    // The product Google Ads sold is listed, with no ad to pay.
    assert_eq!(report.products.len(), 1);
    assert_eq!(report.products[0].verdict(), None);

    // Nothing sold, nothing spent.
    let empty = ad_report(&db, "tenant-other", EVER, DAYS.0, DAYS.1)
        .await
        .unwrap();
    assert_eq!(empty, AdReport::default());
}

#[tokio::test]
async fn spend_in_another_currency_is_not_set_against_the_sales() {
    let db = shop().await;
    order(
        &db,
        "o1",
        Some("google / cpc"),
        ("01TREE", "Cat tree"),
        9_900,
        Some(3_000),
    )
    .await;
    spend(&db, "USD", 50_000_000, &[("01tree", 50_000_000)]).await;
    let report = ad_report(&db, SHOP, EVER, DAYS.0, DAYS.1).await.unwrap();
    assert_eq!(report.currency, "USD");
    let google = report.channel(Paid::GoogleAds).unwrap();
    assert_eq!((google.orders, google.spend_cents), (0, Some(5_000)));
    assert_eq!(google.after_ads_cents(), Some(-5_000));
    let tree = &report.products[0];
    assert_eq!((tree.spend_cents, tree.sales_cents), (5_000, 0));
    assert_eq!(tree.verdict(), Some(Verdict::Costs));
    // The file's total, in euros, carries no spend.
    let file = sales_lines(&db, SHOP, EVER, DAYS.0, DAYS.1).await.unwrap();
    assert_eq!(
        file.lines().next(),
        Some("total,,,EUR,1,1,99.00,0.00,30.00,,,,")
    );
}
