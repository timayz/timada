mod common;

use common::*;
use timada_catalog::CreateCategory;
use timada_core::db::Db;
use timada_orders::{
    BEST_SELLERS, Command, OrderCosts, OrderLine, Period, SALES_BY_CATEGORY, SALES_BY_SOURCE,
    SALES_TOTALS, best_sellers, read_model_subscription, sales_by_category, sales_by_source,
    sales_totals,
};

const EVER: Period = Period {
    from: 0,
    to: i64::MAX,
};

/// A store with the catalog's read models beside the orders': a report by
/// category reads how the catalog files its products.
async fn shop() -> (evento::Turso, Db) {
    let (executor, pool) = executor_with_pool().await;
    let mut conn = executor.connect().unwrap();
    timada_catalog::MIGRATIONS
        .to_latest(&mut conn)
        .await
        .map_err(|(_, err)| err)
        .unwrap();
    (executor, pool)
}

async fn catch_up(executor: &evento::Turso, pool: &Db) {
    timada_catalog::read_model_subscription::<evento::Turso>(pool.clone())
        .run_once(executor)
        .await
        .unwrap();
    // Twice: delivery may repeat without the sums drifting.
    for _ in 0..2 {
        read_model_subscription::<evento::Turso>(pool.clone())
            .run_once(executor)
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn a_period_s_sales_are_summed_by_currency_product_category_and_source() {
    let (executor, pool) = shop().await;
    let orders = Command::new(executor.clone(), TENANT);
    let catalog = timada_catalog::Command::new(executor.clone(), TENANT);
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    let plate = product(&catalog, "Plate", 900, "EUR").await;
    catalog.set_cost(ALICE, &mug, Some(400)).await.unwrap();
    let kitchen = catalog
        .create_category(
            ALICE,
            CreateCategory {
                name: "Kitchen".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    catalog
        .categorize_product(ALICE, &mug, &kitchen.id)
        .await
        .unwrap();

    // Two mugs, paid, 5.00 given back since.
    let first = cart_with(&orders, &mug, 2).await;
    let first = pay(&orders, &first, checkout()).await;
    orders
        .record_refund(ALICE, &first.order_id, "re_1", 500)
        .await
        .unwrap();
    // A mug and a plate, whose cost the shop never said: no profit told.
    let second = cart_with(&orders, &mug, 1).await;
    orders
        .add_to_cart(SHOPPER, &second, item(&plate, None, 1))
        .await
        .unwrap();
    pay(&orders, &second, checkout()).await;
    // A mug ordered and cancelled: it counts for nothing.
    let third = cart_with(&orders, &mug, 1).await;
    let third = place(&orders, &third, checkout()).await;
    cancel(&orders, &third.order_id, "Changed their mind").await;
    catch_up(&executor, &pool).await;

    let totals = sales_totals(&pool, TENANT, EVER).await.unwrap();
    assert_eq!(totals.len(), 1);
    let eur = &totals[0];
    assert_eq!(eur.currency, "EUR");
    assert_eq!(eur.orders, 2);
    assert_eq!(eur.revenue_cents, 2500 + 2150);
    assert_eq!(eur.refunded_cents, 500);
    assert_eq!(eur.net_cents(), 4150);
    assert_eq!(eur.items, 4);
    assert_eq!(eur.cancelled, 1);
    assert_eq!(eur.average_basket_cents(), Some(2325));
    // The first order alone knows what it left: 25.00 less two mugs at
    // 4.00, less the 5.00 given back.
    assert_eq!(eur.costed_orders, 1);
    assert_eq!(eur.profit_cents, 2500 - 800 - 500);

    let sellers = best_sellers(&pool, TENANT, EVER, 10).await.unwrap();
    assert_eq!(sellers.len(), 2);
    assert_eq!(sellers[0].product_id, mug);
    assert_eq!(sellers[0].name, "Mug");
    assert_eq!(sellers[0].quantity, 3);
    assert_eq!(sellers[0].sales_cents, 3750);
    assert_eq!(sellers[0].costed_quantity, 2);
    assert_eq!(sellers[0].profit_cents, 1700);
    assert_eq!(sellers[1].product_id, plate);
    assert_eq!(sellers[1].quantity, 1);
    assert_eq!(sellers[1].costed_quantity, 0);
    assert_eq!(best_sellers(&pool, TENANT, EVER, 1).await.unwrap().len(), 1);

    let categories = sales_by_category(&pool, TENANT, EVER).await.unwrap();
    assert_eq!(categories.len(), 2);
    assert_eq!(
        categories[0].category_id.as_deref(),
        Some(kitchen.id.as_str())
    );
    assert_eq!(categories[0].name.as_deref(), Some("Kitchen"));
    assert_eq!(categories[0].quantity, 3);
    assert_eq!(categories[0].sales_cents, 3750);
    assert_eq!(categories[0].profit_cents, 1700);
    assert_eq!(categories[1].category_id, None, "the plate is in none");
    assert_eq!(categories[1].sales_cents, 900);

    let sources = sales_by_source(&pool, TENANT, EVER).await.unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].source, None);
    assert_eq!(sources[0].orders, 2);
    assert_eq!(sources[0].revenue_cents, 4650);
    assert_eq!(sources[0].profit_cents, 1200);

    // Another shop's orders, and another period, are not this one's.
    assert!(sales_totals(&pool, "other", EVER).await.unwrap().is_empty());
    let before = Period { from: 0, to: 1 };
    assert!(
        sales_totals(&pool, TENANT, before)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        best_sellers(&pool, TENANT, before, 10)
            .await
            .unwrap()
            .is_empty()
    );

    // Each sum is read off the index on when the order was placed.
    for (statement, index, limit) in [
        (SALES_TOTALS, "orders_sales_placed", false),
        (SALES_BY_SOURCE, "orders_sales_placed", false),
        (BEST_SELLERS, "orders_sale_lines_placed", true),
        (SALES_BY_CATEGORY, "orders_sale_lines_placed", false),
    ] {
        let mut params = vec![
            timada_core::db::Value::from(TENANT),
            timada_core::db::Value::from(0i64),
            timada_core::db::Value::from(i64::MAX),
        ];
        if limit {
            params.push(timada_core::db::Value::from(10i64));
        }
        let plan: Vec<(i64, i64, i64, String)> = pool
            .fetch_all(
                &format!("EXPLAIN QUERY PLAN {statement}"),
                timada_core::db::params_from_iter(params),
            )
            .await
            .unwrap();
        let plan: Vec<String> = plan.into_iter().map(|(_, _, _, detail)| detail).collect();
        assert!(
            plan.iter()
                .any(|line| line.contains("SEARCH") && line.contains(index)),
            "{plan:?}"
        );
        assert!(!plan.iter().any(|line| line.contains("SCAN")), "{plan:?}");
    }
}

fn line(amount_cents: u64, quantity: u32) -> OrderLine {
    OrderLine {
        unit_amount_cents: amount_cents,
        quantity,
        ..OrderLine::default()
    }
}

#[test]
fn an_order_s_profit_is_shared_among_its_lines() {
    let lines = [line(1000, 2), line(500, 1), line(333, 1)];
    let costs = OrderCosts {
        unit_costs: vec![Some(400), Some(100), Some(50)],
        shipping_cents: 301,
        vat_cents: 472,
        ..OrderCosts::default()
    };
    // 28.33 paid, 7.73 of it to the carrier and the State: 20.60 to share
    // by what each line sold for, then each pays for its own goods.
    let profits = costs.line_profits(&lines, 2833).unwrap();
    assert_eq!(profits, vec![1454 - 800, 363 - 100, 243 - 50]);
    assert_eq!(
        profits.iter().sum::<i64>(),
        costs.profit_cents(&lines, 2833).unwrap()
    );
    // A discount that leaves less than the costs: the lines share the loss.
    let profits = costs.line_profits(&lines, 500).unwrap();
    assert_eq!(
        profits.iter().sum::<i64>(),
        costs.profit_cents(&lines, 500).unwrap()
    );
    // A line without a cost: nothing told of any.
    let unknown = OrderCosts {
        unit_costs: vec![Some(400), None, Some(50)],
        ..OrderCosts::default()
    };
    assert_eq!(unknown.line_profits(&lines, 2833), None);
}

/// What an export walks: every order of the period once, oldest first, a
/// page at a time from where the last one ended, each whole.
#[tokio::test]
async fn a_period_s_orders_are_walked_a_page_at_a_time() {
    use timada_core::page::Cursor;
    use timada_orders::{ORDERS_PLACED_OVER, orders_placed_over};

    let (executor, pool) = shop().await;
    let orders = Command::new(executor.clone(), TENANT);
    let catalog = timada_catalog::Command::new(executor.clone(), TENANT);
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    let mut placed = Vec::new();
    for quantity in 1..=5 {
        let cart = cart_with(&orders, &mug, quantity).await;
        placed.push(place(&orders, &cart, checkout()).await.order_id);
    }
    catch_up(&executor, &pool).await;

    // Placed within the same second or not, each comes once.
    let mut walked = Vec::new();
    let mut after: Option<Cursor> = None;
    loop {
        let page = orders_placed_over(&pool, TENANT, EVER, after.as_ref(), 2)
            .await
            .unwrap();
        let Some(last) = page.last() else {
            break;
        };
        assert!(page.len() <= 2);
        after = Some(Cursor {
            at: last.placed_at as i64,
            id: last.id.clone(),
        });
        walked.extend(page);
    }
    assert_eq!(walked.len(), 5);
    let mut ids: Vec<&str> = walked.iter().map(|order| order.id.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 5);
    assert!(
        walked
            .windows(2)
            .all(|pair| (pair[0].placed_at, &pair[0].id) < (pair[1].placed_at, &pair[1].id)),
        "oldest first"
    );
    // Whole: the lines and the address an export writes.
    assert_eq!(walked[0].lines[0].name, "Mug");
    assert_eq!(walked[0].shipping.name, "Ada Lovelace");

    // Another period, another shop: none of them.
    let before = Period { from: 0, to: 1 };
    assert!(
        orders_placed_over(&pool, TENANT, before, None, 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        orders_placed_over(&pool, "other", EVER, None, 10)
            .await
            .unwrap()
            .is_empty()
    );

    let plan: Vec<(i64, i64, i64, String)> = pool
        .fetch_all(
            &format!("EXPLAIN QUERY PLAN {ORDERS_PLACED_OVER}"),
            (TENANT, 0i64, "", i64::MAX, 10i64),
        )
        .await
        .unwrap();
    let plan: Vec<String> = plan.into_iter().map(|(_, _, _, detail)| detail).collect();
    assert!(
        plan.iter()
            .any(|line| line.contains("SEARCH") && line.contains("orders_orders_placed")),
        "{plan:?}"
    );
    assert!(!plan.iter().any(|line| line.contains("SCAN")), "{plan:?}");
}
