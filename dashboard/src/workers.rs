//! The subscriptions this node runs, kept running.
//!
//! A subscription's worker ends for good on its own once a pass kept failing
//! past its retries: the store was away longer than that, or a handler
//! cannot deal with one event. Left there, what it feeds goes stale on this
//! node for every tenant, silently. So every worker runs under a supervisor
//! that starts it again from its saved cursor after a growing delay (it
//! resumes at the event that failed: a cursor is acknowledged only past the
//! handlers that ran) and records how it stands, for `/admin/cluster`. The
//! delay keeps growing while the restarted worker fails again soon after
//! (the same event, still poison): the wait and the count are forgotten
//! only once a restart lived longer than the longest delay. Ported from the
//! storefront's catalog supervisor (`crates/storefront/src/store.rs`).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context as _, Result};
use evento::subscription::{StopReason, Subscription, SubscriptionBuilder};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::cluster::Executor;

/// The wait before the first restart of a worker that failed, doubling up to
/// [`MAX_RESTART_DELAY`] while it keeps failing. A restarted worker that ran
/// for longer than the longest delay counts as recovered.
const FIRST_RESTART_DELAY: Duration = Duration::from_secs(1);
const MAX_RESTART_DELAY: Duration = Duration::from_secs(5 * 60);

/// How a worker stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkerState {
    Running,
    /// The worker ended on a failure and is started again from its cursor
    /// after a delay. `since` is when it first failed (unix seconds) and
    /// `attempts` how many restarts were tried since, the one being waited
    /// for included; a restart that failed again soon after counts on.
    Restarting {
        error: String,
        since: u64,
        attempts: u32,
    },
    /// Ended and not started again: on shutdown, or because a handler asked
    /// or another worker took the subscription over.
    Stopped {
        reason: String,
    },
}

impl WorkerState {
    pub fn is_running(&self) -> bool {
        matches!(self, Self::Running)
    }
}

/// A supervised subscription.
pub struct Worker {
    name: String,
    state: Mutex<WorkerState>,
    stop: watch::Sender<bool>,
    supervisor: Mutex<Option<JoinHandle<()>>>,
}

impl Worker {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn state(&self) -> WorkerState {
        self.state.lock().unwrap().clone()
    }

    fn set(&self, state: WorkerState) {
        *self.state.lock().unwrap() = state;
    }

    /// Signals the worker to stop, without waiting for it. The supervisor
    /// holds the worker, so dropping the handles alone stops nothing: stop
    /// it.
    pub fn stop(&self) {
        let _ = self.stop.send(true);
    }

    /// Waits for the worker to exit; [`stop`](Self::stop) first.
    pub async fn stopped(&self) {
        let supervisor = self.supervisor.lock().unwrap().take();
        if let Some(supervisor) = supervisor {
            let _ = supervisor.await;
        }
    }
}

/// Starts the subscription `make` builds and keeps it running under the name
/// `name`. The first start failing is an error: the node does not serve
/// without its workers. Later failures restart it.
pub async fn start<F>(name: impl Into<String>, make: F, executor: &Executor) -> Result<Arc<Worker>>
where
    F: Fn() -> SubscriptionBuilder<Executor> + Send + Sync + 'static,
{
    let name = name.into();
    let handle = make()
        .start(executor)
        .await
        .with_context(|| format!("cannot start the {name} subscription"))?;
    let (stop, stop_rx) = watch::channel(false);
    let worker = Arc::new(Worker {
        name,
        state: Mutex::new(WorkerState::Running),
        stop,
        supervisor: Mutex::new(None),
    });
    let supervisor = tokio::spawn(supervise(
        worker.clone(),
        handle,
        make,
        executor.clone(),
        stop_rx,
    ));
    *worker.supervisor.lock().unwrap() = Some(supervisor);
    Ok(worker)
}

/// Runs the subscription `builder` over everything the store holds for it,
/// once, before anything is served: after the store was copied from a peer,
/// the read models are empty until their subscriptions caught up.
pub async fn catch_up(
    name: &str,
    builder: SubscriptionBuilder<Executor>,
    executor: &Executor,
) -> Result<()> {
    // One more try at a failing pass, no more: a store that stays away or an
    // event a handler cannot deal with is for the supervised worker, which
    // resumes at the cursor the pass left.
    builder
        .retry(1)
        .run_once(executor)
        .await
        .with_context(|| format!("the {name} subscription cannot catch up"))
}

