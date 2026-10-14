//! The tenant runtime: one event database per tenant, opened on activity and
//! closed when idle.
//!
//! A tenant is "up" while its database is open: its consensus group runs on
//! this node and its read-model subscriptions follow its stream into the
//! shared database. Every tenant-scoped request goes through [`Tenants::open`],
//! which is the activity signal; [`Tenants::sweep`] closes what has not been
//! asked for in `tenants.idle_timeout_secs`, and a bounded number stay open at
//! once. A peer that talks about a group this node has closed triggers
//! [`Tenants::reopen`], which opens it only if the catalog knows the tenant:
//! the wire cannot create one. A storefront connected through the store API
//! holds a [`TenantLease`]: a leased tenant is neither closed as idle nor
//! evicted (a store with subscriptions is never idle anyway), only by an
//! administrator or at shutdown. A web request holds a [`RequestHold`] the
//! same way while it runs, so a slow write is never cut by the sweeper.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result};
use evento::Evento;
use evento::migrator::Migrate;
use evento::subscription::{Subscription, SubscriptionBuilder};
use evento_accord::{AccordExecutor, GroupId};
use evento_sql::Sql;
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use timada_tenancy::{Command, TenantState, group_of, tenant_by_group};
use tokio::task::JoinHandle;

use crate::cluster::{Cluster, Executor, PENDING_TTL};
use crate::config::TenantsConfig;

/// A read model fed from one tenant's events into the shared database: given
/// the shared (writable) pool and the tenant id, the subscription to start on
/// the tenant's executor. Rows it writes carry the tenant id. Started when the
/// tenant opens, stopped when it closes; its cursor lives in the tenant's own
/// database, so it resumes where it left off.
pub type TenantSubscription =
    Arc<dyn Fn(SqlitePool, &str) -> SubscriptionBuilder<Executor> + Send + Sync>;

/// Whether a tenant's database is open on this node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TenantStatus {
    Running {
        since: SystemTime,
        /// Store connections holding the tenant open.
        leases: usize,
    },
    Stopped,
}

struct OpenTenant {
    tenant_id: String,
    executor: Executor,
    pool: SqlitePool,
    subscriptions: Vec<Subscription>,
    opened_at: SystemTime,
    last_used: Instant,
    /// Live [`TenantLease`]s; shared with each lease so a drop needs no lookup.
    leases: Arc<AtomicUsize>,
    /// Requests running against it, each holding a [`RequestHold`]. Kept
    /// apart from `leases`, which pages show as store connections.
    requests: Arc<AtomicUsize>,
}

impl OpenTenant {
    /// Whether anything holds it open: a store connection or a request.
    fn is_held(&self) -> bool {
        self.leases.load(Ordering::Acquire) > 0 || self.requests.load(Ordering::Acquire) > 0
    }
}

/// A store connection's hold on an open tenant, released on drop. The
/// counter is the one of the open entry the lease was taken on: a tenant
/// closed and reopened underneath an old lease is not affected by it.
pub struct TenantLease {
    executor: Executor,
    tenant_id: String,
    leases: Arc<AtomicUsize>,
}

impl TenantLease {
    pub fn executor(&self) -> &Executor {
        &self.executor
    }

    pub fn tenant_id(&self) -> &str {
        &self.tenant_id
    }
}

impl Drop for TenantLease {
    fn drop(&mut self) {
        self.leases.fetch_sub(1, Ordering::AcqRel);
    }
}

/// A web request's hold on an open tenant, released on drop: while it lives
/// the tenant is neither closed as idle nor evicted, so a write that takes
/// longer than the idle timeout still has its database.
pub struct RequestHold {
    executor: Executor,
    requests: Arc<AtomicUsize>,
}

impl RequestHold {
    pub fn executor(&self) -> &Executor {
        &self.executor
    }
}

impl Drop for RequestHold {
    fn drop(&mut self) {
        self.requests.fetch_sub(1, Ordering::AcqRel);
    }
}

