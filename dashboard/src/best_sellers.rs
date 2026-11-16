//! Ranks each shop's best sellers.
//!
//! A storefront sorts a listing by what sells best, and it cannot see the
//! orders: its key reads the catalog. So every node, on a timer, works out
//! for the shops that fall to it the products each sold the most units of
//! over the last `[best_sellers] days`, off the sales read model, and
//! writes the ranking to the shop's catalog (`BestSellers::Ranked`) when it
//! differs from the one it has. The ranking names the products in order,
//! never how many were sold. A shop's storefront follows the stream and
//! copies the rank onto the pages that list each product.
//!
//! The walk goes from shop to shop among those that ever sold something, a
//! seek each, and reads one shop's sales of the period: its cost is the
//! shops', not the platform's orders. The nodes share the shops ([`Share`]);
//! two that disagree for a moment on who is up both write the same ranking,
//! the second finding it written. A store not taking writes ends the pass,
//! and the next starts over; a shop whose ranking cannot be written for a
//! reason of its own is logged and passed over.

use std::sync::Arc;
use std::time::Duration;

use timada_catalog::{Command, MAX_BEST_SELLERS};
use timada_core::Share;
use timada_orders::{best_selling_products, next_shop_with_sales};
use tokio::task::JoinHandle;

use crate::cluster::Executor;
use crate::db::Db;

/// Who the rankings say made them.
pub const RANKER: &str = "timada-dashboard";

/// The pass, with what it needs outside any request.
pub struct BestSellers {
    /// The read models: what each shop sold.
    db: Db,
    /// The store, every tenant's.
    store: Executor,
    /// This node's place among the nodes up, as of now: which shops it
    /// ranks.
    place: Arc<dyn Fn() -> Share + Send + Sync>,
    /// How far back the sales count.
    period: Duration,
}

impl BestSellers {
    pub fn new(
        db: Db,
        store: Executor,
        place: Arc<dyn Fn() -> Share + Send + Sync>,
        period: Duration,
    ) -> Arc<Self> {
        Arc::new(Self {
            db,
            store,
            place,
            period,
        })
    }

    /// Ranks every `every`, the first time right away.
    pub fn start(self: Arc<Self>, every: Duration) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(every);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tick.tick().await;
                match self.rank(crate::auth::now()).await {
                    Ok(0) => {}
                    Ok(shops) => tracing::info!(shops, "best sellers ranked again"),
                    Err(err) => tracing::error!(error = %err, "best sellers not ranked"),
                }
            }
        })
    }

    /// Ranks the shops of this node's part as their sales stand at `now`
    /// (unix seconds); returns how many rankings changed.
    pub async fn rank(&self, now: u64) -> anyhow::Result<usize> {
        let from = now.saturating_sub(self.period.as_secs()) as i64;
        let share = (self.place)();
        let mut written = 0;
        let mut after = String::new();
        while let Some(shop) = next_shop_with_sales(&self.db, &after).await? {
            if share.owns(&shop) {
                let ranked =
                    best_selling_products(&self.db, &shop, from, MAX_BEST_SELLERS as i64).await?;
                let catalog = Command::new(self.store.clone(), &shop);
                match catalog.rank_best_sellers(RANKER, &ranked, now).await {
                    Ok(true) => written += 1,
                    Ok(false) => {}
                    // The store not taking writes, or not answering: the
                    // next pass starts over.
                    Err(err) if err.is_store_fault() => return Err(err.into()),
                    Err(err) => {
                        tracing::warn!(tenant = %shop, error = %err, "a shop's best sellers not ranked");
                    }
                }
            }
            after = shop;
        }
        Ok(written)
    }
}

#[cfg(test)]
mod tests {
    use evento::Evento;
    use timada_catalog::best_sellers_id;

    use super::*;

    const DAY: u64 = 24 * 60 * 60;
    const PERIOD: Duration = Duration::from_secs(30 * DAY);
    /// 2026-11-16 12:00 UTC.
    const NOW: u64 = 1_794_830_400;

    async fn store() -> (Evento, Db) {
        let turso = evento::Turso::in_memory_concurrent().await.unwrap();
        crate::db::migrate_store_read_models(&turso).await.unwrap();
        (Evento::new(turso.clone()), Db::from(turso))
    }

