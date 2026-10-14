//! The tenant runtime end to end on one node: a tenant's database is created
//! on first use, written and read through its own consensus group, closed when
//! too many are open, and reopened with its read models resuming
//! where they stopped. The catalog's product list is the real read model fed
//! this way; a probe subscription records what the runtime delivers.
//!
//! The dashboard is a binary crate, so the runtime is reached through the
//! binary's own test harness (`#[path]` includes of the modules under test),
//! which drags in items this test never calls.
#![allow(dead_code)]

#[path = "../src/cluster.rs"]
mod cluster;
#[path = "../src/config.rs"]
mod config;
#[path = "../src/limits.rs"]
mod limits;
#[path = "../src/tenants.rs"]
mod tenants;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use evento::metadata::RawEvent;
use evento::migrator::Migrate;
use evento::subscription::{Context, SubscriptionBuilder};
use evento_accord::GroupId;
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx_migrator::Info;
use timada_catalog::{CreateProduct, ProductFilter, count_products, products_of_tenant};
use timada_tenancy::aggregator::{Renamed, Tenant};
use timada_tenancy::{Command, CreateTenant, TenantState, read_model_subscription};

use cluster::Cluster;
use config::TenantsConfig;
use tenants::{CatchUp, TenantStatus, TenantSubscription, Tenants};

struct Harness {
    tenants: Arc<Tenants>,
    platform: Command<cluster::Executor>,
    pool: SqlitePool,
    dir: PathBuf,
    /// Every event the probe read model saw, as `(tenant id, event name)`.
    seen: Arc<Mutex<Vec<(String, String)>>>,
}

/// A read model fed from each tenant's stream: records what it is given, into
/// a list shared with the test instead of a table.
fn probe(seen: Arc<Mutex<Vec<(String, String)>>>) -> TenantSubscription {
    Arc::new(move |_pool: SqlitePool, tenant_id: &str| {
        SubscriptionBuilder::new("test/probe")
            .data(seen.clone())
            .data(tenant_id.to_owned())
            .handler(on_tenant_event())
            .any_routing_key()
    })
}

#[evento::subscription_all]
async fn on_tenant_event<E: evento::Executor>(
    context: &Context<'_, E>,
    event: RawEvent<Tenant>,
) -> anyhow::Result<()> {
    let seen: Arc<Mutex<Vec<(String, String)>>> = context.extract();
    let tenant_id: String = context.extract();
    seen.lock()
        .unwrap()
        .push((tenant_id, event.0.name.to_string()));
    Ok(())
}

async fn harness(max_open: usize) -> Harness {
    let dir = std::env::temp_dir().join(format!("timada-tenants-{}", ulid::Ulid::generate()));
    std::fs::create_dir_all(&dir).unwrap();
    let options = SqliteConnectOptions::new()
        .filename(dir.join("platform.db"))
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal);
    let pool = SqlitePoolOptions::new()
        .max_connections(3)
        .connect_with(options)
        .await
        .unwrap();
    {
        let mut conn = pool.acquire().await.unwrap();
        let mut migrator = evento::sql_migrator::new::<sqlx::Sqlite>().unwrap();
        migrator
            .add_migrations(timada_tenancy::migrations())
            .unwrap();
        migrator
            .add_migrations(timada_catalog::migrations())
            .unwrap();
        migrator
            .run(&mut *conn, &evento::migrator::Plan::apply_all())
            .await
            .unwrap();
    }
    let cluster = Arc::new(Cluster::build(None).await.unwrap());
    let platform = cluster
        .open_platform(None, pool.clone(), pool.clone())
        .await
        .unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let tenants = Tenants::new(
        cluster,
        &TenantsConfig {
            dir: dir.join("tenants"),
            max_open: Some(max_open),
        },
        None,
        platform.clone(),
        pool.clone(),
        vec![
            probe(seen.clone()),
            Arc::new(timada_catalog::read_model_subscription),
        ],
    );
    Harness {
        tenants,
        platform: Command(platform),
        pool,
        dir,
        seen,
    }
}

impl Harness {
    async fn create(&self, slug: &str) -> TenantState {
        let tenant = self
            .platform
            .create_tenant(CreateTenant {
                name: slug.into(),
                owner_user_id: "alice".into(),
            })
            .await
            .unwrap();
        // The catalog row the runtime consults for group lookups.
        read_model_subscription::<cluster::Executor>(self.pool.clone())
            .run_once(&self.platform.0)
            .await
            .unwrap();
        tenant
    }