pub struct Tenants {
    cluster: Arc<Cluster>,
    dir: PathBuf,
    idle_timeout: Duration,
    max_open: usize,
    region: Option<String>,
    /// The platform store, to check the catalog before opening on a peer's word.
    platform: Executor,
    /// The shared database the read models are written to.
    read_models: SqlitePool,
    subscriptions: Vec<TenantSubscription>,
    open: Mutex<HashMap<GroupId, OpenTenant>>,
    /// One open at a time per group: a request and the transport hook may race
    /// to open the same tenant, and must not both migrate a fresh file.
    open_locks: Mutex<HashMap<GroupId, Arc<tokio::sync::Mutex<()>>>>,
    /// Groups a peer's frames are being looked up for, so the transport's
    /// repeated notices do not start a second lookup of the same group.
    reopening: Mutex<HashSet<GroupId>>,
}

impl Tenants {
    pub fn new(
        cluster: Arc<Cluster>,
        config: &TenantsConfig,
        region: Option<String>,
        platform: Executor,
        read_models: SqlitePool,
        subscriptions: Vec<TenantSubscription>,
    ) -> Arc<Self> {
        let tenants = Arc::new(Self {
            cluster: cluster.clone(),
            dir: config.dir.clone(),
            idle_timeout: config.idle_timeout(),
            max_open: config.max_open.max(1),
            region,
            platform,
            read_models,
            subscriptions,
            open: Mutex::new(HashMap::new()),
            open_locks: Mutex::new(HashMap::new()),
            reopening: Mutex::new(HashSet::new()),
        });
        cluster.attach_tenants(tenants.clone());
        tenants
    }

    /// The file of a tenant's database.
    pub fn path_of(&self, tenant_id: &str) -> PathBuf {
        self.dir.join(format!("{tenant_id}.db"))
    }

    pub fn status(&self, tenant_id: &str) -> TenantStatus {
        let open = self.open.lock().expect("open poisoned");
        match open.get(&GroupId(group_of(tenant_id))) {
            Some(t) if t.tenant_id == tenant_id => TenantStatus::Running {
                since: t.opened_at,
                leases: t.leases.load(Ordering::Acquire),
            },
            _ => TenantStatus::Stopped,
        }
    }

    pub fn open_count(&self) -> usize {
        self.open.lock().expect("open poisoned").len()
    }

