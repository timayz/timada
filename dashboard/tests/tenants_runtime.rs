//! The tenant runtime end to end on one node: a tenant's database is created
//! on first use, written and read through its own consensus group, closed when
//! idle or when too many are open, and reopened with its read models resuming
//! where they stopped.
//!
//! The dashboard is a binary crate, so the runtime is reached through the
//! binary's own test harness (`#[path]` includes of the modules under test),
//! which drags in items this test never calls.
#![allow(dead_code)]

#[path = "../src/cluster.rs"]
mod cluster;
#[path = "../src/config.rs"]
mod config;
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
use timada_tenancy::aggregator::{Renamed, Tenant};
use timada_tenancy::{Command, CreateTenant, TenantState, read_model_subscription};

use cluster::Cluster;
use config::TenantsConfig;
use tenants::{TenantStatus, TenantSubscription, Tenants};

const IDLE: Duration = Duration::from_millis(300);

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
            idle_timeout_secs: 1,
            max_open,
        },
        None,
        platform.clone(),
        pool.clone(),
        vec![probe(seen.clone())],
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
                slug: slug.into(),
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
async fn a_tenant_database_opens_on_use_closes_when_idle_and_resumes() {
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

    // Idle: a sweep before the timeout keeps it, one after closes it.
    h.tenants.sweep().await;
    assert!(matches!(
        h.tenants.status(&acme.id),
        TenantStatus::Running { .. }
    ));
    tokio::time::sleep(IDLE * 4).await;
    h.tenants.sweep().await;
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
