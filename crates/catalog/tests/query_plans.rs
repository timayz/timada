//! Probes of the products list under Turso's planner (issue #473): seeds a
//! tenant of 20,000 products among 40,000, prints `EXPLAIN QUERY PLAN` and a
//! timing for each page the list reads, and asserts that a page is read off
//! an index in its order, never sorted. Ignored: they take a while and their
//! report is their point. Run them with
//! `cargo test -p timada-catalog --test query_plans -- --ignored --nocapture`.

use std::time::Instant;

use timada_catalog::{
    ProductCursor, ProductFilter, ProductPage, ProductStatus, Search, count_products,
    page_statement, products_of_tenant,
};
use timada_core::db::{
    Connection, Db, IntoParams, Turso, Value, execute, fetch_all, fetch_scalar, params_from_iter,
    search_key, tenant_search_key,
};

/// A database file opened for several writers, with the catalog schema.
async fn database() -> (Turso, Db) {
    let store = Turso::temporary_concurrent().await.unwrap();
    let mut conn = store.connect().unwrap();
    timada_catalog::MIGRATIONS
        .to_latest(&mut conn)
        .await
        .map_err(|(_, err)| err)
        .unwrap();
    (store.clone(), Db::from(store))
}

/// The `detail` column of `EXPLAIN QUERY PLAN sql`, one line per step.
async fn plan(conn: &Connection, sql: &str, params: impl IntoParams) -> Vec<String> {
    let rows: Vec<(i64, i64, i64, String)> =
        fetch_all(conn, &format!("EXPLAIN QUERY PLAN {sql}"), params)
            .await
            .unwrap();
    rows.into_iter().map(|(_, _, _, detail)| detail).collect()
}

/// Prints the plan of `sql` under `title`, and returns it.
async fn report_plan(
    conn: &Connection,
    title: &str,
    sql: &str,
    params: impl IntoParams,
) -> Vec<String> {
    println!("--- {title}");
    let plan = plan(conn, sql, params).await;
    for line in &plan {
        println!("    {line}");
    }
    plan
}

/// Fails when `plan` sorts its rows instead of reading them in order.
fn assert_no_sort(plan: &[String]) {
    assert!(
        !plan.iter().any(|line| line.contains("B-TREE")),
        "the plan sorts:\n{}",
        plan.join("\n")
    );
}

/// Fails unless a step of `plan` names `index`.
fn assert_uses(plan: &[String], index: &str) {
    assert!(
        plan.iter().any(|line| line.contains(index)),
        "no step reads {index}:\n{}",
        plan.join("\n")
    );
}

/// Runs `run` `rounds` times and prints the mean wall time under `title`.
async fn timed<F, Fut, T>(title: &str, rounds: u32, mut run: F) -> T
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = T>,
{
    let mut last = None;
    let started = Instant::now();
    for _ in 0..rounds {
        last = Some(run().await);
    }
    let each = started.elapsed() / rounds;
    println!("    {title}: {each:?} per run ({rounds} runs)");
    last.unwrap()
}

/// The probed tenant, with an id shaped like the real ones.
const SHOP: &str = "01jc5z8q7m3n4p5r6s7t8v9w0x";
const SHOP_PRODUCTS: usize = 20_000;
const OTHER_TENANTS: usize = 4;
const OTHER_PRODUCTS: usize = 5_000;
const WORDS: &[&str] = &[
    "mug", "plate", "bowl", "tea", "cup", "vase", "lamp", "rug", "pillow", "throw", "candle",
    "basket",
];

/// Seeds `count` products of `tenant`, one in ten a draft, named from
/// `WORDS` so a search for one word matches a twelfth of them. Written
/// `BATCH` rows a statement: each statement naming the full-text columns
/// builds a segment of the index, and one per row takes minutes.
async fn seed(conn: &Connection, tenant: &str, count: usize) {
    const BATCH: usize = 500;
    for start in (0..count).step_by(BATCH) {
        let mut values = Vec::new();
        let mut rows = Vec::new();
        for n in start..(start + BATCH).min(count) {
            let name = format!(
                "{} {} {n}",
                WORDS[n % WORDS.len()],
                WORDS[(n / 7) % WORDS.len()]
            );
            let slug = name.replace(' ', "-");
            let status = if n % 10 == 0 { "draft" } else { "published" };
            let at = values.len();
            rows.push(format!(
                "(?{}, ?{}, ?{}, ?{}, 1200, 'EUR', ?{}, 'u', ?{}, ?{}, NULL, 0, 1200, 1200, 0,
                    'standard', ?{}, ?{})",
                at + 1,
                at + 2,
                at + 3,
                at + 4,
                at + 5,
                at + 6,
                at + 6,
                at + 7,
                at + 8
            ));
            values.extend([
                Value::from(tenant),
                Value::from(format!("p{n:07}")),
                Value::from(slug.as_str()),
                Value::from(name.as_str()),
                Value::from(status),
                Value::from((n * 37 % count) as i64),
                Value::from(tenant_search_key(tenant, &slug)),
                Value::from(search_key(&name)),
            ]);
        }
        execute(
            conn,
            &format!(
                "INSERT INTO catalog_products (tenant_id, id, slug, name, amount_cents, currency,
                    status, created_by, created_at, updated_at, thumbnail, image_count,
                    min_amount_cents, max_amount_cents, variant_count, tax_category,
                    slug_search, name_search)
                 VALUES {}",
                rows.join(", ")
            ),
            params_from_iter(values),
        )
        .await
        .unwrap();
    }
}