    /// The tenant's executor, opening its database if needed. Counts as
    /// activity. A suspended tenant is refused.
    pub async fn open(&self, tenant: &TenantState) -> Result<Executor> {
        anyhow::ensure!(
            !tenant.is_suspended(),
            "tenant {} is suspended",
            tenant.slug
        );
        let group = GroupId(group_of(&tenant.id));
        if let Some(executor) = self.touch(group, &tenant.id) {
            return Ok(executor);
        }
        let open_lock = Arc::clone(
            self.open_locks
                .lock()
                .expect("open_locks poisoned")
                .entry(group)
                .or_default(),
        );
        let _opening = open_lock.lock().await;
        if let Some(executor) = self.touch(group, &tenant.id) {
            return Ok(executor);
        }

        // Two ids hashing to one group would share a consensus group: refuse
        // the second rather than mix their streams.
        if let Some(other) = tenant_by_group(&self.read_models, group.0).await?
            && other.id != tenant.id
        {
            anyhow::bail!(
                "tenants {} and {} share consensus group {}; cannot open the second",
                other.slug,
                tenant.slug,
                group.0
            );
        }

        let path = self.path_of(&tenant.id);
        let (sql, pool) = open_db(&path)
            .await
            .with_context(|| format!("cannot open {}", path.display()))?;
        let node = match self
            .cluster
            .open_group(group, sql.clone(), pool.clone())
            .await
        {
            Ok(node) => node,
            Err(err) => {
                pool.close().await;
                return Err(err.context(format!("cannot open the group of tenant {}", tenant.slug)));
            }
        };
        let mut executor = Evento::new(AccordExecutor::<Sql<sqlx::Sqlite>>::new(node, sql));
        if let Some(region) = &self.region {
            executor = executor.default_routing_key(region.clone());
        }

        let mut subscriptions = Vec::with_capacity(self.subscriptions.len());
        for build in &self.subscriptions {
            let subscription = build(self.read_models.clone(), &tenant.id)
                .start(&executor)
                .await
                .with_context(|| format!("cannot start a read model of tenant {}", tenant.slug))?;
            subscriptions.push(subscription);
        }

        // No std lock guard may live across an await (the futures must stay
        // `Send`), so the bookkeeping is one synchronous block and the awaits
        // come after it.
        let evicted = {
            let mut open = self.open.lock().expect("open poisoned");
            open.insert(
                group,
                OpenTenant {
                    tenant_id: tenant.id.clone(),
                    executor: executor.clone(),
                    pool,
                    subscriptions,
                    opened_at: SystemTime::now(),
                    last_used: Instant::now(),
                    leases: Arc::new(AtomicUsize::new(0)),
                    requests: Arc::new(AtomicUsize::new(0)),
                },
            );
            let mut evicted = Vec::new();
            while open.len() > self.max_open {
                // Only a tenant nothing holds can go; when every one is held
                // the limit is exceeded rather than a live connection or a
                // running request cut.
                let Some((&victim, _)) = open
                    .iter()
                    .filter(|(g, t)| **g != group && !t.is_held())
                    .min_by_key(|(_, t)| t.last_used)
                else {
                    tracing::warn!(
                        open = open.len(),
                        max_open = self.max_open,
                        "every open tenant is held by a store connection or a request; over the limit"
                    );
                    break;
                };
                if let Some(t) = open.remove(&victim) {
                    evicted.push((victim, t));
                }
            }
            evicted
        };
        tracing::info!(tenant = %tenant.slug, group = group.0, path = %path.display(), "tenant opened");
        for (group, t) in evicted {
            tracing::info!(tenant = %t.tenant_id, group = group.0, "tenant closed (too many open)");
            self.shut(group, t).await;
        }
        Ok(executor)
    }

    /// Holds a tenant open for a store connection: [`open`](Self::open), then
    /// a lease that keeps the sweeper and the eviction away until it drops.
    pub async fn lease(&self, tenant: &TenantState) -> Result<TenantLease> {
        let executor = self.open(tenant).await?;
        let leases = self.counter_of(tenant, |t| &t.leases)?;
        Ok(TenantLease {
            executor,
            tenant_id: tenant.id.clone(),
            leases,
        })
    }

    /// Holds a tenant open for one request: [`open`](Self::open), then a hold
    /// that keeps the sweeper and the eviction away until it drops. Counts as
    /// activity, as `open` does.
    pub async fn hold(&self, tenant: &TenantState) -> Result<RequestHold> {
        let executor = self.open(tenant).await?;
        let requests = self.counter_of(tenant, |t| &t.requests)?;
        Ok(RequestHold { executor, requests })
    }

    /// Takes one on the counter `pick` names, on the tenant's open entry.
    fn counter_of(
        &self,
        tenant: &TenantState,
        pick: fn(&OpenTenant) -> &Arc<AtomicUsize>,
    ) -> Result<Arc<AtomicUsize>> {
        let group = GroupId(group_of(&tenant.id));
        let open = self.open.lock().expect("open poisoned");
        let counter = open
            .get(&group)
            .filter(|t| t.tenant_id == tenant.id)
            .map(|t| Arc::clone(pick(t)))
            .with_context(|| format!("tenant {} closed while being held", tenant.slug))?;
        // Under the lock, so the sweeper cannot pick the tenant between the
        // lookup and the count.
        counter.fetch_add(1, Ordering::AcqRel);
        Ok(counter)
    }