async fn supervise<F>(
    worker: Arc<Worker>,
    mut handle: Subscription,
    make: F,
    executor: Executor,
    mut stop: watch::Receiver<bool>,
) where
    F: Fn() -> SubscriptionBuilder<Executor>,
{
    let name = worker.name.clone();
    let mut delay = FIRST_RESTART_DELAY;
    // The first failure of the current run of failures, and how many
    // restarts it took so far; forgotten once a restart held up.
    let mut since = 0;
    let mut attempts = 0;
    let mut restarted_at: Option<std::time::Instant> = None;
    loop {
        let reason = tokio::select! {
            _ = stop.changed() => {
                handle.shutdown().await.ok();
                worker.set(WorkerState::Stopped { reason: StopReason::Shutdown.to_string() });
                return;
            }
            reason = handle.stopped() => reason,
        };
        if !reason.is_failure() {
            tracing::info!(%reason, "the {name} subscription stopped");
            worker.set(WorkerState::Stopped {
                reason: reason.to_string(),
            });
            return;
        }
        let error = reason.to_string();
        tracing::error!(%error, "the {name} subscription stopped; restarting it");
        if restarted_at.is_none_or(|at| at.elapsed() >= MAX_RESTART_DELAY) {
            // Fresh trouble, or the last restart held up long enough to
            // count as recovered: the wait starts short again.
            delay = FIRST_RESTART_DELAY;
            since = crate::auth::now();
            attempts = 0;
        }
        attempts += 1;
        worker.set(WorkerState::Restarting {
            error,
            since,
            attempts,
        });

        handle = loop {
            tokio::select! {
                _ = stop.changed() => {
                    worker.set(WorkerState::Stopped { reason: StopReason::Shutdown.to_string() });
                    return;
                }
                _ = tokio::time::sleep(delay) => {}
            }
            delay = (delay * 2).min(MAX_RESTART_DELAY);
            match make().start(&executor).await {
                Ok(handle) => break handle,
                Err(err) => {
                    let error = format!("{err:#}");
                    tracing::warn!(%error, "the {name} subscription cannot restart yet");
                    attempts += 1;
                    worker.set(WorkerState::Restarting {
                        error,
                        since,
                        attempts,
                    });
                }
            }
        };
        restarted_at = Some(std::time::Instant::now());
        worker.set(WorkerState::Running);
        tracing::info!("the {name} subscription is running again");
    }
}

/// Every worker of the node, for the cluster page.
#[derive(Clone, Default)]
pub struct Workers(Arc<Vec<Arc<Worker>>>);

impl Workers {
    pub fn new(workers: Vec<Arc<Worker>>) -> Self {
        Self(Arc::new(workers))
    }

    /// Each worker's name and how it stands, in start order.
    pub fn states(&self) -> Vec<(String, WorkerState)> {
        self.0
            .iter()
            .map(|worker| (worker.name().to_owned(), worker.state()))
            .collect()
    }

    /// Stops every worker and waits for them.
    pub async fn shutdown(&self) {
        for worker in self.0.iter() {
            worker.stop();
        }
        for worker in self.0.iter() {
            worker.stopped().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use evento::metadata::Event;
    use evento::subscription::Context;
    use evento::{Evento, Turso};
    use timada_tenancy::aggregator::Created;
    use timada_tenancy::{Command, CreateTenant};

    use super::*;

    /// What the handler saw: the tenant names, in order, failures counted
    /// apart.
    #[derive(Clone, Default)]
    struct Seen {
        names: Arc<Mutex<Vec<String>>>,
        failures_left: Arc<AtomicUsize>,
    }

    #[evento::subscription]
    async fn on_created<E: evento::Executor>(
        context: &Context<'_, E>,
        event: Event<Created>,
    ) -> anyhow::Result<()> {
        let seen: Seen = context.extract();
        if event.data.name == "poison"
            && seen
                .failures_left
                .try_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                    left.checked_sub(1)
                })
                .is_ok()
        {
            anyhow::bail!("cannot deal with {}", event.data.name);
        }
        seen.names.lock().unwrap().push(event.data.name.clone());
        Ok(())
    }

    fn builder(seen: Seen) -> SubscriptionBuilder<Executor> {
        SubscriptionBuilder::new("test/created")
            .data(seen)
            .handler(on_created())
            .any_routing_key()
            .retry(0)
    }