    /// A line of a counted order of `shop`, placed `days` ago.
    async fn sold(db: &Db, shop: &str, product: &str, quantity: i64, days: u64, counted: bool) {
        let order = ulid::Ulid::generate().to_string();
        db.execute(
            "INSERT INTO orders_sale_lines (tenant_id, order_id, line, placed_at, currency,
                counted, product_id, name, quantity, amount_cents)
             VALUES (?1, ?2, 0, ?3, 'EUR', ?4, ?5, ?5, ?6, 1000)",
            (
                shop,
                order.as_str(),
                (NOW - days * DAY) as i64,
                counted,
                product,
                quantity,
            ),
        )
        .await
        .unwrap();
    }

    async fn ranking(store: &Evento, shop: &str) -> Option<Vec<String>> {
        Command::new(store.clone(), shop)
            .load_best_sellers(&best_sellers_id(NOW))
            .await
            .unwrap()
            .map(|state| state.product_ids)
    }

    fn node(store: &Evento, db: &Db, share: Share) -> Arc<BestSellers> {
        BestSellers::new(db.clone(), store.clone(), Arc::new(move || share), PERIOD)
    }

    #[tokio::test]
    async fn a_shop_is_ranked_by_the_units_it_sold_over_the_period() {
        let (store, db) = store().await;
        sold(&db, "acme", "mug", 1, 2, true).await;
        sold(&db, "acme", "mat", 2, 3, true).await;
        sold(&db, "acme", "mat", 2, 20, true).await;
        // Out of the period, and an order that does not count.
        sold(&db, "acme", "wand", 50, 31, true).await;
        sold(&db, "acme", "bowl", 50, 1, false).await;
        sold(&db, "globex", "desk", 1, 1, true).await;

        let alone = node(&store, &db, Share::ALONE);
        assert_eq!(alone.rank(NOW).await.unwrap(), 2);
        assert_eq!(
            ranking(&store, "acme").await.unwrap(),
            ["mat".to_owned(), "mug".to_owned()]
        );
        assert_eq!(
            ranking(&store, "globex").await.unwrap(),
            ["desk".to_owned()]
        );
        // Nothing sold since: nothing written.
        assert_eq!(alone.rank(NOW).await.unwrap(), 0);

        // The mug overtakes the mat: the one shop is ranked again.
        sold(&db, "acme", "mug", 9, 0, true).await;
        assert_eq!(alone.rank(NOW).await.unwrap(), 1);
        assert_eq!(
            ranking(&store, "acme").await.unwrap(),
            ["mug".to_owned(), "mat".to_owned()]
        );
        // Shops whose sales all left the period are ranked empty, once.
        let later = NOW + 40 * DAY;
        assert_eq!(alone.rank(later).await.unwrap(), 2);
        assert_eq!(alone.rank(later).await.unwrap(), 0);
        let globex = Command::new(store.clone(), "globex")
            .load_best_sellers(&best_sellers_id(later))
            .await
            .unwrap()
            .unwrap();
        assert!(globex.product_ids.is_empty());
    }

    #[tokio::test]
    async fn each_node_ranks_its_own_part_of_the_shops() {
        let (store, db) = store().await;
        let shops: Vec<String> = (0..8).map(|n| format!("shop-{n}")).collect();
        for shop in &shops {
            sold(&db, shop, "mug", 1, 1, true).await;
        }
        let first = Share { index: 0, of: 2 };
        let second = Share { index: 1, of: 2 };
        let ranked = node(&store, &db, first).rank(NOW).await.unwrap();
        let theirs = shops.iter().filter(|shop| second.owns(shop)).count();
        assert_eq!(ranked, shops.len() - theirs);
        for shop in &shops {
            assert_eq!(ranking(&store, shop).await.is_some(), first.owns(shop));
        }
        assert_eq!(node(&store, &db, second).rank(NOW).await.unwrap(), theirs);
        assert_eq!(node(&store, &db, Share::ALONE).rank(NOW).await.unwrap(), 0);
    }

    /// The walk seeks from shop to shop, and a shop's ranking reads its own
    /// lines of the period.
    #[tokio::test]
    async fn the_walk_and_the_ranking_read_the_index() {
        let (_, db) = store().await;
        for (statement, params) in [
            (
                timada_orders::NEXT_SHOP_WITH_SALES,
                vec![timada_core::db::Value::from("")],
            ),
            (
                timada_orders::BEST_SELLING_PRODUCTS,
                vec!["acme".into(), 0i64.into(), 10i64.into()],
            ),
        ] {
            let plan: Vec<String> = db
                .fetch_all::<(i64, i64, i64, String)>(
                    &format!("EXPLAIN QUERY PLAN {statement}"),
                    timada_core::db::params_from_iter(params),
                )
                .await
                .unwrap()
                .into_iter()
                .map(|(_, _, _, detail)| detail)
                .collect();
            assert!(
                plan.iter()
                    .any(|step| step.starts_with("SEARCH")
                        && step.contains("orders_sale_lines_placed")),
                "{plan:#?}"
            );
            assert!(
                !plan.iter().any(|step| step.starts_with("SCAN")),
                "{plan:#?}"
            );
        }
    }
}