    /// Marks a tenant used and returns its executor when it is open.
    fn touch(&self, group: GroupId, tenant_id: &str) -> Option<Executor> {
        let mut open = self.open.lock().expect("open poisoned");
        let t = open.get_mut(&group).filter(|t| t.tenant_id == tenant_id)?;
        t.last_used = Instant::now();
        Some(t.executor.clone())
    }

    /// Closes a tenant's database now (an administrator suspended it, or the
    /// node is shutting down). Nothing to do when it is not open.
    pub async fn close(&self, tenant_id: &str) {
        let group = GroupId(group_of(tenant_id));
        let removed = {
            let mut open = self.open.lock().expect("open poisoned");
            match open.get(&group) {
                Some(t) if t.tenant_id == tenant_id => open.remove(&group),
                _ => None,
            }
        };
        if let Some(t) = removed {
            tracing::info!(tenant = %t.tenant_id, group = group.0, "tenant closed");
            self.shut(group, t).await;
        }
    }

    /// Closes every tenant idle for longer than the timeout. A tenant with a
    /// write in flight, or written to within the timeout, is not idle,
    /// whichever node took the write: a node that only takes part in another
    /// node's writes sees no request for the tenant, and closing it would
    /// leave those writes short of a quorum.
    pub async fn sweep(&self) {
        let candidates: Vec<(GroupId, String, SqlitePool)> = {
            let open = self.open.lock().expect("open poisoned");
            let cutoff = Instant::now() - self.idle_timeout;
            open.iter()
                .filter(|(_, t)| t.last_used <= cutoff && !t.is_held())
                .map(|(g, t)| (*g, t.tenant_id.clone(), t.pool.clone()))
                .collect()
        };
        let mut idle = Vec::new();
        for (group, tenant_id, pool) in candidates {
            if self.cluster.group_busy(group) || self.written_recently(&pool).await {
                self.touch(group, &tenant_id);
            } else {
                idle.push(group);
            }
        }
        let idle = {
            let mut open = self.open.lock().expect("open poisoned");
            let cutoff = Instant::now() - self.idle_timeout;
            // Used again or held while the events were being looked at: kept.
            let still_idle: Vec<GroupId> = idle
                .into_iter()
                .filter(|g| {
                    open.get(g)
                        .is_some_and(|t| t.last_used <= cutoff && !t.is_held())
                })
                .collect();
            still_idle
                .into_iter()
                .filter_map(|g| open.remove(&g).map(|t| (g, t)))
                .collect::<Vec<_>>()
        };
        for (group, t) in idle {
            tracing::info!(tenant = %t.tenant_id, group = group.0, "tenant closed (idle)");
            self.shut(group, t).await;
        }
    }

    /// Whether the tenant's newest event is younger than the idle timeout.
    /// Every node applies every write to its own copy, so this sees writes
    /// taken anywhere. A failed query counts as recent: keeping a tenant open
    /// one sweep longer costs less than closing it under a write.
    async fn written_recently(&self, pool: &SqlitePool) -> bool {
        let newest: Result<Option<i64>, sqlx::Error> =
            sqlx::query_scalar("SELECT MAX(timestamp) FROM event")
                .fetch_one(pool)
                .await;
        let Ok(newest) = newest else {
            return true;
        };
        let Some(newest) = newest else {
            return false;
        };
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(i64::MAX);
        // Whole seconds: an event stamped `newest` may be up to a second younger.
        now.saturating_sub(newest) <= self.idle_timeout.as_secs() as i64
    }

