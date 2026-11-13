//! Drops the carts nobody touched for long.
//!
//! A shopper starts a cart at their first add and the storefront keeps its
//! id; one who never comes back leaves the cart behind, and so does a
//! visitor that never kept the id. Nothing in the event store fires later,
//! so every node looks, on a timer, for the carts the read model says lay
//! idle past `[carts] idle_days` with nothing about them that could still
//! move money (no payment open, none on its way back, no request to pay
//! unanswered), checks each on its write model and drops it
//! (`Cart::Expired`, written at the version read, so a shopper back in
//! between keeps it). From then on the cart is gone for every command and
//! every reader: a storefront still naming it is told there is no such
//! cart and starts another. Its row leaves the read model, so the shop's
//! open carts and the checkout sweeps walk the carts that matter. The store
//! keeps the stream: an event is never deleted.
//!
//! The nodes share the work: every node's read model holds every cart, and
//! each drops the ones that fall to it among the nodes up ([`Share`]), so a
//! cart costs one write, not one per node. Two nodes that disagree for a
//! moment on who is up may both try a cart; the write at the version read
//! lets one through and the other finds it dropped. A node that stops leaves
//! its part to the others once they no longer hear from it.
//!
//! A look drops at most [`CARTS_PER_LOOK`] carts, the oldest first; while a
//! look drops that many, the next follows a [`BREATH`] later rather than at
//! the next tick, so a backlog goes out in batches instead of one burst of
//! writes, and outpaces the carts the storefronts may start. A store not
//! taking writes ends the sweep; a cart that cannot be dropped for a reason
//! of its own is logged and passed over.

use std::sync::Arc;
use std::time::Duration;

use timada_core::Share;
use timada_orders::{CartCursor, Command, idle_carts_before};
use tokio::task::JoinHandle;

use crate::cluster::Executor;
use crate::db::Db;

/// Who the drops say made them.
pub const DROPPER: &str = "timada-dashboard";

/// The most carts one look drops on this node.
pub const CARTS_PER_LOOK: usize = 1_000;

/// The rows read at a time.
const PAGE: i64 = 200;

/// The pause after a look that dropped its full [`CARTS_PER_LOOK`], before
/// the next.
const BREATH: Duration = Duration::from_secs(10);

/// The look, with what it needs outside any request.
pub struct IdleCarts {
    /// The read models: the carts that lay idle.
    db: Db,
    /// The store, every tenant's, read from this node's file: a cart the
    /// shopper touched a moment ago on another node is kept, its drop
    /// refused at the version read.
    store: Executor,
    /// This node's place among the nodes up, as of now: which carts it drops.
    place: Arc<dyn Fn() -> Share + Send + Sync>,
    /// How long a cart is kept after its last change.
    idle: Duration,
}

impl IdleCarts {
    pub fn new(
        db: Db,
        store: Executor,
        place: Arc<dyn Fn() -> Share + Send + Sync>,
        idle: Duration,
    ) -> Arc<Self> {
        Arc::new(Self {
            db,
            store,
            place,
            idle,
        })
    }