    /// Writes a probe event into the tenant's own store (the tenancy events
    /// double as a payload; nothing of the catalog is involved).
    async fn write(&self, tenant: &TenantState, stream: &str, name: &str, version: u16) {
        let executor = self.tenants.open(tenant).await.unwrap();
        evento::append(stream)
            .original_version(version)
            .event(&Renamed { name: name.into() })
            .commit(&executor)
            .await
            .unwrap();
    }

    async fn seen_by(&self, tenant: &TenantState) -> Vec<String> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|(id, _)| *id == tenant.id)
            .map(|(_, name)| name.clone())
            .collect()
    }

    async fn wait_seen(&self, tenant: &TenantState, count: usize) {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if self.seen_by(tenant).await.len() >= count {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .unwrap_or_else(|_| {
            panic!(
                "probe saw {:?}, wanted {count} events",
                names_seen(&self.seen, &tenant.id)
            )
        });
    }
}

fn names_seen(seen: &Mutex<Vec<(String, String)>>, tenant_id: &str) -> Vec<String> {
    seen.lock()
        .unwrap()
        .iter()
        .filter(|(id, _)| id == tenant_id)
        .map(|(_, n)| n.clone())
        .collect()
}

#[tokio::test]
async fn a_tenant_database_opens_on_use_and_resumes_after_closing() {
    let h = harness(8).await;
    let acme = h.create("acme").await;
    assert_eq!(h.tenants.status(&acme.id), TenantStatus::Stopped);
    assert!(!h.tenants.path_of(&acme.id).exists());

    // First use creates and migrates the file; the write goes through the
    // tenant's own group and reaches its read model.
    h.write(&acme, "probe", "one", 0).await;
    assert!(matches!(
        h.tenants.status(&acme.id),
        TenantStatus::Running { .. }
    ));
    assert!(h.tenants.path_of(&acme.id).exists());
    assert_eq!(h.tenants.open_count(), 1);
    h.wait_seen(&acme, 1).await;
    let executor = h.tenants.open(&acme).await.unwrap();
    let loaded = timada_tenancy::tenant_projection::<cluster::Executor>()
        .load("probe")
        .execute(&executor)
        .await
        .unwrap()
        .expect("the probe stream is in the tenant store");
    assert_eq!(loaded.name, "one");
    assert!(
        h.platform.load_tenant("probe").await.unwrap().is_none(),
        "nothing landed in the platform store"
    );

    // Unused, it stays open; closed, its file stays.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(matches!(
        h.tenants.status(&acme.id),
        TenantStatus::Running { .. }
    ));
    h.tenants.close(&acme.id).await;
    assert_eq!(h.tenants.status(&acme.id), TenantStatus::Stopped);
    assert_eq!(h.tenants.open_count(), 0);
    assert!(h.tenants.path_of(&acme.id).exists(), "the file stays");

    // Reopen: the data is still there and the read model resumes from its
    // cursor, so the old event is not delivered again.
    h.write(&acme, "probe", "two", 1).await;
    h.wait_seen(&acme, 2).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(h.seen_by(&acme).await, vec!["Renamed", "Renamed"]);
    let executor = h.tenants.open(&acme).await.unwrap();
    let loaded = timada_tenancy::tenant_projection::<cluster::Executor>()
        .load("probe")
        .execute(&executor)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded.name, "two");

    h.tenants.shutdown().await;
    assert_eq!(h.tenants.open_count(), 0);
    let _ = std::fs::remove_dir_all(&h.dir);
}

#[tokio::test]
async fn too_many_open_tenants_evicts_the_least_recently_used() {
    let h = harness(2).await;
    let a = h.create("aaa").await;
    let b = h.create("bbb").await;
    let c = h.create("ccc").await;

    h.tenants.open(&a).await.unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    h.tenants.open(&b).await.unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    h.tenants.open(&a).await.unwrap(); // a is now the most recent
    tokio::time::sleep(Duration::from_millis(20)).await;
    h.tenants.open(&c).await.unwrap();

    assert_eq!(h.tenants.open_count(), 2);
    assert_eq!(
        h.tenants.status(&b.id),
        TenantStatus::Stopped,
        "b was least recently used"
    );
    assert!(matches!(
        h.tenants.status(&a.id),
        TenantStatus::Running { .. }
    ));
    assert!(matches!(
        h.tenants.status(&c.id),
        TenantStatus::Running { .. }
    ));

    h.tenants.shutdown().await;
    let _ = std::fs::remove_dir_all(&h.dir);
}

