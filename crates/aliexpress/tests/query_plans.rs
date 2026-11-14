//! A probe of the query the sweep runs, on the catalogue the issue reckons with
//! (#373): 200,000 imported products over many shops, nearly all due. Ignored: it
//! takes a while and its report is its point. Run it with
//! `cargo test -p timada-aliexpress --test query_plans -- --ignored --nocapture`.

use std::time::Instant;

use timada_aliexpress::{MIGRATIONS, NODE_MIGRATIONS, links_due};
use timada_core::Share;
use timada_core::db::{Db, Turso};

/// How many products of each shop, one huge catalogue among the small ones.
fn products_of(shop: usize) -> usize {
    if shop == 0 { 100_000 } else { 100 }
}

#[tokio::test]
#[ignore = "seeds 200,000 rows; run by hand"]
async fn links_due_on_a_platform_with_more_products_than_it_can_read() {
    let store = Turso::temporary_concurrent().await.unwrap();
    let mut conn = store.connect().unwrap();
    for migrations in [&MIGRATIONS, &NODE_MIGRATIONS] {
        migrations
            .to_latest(&mut conn)
            .await
            .map_err(|(_, err)| err)
            .unwrap();
    }
    let db = Db::from(store);

    let started = Instant::now();
    let mut seeded = 0;
    for shop in 0..=1_000 {
        let tenant = format!("shop-{shop:04}");
        for chunk in (0..products_of(shop)).collect::<Vec<_>>().chunks(1_000) {
            let rows: Vec<String> = chunk
                .iter()
                .map(|index| {
                    seeded += 1;
                    let id = format!("{tenant}-{index:06}");
                    let tried = 1_000 + (seeded % 86_000);
                    let wanted = if seeded % 200 == 0 { "90000" } else { "NULL" };
                    let published = i32::from(seeded % 2 == 0);
                    format!(
                        "('{tenant}', '{id}', '{id}', '{id}', 'linked', 0, 0, {tried}, NULL, 0, 0, 0, \
                         {}, {wanted}, {published}, '[]')",
                        timada_core::shard_of(&id)
                    )
                })
                .collect();
            db.execute(
                &format!(
                    "INSERT INTO aliexpress_products (tenant_id, id, ae_product_id, product_id, \
                     status, offline, own_pricing, last_synced_at, last_error, requested_at, \
                     updated_at, new_combinations, shard, wanted_at, published, skus) \
                     VALUES {}",
                    rows.join(", ")
                ),
                (),
            )
            .await
            .unwrap();
        }
    }
    println!("seeded {seeded} rows in {:?}", started.elapsed());

    for (share, limit) in [
        (Share::ALONE, 41),
        (Share::ALONE, 100),
        (Share { index: 0, of: 3 }, 13),
    ] {
        let started = Instant::now();
        let due = links_due(&db, 100_000, 80_000, share, limit).await.unwrap();
        println!(
            "{share:?}, limit {limit}: {} rows in {:?}",
            due.len(),
            started.elapsed()
        );
        assert_eq!(due.len(), limit as usize);
    }
}