    /// Looks every `every`, the first time right away.
    pub fn start(self: Arc<Self>, every: Duration) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(every);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tick.tick().await;
                match self.sweep(crate::auth::now()).await {
                    Ok(0) => {}
                    Ok(dropped) => tracing::info!(dropped, "idle carts dropped"),
                    Err(err) => tracing::error!(error = %err, "idle carts not dropped"),
                }
            }
        })
    }

    /// Drops the carts of this node's part that lay idle at `now` (unix
    /// seconds), a look at a time, the next a [`BREATH`] later while a look
    /// drops its full [`CARTS_PER_LOOK`]; returns how many it dropped.
    pub async fn sweep(&self, now: u64) -> anyhow::Result<usize> {
        self.sweep_paced(now, BREATH, PAGE, CARTS_PER_LOOK).await
    }

    /// [`sweep`](Self::sweep), `breath` apart, each look reading `page`
    /// rows at a time and dropping up to `most`.
    async fn sweep_paced(
        &self,
        now: u64,
        breath: Duration,
        page: i64,
        most: usize,
    ) -> anyhow::Result<usize> {
        let mut dropped = 0;
        loop {
            let looked = self.look(now, page, most).await?;
            dropped += looked;
            if looked < most {
                return Ok(dropped);
            }
            tokio::time::sleep(breath).await;
        }
    }

    /// One look: the idle carts of this node's part, `page` rows at a time,
    /// oldest first, until `most` are dropped or none is left.
    async fn look(&self, now: u64, page: i64, most: usize) -> anyhow::Result<usize> {
        let idle_secs = self.idle.as_secs();
        let before = now.saturating_sub(idle_secs) as i64;
        let share = (self.place)();
        let mut after: Option<CartCursor> = None;
        let mut dropped = 0;
        loop {
            let rows = idle_carts_before(&self.db, before, after.as_ref(), page).await?;
            for row in rows.iter().filter(|row| share.owns(&row.id)) {
                let orders = Command::new(self.store.clone(), &row.tenant_id);
                match orders.expire_cart(DROPPER, &row.id, now, idle_secs).await {
                    Ok(true) => {
                        dropped += 1;
                        if dropped == most {
                            return Ok(dropped);
                        }
                    }
                    // Touched since its row was read, or dropped already.
                    Ok(false) => {}
                    // The store not taking writes, or not answering: the
                    // next sweep goes on from the oldest.
                    Err(err) if err.is_store_fault() => return Err(err.into()),
                    Err(err) => {
                        tracing::warn!(tenant = %row.tenant_id, cart = %row.id, error = %err, "an idle cart not dropped");
                    }
                }
            }
            match rows.last() {
                Some(last) if rows.len() as i64 == page => after = Some(last.into()),
                _ => return Ok(dropped),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use evento::Evento;
    use timada_catalog::CreateProduct;
    use timada_orders::{CartItem, STOREFRONT_ACTOR, read_model_subscription};

    use super::*;

    const DAY: u64 = 24 * 60 * 60;
    const IDLE: Duration = Duration::from_secs(30 * DAY);

    /// The store, its read models, and a mug on sale in each shop.
    async fn store(shops: &[&str]) -> (Evento, Db, Vec<String>) {
        let turso = evento::Turso::in_memory_concurrent().await.unwrap();
        crate::db::migrate_store_read_models(&turso).await.unwrap();
        let db = Db::from(turso.clone());
        let store = Evento::new(turso);
        let mut mugs = Vec::new();
        for shop in shops {
            let catalog = timada_catalog::Command::new(store.clone(), *shop);
            let mug = catalog
                .create_product(
                    "alice",
                    CreateProduct {
                        name: "Mug".into(),
                        slug: None,
                    },
                )
                .await
                .unwrap();
            catalog
                .price_product("alice", &mug.id, 1250, "EUR")
                .await
                .unwrap();
            catalog.publish_product("alice", &mug.id).await.unwrap();
            mugs.push(mug.id);
        }
        (store, db, mugs)
    }

    async fn start_cart(store: &Evento, shop: &str, mug: &str) -> String {
        Command::new(store.clone(), shop)
            .start_cart(
                STOREFRONT_ACTOR,
                CartItem {
                    product_id: mug.to_owned(),
                    variant_id: None,
                    quantity: 1,
                },
            )
            .await
            .unwrap()
    }

    /// The read model caught up; the carts it still lists, every shop's.
    async fn listed(store: &Evento, db: &Db) -> Vec<String> {
        read_model_subscription::<Evento>(db.clone())
            .run_once(store)
            .await
            .unwrap();
        let rows: Vec<(String,)> = db
            .fetch_all("SELECT id FROM orders_carts ORDER BY id", ())
            .await
            .unwrap();
        rows.into_iter().map(|(id,)| id).collect()
    }

    fn node(store: &Evento, db: &Db, share: Share) -> Arc<IdleCarts> {
        IdleCarts::new(db.clone(), store.clone(), Arc::new(move || share), IDLE)
    }

    #[tokio::test]
    async fn each_node_drops_its_own_part_of_the_idle_carts_of_every_shop() {
        let (store, db, mugs) = store(&["acme", "globex"]).await;
        let mut carts = Vec::new();
        for _ in 0..4 {
            carts.push(start_cart(&store, "acme", &mugs[0]).await);
            carts.push(start_cart(&store, "globex", &mugs[1]).await);
        }
        carts.sort();
        assert_eq!(listed(&store, &db).await, carts);
        let now = crate::auth::now();
        let first = Share { index: 0, of: 2 };
        let second = Share { index: 1, of: 2 };

        // Not idle yet: nothing goes.
        assert_eq!(node(&store, &db, first).sweep(now).await.unwrap(), 0);

        let later = now + IDLE.as_secs();
        let dropped = node(&store, &db, first).sweep(later).await.unwrap();
        let theirs: Vec<String> = carts.iter().filter(|id| second.owns(id)).cloned().collect();
        assert_eq!(dropped, carts.len() - theirs.len());
        assert_eq!(listed(&store, &db).await, theirs);
        // The other node's part, then nothing left to drop.
        let dropped = node(&store, &db, second).sweep(later).await.unwrap();
        assert_eq!(dropped, theirs.len());
        assert!(listed(&store, &db).await.is_empty());
        assert_eq!(
            node(&store, &db, Share::ALONE).sweep(later).await.unwrap(),
            0
        );
        // Gone for the storefront too.
        let orders = Command::new(store.clone(), "acme");
        for cart in &carts {
            assert!(orders.load_cart(cart).await.unwrap().is_none());
        }
    }

    #[tokio::test]
    async fn a_look_drops_so_many_and_the_next_goes_on() {
        let (store, db, mugs) = store(&["acme"]).await;
        let mut carts = Vec::new();
        for _ in 0..5 {
            carts.push(start_cart(&store, "acme", &mugs[0]).await);
        }
        assert_eq!(listed(&store, &db).await.len(), 5);
        let later = crate::auth::now() + IDLE.as_secs();
        let alone = node(&store, &db, Share::ALONE);

        // Two rows a page, three carts a look: the rows a look already
        // judged are never read again in it.
        assert_eq!(alone.look(later, 2, 3).await.unwrap(), 3);
        assert_eq!(listed(&store, &db).await.len(), 2);
        assert_eq!(alone.look(later, 2, 3).await.unwrap(), 2);
        assert!(listed(&store, &db).await.is_empty());
    }

    #[tokio::test]
    async fn a_sweep_looks_again_while_a_look_drops_its_full_share() {
        let (store, db, mugs) = store(&["acme"]).await;
        for _ in 0..7 {
            start_cart(&store, "acme", &mugs[0]).await;
        }
        assert_eq!(listed(&store, &db).await.len(), 7);
        let later = crate::auth::now() + IDLE.as_secs();

        // Three a look: three, three, then the last one, which ends it. The
        // rows dropped by one look are still listed for the next (nothing
        // follows the read model here), and passed over.
        let dropped = node(&store, &db, Share::ALONE)
            .sweep_paced(later, Duration::ZERO, 2, 3)
            .await
            .unwrap();
        assert_eq!(dropped, 7);
        assert!(listed(&store, &db).await.is_empty());
    }
}