#[tokio::test]
async fn a_request_holds_its_tenant_against_eviction() {
    let h = harness(1).await;
    let a = h.create("aaa").await;
    let b = h.create("bbb").await;
    let c = h.create("ccc").await;

    let hold = h.tenants.hold(&a).await.unwrap();
    assert!(
        matches!(
            h.tenants.status(&a.id),
            TenantStatus::Running { leases: 0, .. }
        ),
        "a request is not shown as a store connection"
    );

    // It is not evicted to make room for another tenant.
    h.tenants.open(&b).await.unwrap();
    assert!(matches!(
        h.tenants.status(&a.id),
        TenantStatus::Running { .. }
    ));
    let probe = timada_tenancy::tenant_projection::<cluster::Executor>()
        .load("probe")
        .execute(hold.executor())
        .await;
    assert!(probe.is_ok(), "its database is still usable: {probe:?}");

    // Once the request is done, it can go like any other.
    drop(hold);
    tokio::time::sleep(Duration::from_millis(20)).await;
    h.tenants.open(&c).await.unwrap();
    assert_eq!(h.tenants.status(&a.id), TenantStatus::Stopped);

    h.tenants.shutdown().await;
    let _ = std::fs::remove_dir_all(&h.dir);
}

#[tokio::test]
async fn suspended_and_unknown_tenants_are_not_opened() {
    let h = harness(8).await;
    let acme = h.create("acme").await;
    h.platform.suspend_tenant(&acme.id, "unpaid").await.unwrap();
    let suspended = h.platform.load_tenant(&acme.id).await.unwrap().unwrap();
    let err = match h.tenants.open(&suspended).await {
        Ok(_) => panic!("a suspended tenant must not open"),
        Err(err) => err,
    };
    assert!(err.to_string().contains("suspended"), "{err}");
    assert_eq!(h.tenants.open_count(), 0);

    // A peer talking about a group the catalog does not know is ignored, and
    // one it knows but which is suspended stays closed.
    h.tenants.reopen(GroupId(0xdead_beef));
    h.tenants
        .reopen(GroupId(timada_tenancy::group_of(&acme.id)));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(h.tenants.open_count(), 0);

    // Resumed: the hook opens it on the catalog's word.
    h.platform.resume_tenant(&acme.id).await.unwrap();
    h.tenants
        .reopen(GroupId(timada_tenancy::group_of(&acme.id)));
    tokio::time::timeout(Duration::from_secs(10), async {
        while h.tenants.open_count() == 0 {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the hook opens a known, active tenant");
    assert!(matches!(
        h.tenants.status(&acme.id),
        TenantStatus::Running { .. }
    ));

    h.tenants.shutdown().await;
    let _ = std::fs::remove_dir_all(&h.dir);
}

#[tokio::test]
async fn a_peer_asking_for_a_tenant_the_catalog_learns_late_still_opens_it() {
    let h = harness(8).await;
    let acme = h.create("acme").await;
    let group = timada_tenancy::group_of(&acme.id) as i64;

    // Created on another node a moment ago: its catalog row is not here yet
    // when the first frames arrive.
    sqlx::query("UPDATE tenancy_tenants SET group_id = -1 WHERE id = ?1")
        .bind(&acme.id)
        .execute(&h.pool)
        .await
        .unwrap();
    h.tenants.reopen(GroupId(group as u64));
    // The transport repeats its notice; one lookup is enough.
    h.tenants.reopen(GroupId(group as u64));
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(h.tenants.open_count(), 0);

    // The row lands: the lookup still running finds it and opens the tenant.
    sqlx::query("UPDATE tenancy_tenants SET group_id = ?1 WHERE id = ?2")
        .bind(group)
        .bind(&acme.id)
        .execute(&h.pool)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while h.tenants.open_count() == 0 {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the retried lookup opens the tenant");

    h.tenants.shutdown().await;
    let _ = std::fs::remove_dir_all(&h.dir);
}

#[tokio::test]
async fn a_tenant_the_cluster_wrote_to_is_not_evicted_first() {
    let h = harness(2).await;
    let a = h.create("aaa").await;
    let b = h.create("bbb").await;
    let c = h.create("ccc").await;

    let executor = h.tenants.open(&a).await.unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    h.tenants.open(&b).await.unwrap();
    // A write that does not go through `open`, as on a node that only takes
    // part in another node's writes: no request, but the group moved.
    evento::append("probe")
        .original_version(0)
        .event(&Renamed { name: "one".into() })
        .commit(&executor)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    h.tenants.open(&c).await.unwrap();

    assert!(matches!(
        h.tenants.status(&a.id),
        TenantStatus::Running { .. }
    ));
    assert_eq!(
        h.tenants.status(&b.id),
        TenantStatus::Stopped,
        "b was the least recently used once a's write counted"
    );

    h.tenants.shutdown().await;
    let _ = std::fs::remove_dir_all(&h.dir);
}

#[tokio::test]
async fn a_new_tenant_opens_as_soon_as_its_creation_arrives() {
    let h = harness(8).await;
    let acme = h.create("acme").await;
    assert_eq!(h.tenants.status(&acme.id), TenantStatus::Stopped);

    // No request asked for it: its creation alone opens it, so its group
    // runs here before another node's first write needs it.
    tenants::creation_subscription(h.tenants.clone())
        .run_once(&h.platform.0)
        .await
        .unwrap();
    assert!(matches!(
        h.tenants.status(&acme.id),
        TenantStatus::Running { .. }
    ));

    h.tenants.shutdown().await;
    let _ = std::fs::remove_dir_all(&h.dir);
}

/// Short holds, so a pass over a few tenants takes well under a second.
const QUICK_CATCH_UP: CatchUp = CatchUp {
    min_hold: Duration::from_millis(300),
    quiet: Duration::from_millis(100),
    max_hold: Duration::from_secs(5),
    parallel: 2,
};

#[tokio::test]
async fn the_catch_up_pass_opens_each_closed_tenant_and_keeps_it_open() {
    let h = harness(8).await;
    let acme = h.create("acme").await;
    let beta = h.create("beta").await;
    let gamma = h.create("gamma").await;
    let delta = h.create("delta").await;
    h.platform
        .suspend_tenant(&gamma.id, "unpaid")
        .await
        .unwrap();
    read_model_subscription::<cluster::Executor>(h.pool.clone())
        .run_once(&h.platform.0)
        .await
        .unwrap();
    // Already open: catching up on its own, so the pass leaves it alone.
    h.tenants.open(&delta).await.unwrap();

    let pass = tokio::spawn({
        let tenants = h.tenants.clone();
        async move { tenants.catch_up_all(QUICK_CATCH_UP).await }
    });
    tokio::time::timeout(Duration::from_secs(10), async {
        while !matches!(h.tenants.status(&acme.id), TenantStatus::Running { .. }) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the pass opens a closed tenant");
    tokio::time::timeout(Duration::from_secs(10), pass)
        .await
        .expect("the pass ends")
        .unwrap();

    for tenant in [&acme, &beta] {
        assert!(matches!(
            h.tenants.status(&tenant.id),
            TenantStatus::Running { .. }
        ));
    }
    // A suspended tenant is not opened, not even to catch up.
    assert!(!h.tenants.path_of(&gamma.id).exists());
    assert!(matches!(
        h.tenants.status(&delta.id),
        TenantStatus::Running { .. }
    ));

    h.tenants.shutdown().await;
    let _ = std::fs::remove_dir_all(&h.dir);
}

/// Waits until the shared product list of `tenant` satisfies `ok`.
async fn wait_products(
    pool: &SqlitePool,
    tenant: &TenantState,
    what: &str,
    mut ok: impl FnMut(&[timada_catalog::ProductRow]) -> bool,
) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let rows = products_of_tenant(pool, &tenant.id, &ProductFilter::default(), 0, 10)
                .await
                .unwrap();
            if ok(&rows) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out: {what}"));
}

#[tokio::test]
async fn a_tenant_read_model_lands_in_the_shared_database() {
    let h = harness(8).await;
    let acme = h.create("acme").await;
    let zeta = h.create("zeta").await;

    // A product written through acme's own store shows up in the shared
    // list, under acme only.
    let catalog = timada_catalog::Command(h.tenants.open(&acme).await.unwrap());
    let mug = catalog
        .create_product(
            "alice",
            CreateProduct {
                name: "Blue Mug".into(),
                slug: None,
            },
        )
        .await
        .unwrap();
    wait_products(&h.pool, &acme, "the product is listed", |rows| {
        rows.len() == 1 && rows[0].id == mug.id
    })
    .await;
    assert_eq!(
        count_products(&h.pool, &zeta.id, &ProductFilter::default())
            .await
            .unwrap(),
        0
    );
    assert!(
        catalog
            .load_product_by_slug("blue-mug")
            .await
            .unwrap()
            .is_some()
    );

    // Closed, reopened on use: the read model resumes from its cursor in
    // the tenant's file and updates the same row.
    h.tenants.close(&acme.id).await;
    assert_eq!(h.tenants.status(&acme.id), TenantStatus::Stopped);
    let catalog = timada_catalog::Command(h.tenants.open(&acme).await.unwrap());
    catalog
        .price_product("alice", &mug.id, 1250, "EUR")
        .await
        .unwrap();
    wait_products(&h.pool, &acme, "the price reaches the row", |rows| {
        rows.len() == 1 && rows[0].amount_cents == Some(1250)
    })
    .await;
    assert_eq!(
        count_products(&h.pool, &acme.id, &ProductFilter::default())
            .await
            .unwrap(),
        1
    );

    h.tenants.shutdown().await;
    let _ = std::fs::remove_dir_all(&h.dir);
}