#[tokio::test]
#[ignore = "a probe: run with --ignored --nocapture"]
async fn the_list_reads_its_pages_off_the_index() {
    let (store, db) = database().await;
    let conn = store.connect().unwrap();
    let started = Instant::now();
    seed(&conn, SHOP, SHOP_PRODUCTS).await;
    println!("shop seeded in {:?}", started.elapsed());
    for t in 0..OTHER_TENANTS {
        seed(&conn, &format!("01jc5z8q7m{t:016}"), OTHER_PRODUCTS).await;
    }
    println!("seeded in {:?}", started.elapsed());

    let published = ProductFilter::default().status(Some(ProductStatus::Published));
    let every = ProductFilter::default();
    // The statements of the list, as `products_of_tenant` writes them.
    let page = |status: Option<&'static str>| {
        (
            SHOP,
            None::<String>,
            None::<String>,
            status,
            1_000,
            "p0000500",
            51,
        )
    };
    let steps = report_plan(
        &conn,
        "published, a page",
        &page_statement(Search::None, &published, true),
        page(Some("published")),
    )
    .await;
    assert_no_sort(&steps);
    assert_uses(&steps, "catalog_products_status_changed");
    let steps = report_plan(
        &conn,
        "every status, a page",
        &page_statement(Search::None, &every, true),
        page(None),
    )
    .await;
    assert_no_sort(&steps);
    assert_uses(&steps, "catalog_products_changed");
    let steps = report_plan(
        &conn,
        "published, the page above",
        &page_statement(Search::None, &published, false),
        page(Some("published")),
    )
    .await;
    assert_no_sort(&steps);
    report_plan(
        &conn,
        "search, a page",
        &page_statement(Search::Phrase, &published, true),
        (
            SHOP,
            "+\"3n4p5r6s7t8v9w0x\" +\"candle\"",
            None::<String>,
            "published",
            i64::MAX,
            "",
            51,
        ),
    )
    .await;

    let first = timed("published, first page", 20, || {
        products_of_tenant(&db, SHOP, &published, ProductPage::First, 50)
    })
    .await
    .unwrap();
    assert_eq!(first.rows.len(), 50);
    // The deepest page: its cursor is the last row of the list.
    let last: (i64, String) = fetch_all(
        &conn,
        "SELECT updated_at, id FROM catalog_products WHERE tenant_id = ?1 AND status = 'published'
         ORDER BY updated_at, id LIMIT 1 OFFSET 60",
        (SHOP,),
    )
    .await
    .unwrap()
    .remove(0);
    let deep = ProductCursor {
        updated_at: last.0,
        id: last.1,
    };
    let page = timed("published, deepest page", 20, || {
        products_of_tenant(&db, SHOP, &published, ProductPage::Older(&deep), 50)
    })
    .await
    .unwrap();
    assert_eq!(page.rows.len(), 50);
    timed("published, back up from the deepest", 20, || {
        products_of_tenant(&db, SHOP, &published, ProductPage::Newer(&deep), 50)
    })
    .await
    .unwrap();
    timed("every status, first page", 20, || {
        products_of_tenant(&db, SHOP, &every, ProductPage::First, 50)
    })
    .await
    .unwrap();
    let n = timed("count, published", 20, || {
        count_products(&db, SHOP, &published)
    })
    .await
    .unwrap();
    assert_eq!(n, (SHOP_PRODUCTS - SHOP_PRODUCTS / 10) as i64);

    // A search: the phrase alone matches every shop's products, the
    // tenant's token narrows the index's answer to this shop's.
    let fts = "SELECT count(*) FROM catalog_products
         WHERE tenant_id = ?1 AND fts_match(slug_search, name_search, ?2)";
    let alone: i64 = timed("search, phrase alone", 10, || {
        fetch_scalar(&conn, fts, (SHOP, "\"candle\""))
    })
    .await
    .unwrap();
    let tokened: i64 = timed("search, phrase and token", 10, || {
        fetch_scalar(&conn, fts, (SHOP, "+\"3n4p5r6s7t8v9w0x\" +\"candle\""))
    })
    .await
    .unwrap();
    assert_eq!(alone, tokened, "the token keeps every match of the shop");
    let candle = published.clone().search(Some("candle"));
    let short = published.clone().search(Some("cu"));
    let matched = timed("search, the list's page", 10, || {
        products_of_tenant(&db, SHOP, &candle, ProductPage::First, 50)
    })
    .await
    .unwrap();
    assert_eq!(matched.rows.len(), 50);
    timed("search, the list's count", 10, || {
        count_products(&db, SHOP, &candle)
    })
    .await
    .unwrap();
    timed("short search, the list's page", 10, || {
        products_of_tenant(&db, SHOP, &short, ProductPage::First, 50)
    })
    .await
    .unwrap();
}