    async fn create(store: &Evento, name: &str) {
        Command(store.clone())
            .create_tenant(CreateTenant {
                name: name.into(),
                owner_user_id: "alice".into(),
            })
            .await
            .unwrap();
    }

    async fn eventually(what: &str, mut ok: impl AsyncFnMut() -> bool) {
        tokio::time::timeout(Duration::from_secs(10), async {
            while !ok().await {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("{what}"));
    }

    /// A pass that fails past its retries ends the worker; the supervisor
    /// starts it again from the event that failed, so nothing is skipped and
    /// nothing is replayed, and the page sees it restarting then running.
    #[tokio::test]
    async fn a_failed_worker_is_restarted_at_the_event_that_failed() {
        let store = Evento::new(Turso::in_memory_concurrent().await.unwrap());
        let seen = Seen::default();
        seen.failures_left.store(1, Ordering::SeqCst);
        create(&store, "first").await;
        let worker = start(
            "created",
            {
                let seen = seen.clone();
                move || builder(seen.clone())
            },
            &store,
        )
        .await
        .unwrap();
        eventually("the first event is seen", async || {
            seen.names.lock().unwrap().len() == 1
        })
        .await;
        assert_eq!(worker.state(), WorkerState::Running);

        create(&store, "poison").await;
        create(&store, "last").await;
        eventually("the worker is seen restarting", async || {
            matches!(worker.state(), WorkerState::Restarting { attempts: 1, .. })
        })
        .await;
        eventually(
            "the worker runs again and gets past the event",
            async || seen.names.lock().unwrap().len() == 3,
        )
        .await;
        assert_eq!(worker.state(), WorkerState::Running);
        assert_eq!(
            *seen.names.lock().unwrap(),
            ["first", "poison", "last"],
            "resumed at the failing event, which passed on the second delivery"
        );

        worker.stop();
        worker.stopped().await;
        assert_eq!(
            worker.state(),
            WorkerState::Stopped {
                reason: "shutdown requested".to_owned()
            }
        );
    }

    /// A worker that fails again right after its restart is waited for
    /// longer each time, and the page sees one run of failures, not a new
    /// one every half minute.
    #[tokio::test]
    async fn a_worker_failing_again_at_once_waits_longer_and_counts_on() {
        let store = Evento::new(Turso::in_memory_concurrent().await.unwrap());
        let seen = Seen::default();
        seen.failures_left.store(2, Ordering::SeqCst);
        let worker = start(
            "created",
            {
                let seen = seen.clone();
                move || builder(seen.clone())
            },
            &store,
        )
        .await
        .unwrap();
        create(&store, "poison").await;
        let first = tokio::time::Instant::now();
        eventually("the first failure is seen", async || {
            matches!(worker.state(), WorkerState::Restarting { attempts: 1, .. })
        })
        .await;
        let WorkerState::Restarting { since, .. } = worker.state() else {
            unreachable!()
        };
        // Restarted after 1 s, fails again at once: 2 s before the next.
        eventually("the second failure counts on", async || {
            matches!(worker.state(), WorkerState::Restarting { attempts: 2, .. })
        })
        .await;
        assert!(matches!(worker.state(), WorkerState::Restarting { since: s, .. } if s == since));
        eventually("the third start gets past the event", async || {
            seen.names.lock().unwrap().len() == 1
        })
        .await;
        assert!(
            first.elapsed() >= Duration::from_secs(3),
            "1 s then 2 s of waiting, not 1 s twice"
        );
        assert_eq!(worker.state(), WorkerState::Running);
        worker.stop();
        worker.stopped().await;
    }

    /// A shutdown during the restart wait ends the supervisor at once.
    #[tokio::test]
    async fn a_shutdown_while_waiting_to_restart_is_prompt() {
        let store = Evento::new(Turso::in_memory_concurrent().await.unwrap());
        let seen = Seen::default();
        seen.failures_left.store(usize::MAX, Ordering::SeqCst);
        let workers = Workers::new(vec![
            start(
                "created",
                {
                    let seen = seen.clone();
                    move || builder(seen.clone())
                },
                &store,
            )
            .await
            .unwrap(),
        ]);
        create(&store, "poison").await;
        eventually("the worker is seen restarting", async || {
            !workers.states()[0].1.is_running()
        })
        .await;
        tokio::time::timeout(Duration::from_secs(5), workers.shutdown())
            .await
            .expect("the shutdown does not wait out the restart delay");
        assert!(matches!(workers.states()[0].1, WorkerState::Stopped { .. }));
    }
}