    /// Runs [`sweep`](Self::sweep) periodically until the task is aborted.
    pub fn start_sweeper(self: &Arc<Self>) -> JoinHandle<()> {
        let tenants = Arc::clone(self);
        let every =
            (self.idle_timeout / 4).clamp(Duration::from_millis(50), Duration::from_secs(30));
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(every);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                interval.tick().await;
                tenants.sweep().await;
            }
        })
    }

    /// The transport's unrouted-group hook: a peer sent frames for a group this
    /// node has not opened. Open it only if the catalog knows the tenant and it
    /// is not suspended; anything else is ignored.
    ///
    /// The catalog here is a read model, a moment behind the platform store: a
    /// tenant created on another node and written to straight away is not in
    /// it yet when the first frames arrive. The lookup is therefore retried for
    /// as long as the transport keeps those frames waiting; giving up on the
    /// first miss would leave the write without a quorum.
    pub fn reopen(self: &Arc<Self>, group: GroupId) {
        if !self
            .reopening
            .lock()
            .expect("reopening poisoned")
            .insert(group)
        {
            return;
        }
        let tenants = Arc::clone(self);
        tokio::spawn(async move {
            tenants.reopen_known(group).await;
            tenants
                .reopening
                .lock()
                .expect("reopening poisoned")
                .remove(&group);
        });
    }

    async fn reopen_known(&self, group: GroupId) {
        const RETRY: Duration = Duration::from_millis(100);
        let deadline = Instant::now() + PENDING_TTL;
        let row = loop {
            match tenant_by_group(&self.read_models, group.0).await {
                Ok(Some(row)) => break row,
                Ok(None) if Instant::now() + RETRY < deadline => {
                    tokio::time::sleep(RETRY).await;
                }
                Ok(None) => {
                    tracing::debug!(group = group.0, "frames for an unknown group ignored");
                    return;
                }
                Err(err) => {
                    tracing::warn!(group = group.0, %err, "cannot look the group up");
                    return;
                }
            }
        };
        let tenant = match Command(self.platform.clone()).load_tenant(&row.id).await {
            Ok(Some(tenant)) if !tenant.is_suspended() => tenant,
            Ok(_) => return,
            Err(err) => {
                tracing::warn!(tenant = %row.slug, %err, "cannot load the tenant");
                return;
            }
        };
        if let Err(err) = self.open(&tenant).await {
            tracing::warn!(tenant = %tenant.slug, %err, "cannot reopen the tenant for a peer");
        }
    }

    /// Closes every open tenant.
    pub async fn shutdown(&self) {
        let all: Vec<(GroupId, OpenTenant)> =
            self.open.lock().expect("open poisoned").drain().collect();
        for (group, t) in all {
            self.shut(group, t).await;
        }
    }

    async fn shut(&self, group: GroupId, t: OpenTenant) {
        for subscription in &t.subscriptions {
            subscription.stop();
        }
        for subscription in t.subscriptions {
            let reason = subscription.stopped().await;
            if reason.is_failure() {
                tracing::error!(tenant = %t.tenant_id, %reason, "a tenant read model had stopped");
            }
        }
        self.cluster.close_group(group);
        t.pool.close().await;
    }
}

/// Opens (creating and migrating if needed) a tenant's file: WAL with full
/// synchronous writes because the file is also the consensus journal, few
/// connections because sqlx-sqlite runs one OS thread per connection.
async fn open_db(path: &Path) -> Result<(Sql<sqlx::Sqlite>, SqlitePool)> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Full)
        .busy_timeout(Duration::from_secs(5));
    let pool = SqlitePoolOptions::new()
        .max_connections(3)
        .min_connections(0)
        .idle_timeout(Some(Duration::from_secs(30)))
        .connect_with(options)
        .await?;
    let mut conn = pool.acquire().await?;
    evento_sql_migrator::new::<sqlx::Sqlite>()?
        .run(&mut *conn, &sqlx_migrator::Plan::apply_all())
        .await?;
    drop(conn);
    // One `Sql` for both the data-store bridge and the executor: two `.into()`s
    // from the same pool would not share the write-wake channel subscriptions
    // rely on.
    let sql: Sql<sqlx::Sqlite> = pool.clone().into();
    Ok((sql, pool))
}
